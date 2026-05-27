use crate::crypto::Address;
use crate::storage::ChainStore;
use crate::types::{Amount, StateDiff, Transaction};
use crate::{Context, Env, ExitReason, LiteVM, StateDB, Value};

use super::{decode_contract_blob, CallRequest, CallResult};
use ethnum::U256;
use std::collections::HashMap;

pub struct VmExecution {
    pub success: bool,
    pub exit_reason: ExitReason,
    pub gas_used: u64,
    pub events: Vec<(u16, Vec<Value>)>,
}

pub struct VmBlockContext {
    pub height: u64,
    pub timestamp: u64,
    pub chain_id: u64,
    pub gas_price: Amount,
}

pub fn execute_contract_tx(
    store: &ChainStore,
    tx: &Transaction,
    code_address: Address,
    code: Vec<u8>,
    block: VmBlockContext,
    diffs: &mut Vec<StateDiff>,
) -> anyhow::Result<VmExecution> {
    let contract = decode_contract_blob(&code)?;
    let mut overlay = OverlayState::new(store);
    let ctx = Context {
        caller: tx.from,
        address: code_address,
        value: U256::from(tx.value),
        static_call: false,
        metadata: contract.metadata,
        code: contract.code,
        call_data: tx.payload.clone(),
        return_data: Vec::new(),
    };
    let env = Env {
        block_num: block.height,
        timestamp: block.timestamp,
        chain_id: U256::from(block.chain_id),
        origin: tx.from,
        gas_price: U256::from(block.gas_price),
    };
    let mut vm = LiteVM::new(ctx, env, &mut overlay, tx.gas_limit);
    let exit_reason = vm.run();
    let success = matches!(exit_reason, ExitReason::Halt | ExitReason::Return(_));
    let gas_used = tx.gas_limit.saturating_sub(vm.gas);
    let events = if success {
        vm.events.clone()
    } else {
        Vec::new()
    };
    drop(vm);
    if success {
        overlay.commit(diffs)?;
    }
    Ok(VmExecution {
        success,
        exit_reason,
        gas_used,
        events,
    })
}

struct OverlayState<'a> {
    store: &'a ChainStore,
    writes: HashMap<(Address, u8), Value>,
}

impl<'a> OverlayState<'a> {
    fn new(store: &'a ChainStore) -> Self {
        Self {
            store,
            writes: HashMap::new(),
        }
    }

    fn commit(self, diffs: &mut Vec<StateDiff>) -> anyhow::Result<()> {
        for ((address, field), value) in self.writes {
            self.store
                .put_vm_state_value(&address, field, &value, diffs)?;
        }
        Ok(())
    }
}

pub(crate) fn call_contract_with_overlay(
    store: &ChainStore,
    request: CallRequest,
) -> Result<CallResult, ExitReason> {
    let mut overlay = OverlayState::new(store);
    let result = overlay.call_contract(request)?;
    if result.success {
        let mut diffs = Vec::new();
        overlay
            .commit(&mut diffs)
            .map_err(|_| ExitReason::ContractNotFound)?;
    }
    Ok(result)
}

impl StateDB for OverlayState<'_> {
    fn get_state(&mut self, address: &[u8; 32], field_idx: u8) -> Value {
        self.writes
            .get(&(*address, field_idx))
            .cloned()
            .unwrap_or_else(|| self.store.get_vm_state_value(address, field_idx))
    }

    fn set_state(
        &mut self,
        address: &[u8; 32],
        field_idx: u8,
        value: Value,
    ) -> Result<(), ExitReason> {
        self.writes.insert((*address, field_idx), value);
        Ok(())
    }

    fn get_balance(&self, address: &[u8; 32]) -> U256 {
        self.store
            .get_account(address)
            .map(|a| U256::from(a.balance))
            .unwrap_or(U256::ZERO)
    }

    fn call_contract(&mut self, request: CallRequest) -> Result<CallResult, ExitReason> {
        let account = self
            .store
            .get_account(&request.code_address)
            .map_err(|_| ExitReason::ContractNotFound)?;
        let Some(code_hash) = account.code_hash else {
            return Ok(CallResult {
                success: false,
                return_values: Vec::new(),
                gas_left: request.gas,
            });
        };
        let code = self
            .store
            .code(&code_hash)
            .map_err(|_| ExitReason::ContractNotFound)?
            .ok_or(ExitReason::ContractNotFound)?;
        let contract = decode_contract_blob(&code).map_err(|_| ExitReason::TypeError)?;
        let Some(meta) = (if request.interface {
            contract.metadata.interfaces.get(&request.method_idx)
        } else {
            contract.metadata.methods.get(&request.method_idx)
        }) else {
            return Ok(CallResult {
                success: false,
                return_values: Vec::new(),
                gas_left: request.gas,
            });
        };
        let meta = meta.clone();
        if meta.args != request.args.len() {
            return Err(ExitReason::TypeError);
        }

        let metadata = contract.metadata.clone();
        let code = contract.code;
        let writes_before = self.writes.clone();
        let mut vm = LiteVM::new(
            Context {
                caller: request.caller,
                address: request.context_address,
                value: request.value,
                static_call: request.static_call,
                metadata: metadata.clone(),
                code,
                call_data: Vec::new(),
                return_data: Vec::new(),
            },
            request.env,
            self,
            request.gas,
        );
        vm.pc = metadata
            .jump_table
            .get(&request.method_idx)
            .copied()
            .unwrap_or(0);
        vm.stack.extend(request.args);
        let exit = vm.run();
        let gas_left = vm.gas;
        drop(vm);
        let result = match exit {
            ExitReason::Halt => Ok(CallResult {
                success: meta.rets == 0,
                return_values: Vec::new(),
                gas_left,
            }),
            ExitReason::Return(values) => {
                if values.len() != meta.rets {
                    return Err(ExitReason::TypeError);
                }
                Ok(CallResult {
                    success: true,
                    return_values: values,
                    gas_left,
                })
            }
            ExitReason::Revert(_)
            | ExitReason::OutOfGas
            | ExitReason::OutOfMemory
            | ExitReason::InvalidOpcode
            | ExitReason::StackUnderflow
            | ExitReason::OutOfBounds
            | ExitReason::TypeError
            | ExitReason::DivideByZero
            | ExitReason::StaticCallViolation
            | ExitReason::ContractNotFound => Ok(CallResult {
                success: false,
                return_values: Vec::new(),
                gas_left,
            }),
        }?;
        if !result.success {
            self.writes = writes_before;
        }
        Ok(result)
    }
}
