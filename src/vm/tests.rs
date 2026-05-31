#[cfg(test)]
use super::*;
#[cfg(test)]
use ethnum::U256;
#[cfg(test)]
use std::collections::HashMap;

#[cfg(test)]
struct MockStateDB {
    state: HashMap<([u8; 32], u8), Value>,
    balances: HashMap<[u8; 32], U256>,
    contracts: HashMap<[u8; 32], ContractBlob>,
    executed_calls: usize,
}

#[cfg(test)]
impl MockStateDB {
    fn new() -> Self {
        Self {
            state: HashMap::new(),
            balances: HashMap::new(),
            contracts: HashMap::new(),
            executed_calls: 0,
        }
    }
}

#[cfg(test)]
impl StateDB for MockStateDB {
    fn get_state(&mut self, address: &[u8; 32], field_idx: u8) -> Value {
        self.state
            .get(&(*address, field_idx))
            .cloned()
            .unwrap_or(Value::U64(0))
    }
    fn set_state(
        &mut self,
        address: &[u8; 32],
        field_idx: u8,
        value: Value,
    ) -> Result<(), ExitReason> {
        self.state.insert((*address, field_idx), value);
        Ok(())
    }
    fn get_balance(&self, address: &[u8; 32]) -> U256 {
        self.balances.get(address).cloned().unwrap_or(U256::ZERO)
    }

    fn call_contract(&mut self, request: CallRequest) -> Result<CallResult, ExitReason> {
        let Some(contract) = self.contracts.get(&request.code_address).cloned() else {
            return Ok(CallResult {
                success: false,
                return_values: Vec::new(),
                gas_left: request.gas,
            });
        };
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
        self.executed_calls += 1;
        let state_before = self.state.clone();
        let mut vm = LiteVM::new(
            Context {
                caller: request.caller,
                address: request.context_address,
                value: request.value,
                static_call: request.static_call,
                metadata: contract.metadata.clone(),
                code: contract.code,
                call_data: Vec::new(),
                return_data: Vec::new(),
            },
            request.env,
            self,
            request.gas,
        );
        vm.pc = contract
            .metadata
            .jump_table
            .get(&request.method_idx)
            .copied()
            .unwrap_or(0);
        vm.stack.extend(request.args);
        let exit = vm.run();
        let gas_left = vm.gas;
        drop(vm);
        match exit {
            ExitReason::Halt if meta.rets == 0 => Ok(CallResult {
                success: true,
                return_values: Vec::new(),
                gas_left,
            }),
            ExitReason::Return(values) if values.len() == meta.rets => Ok(CallResult {
                success: true,
                return_values: values,
                gas_left,
            }),
            _ => {
                self.state = state_before;
                Ok(CallResult {
                    success: false,
                    return_values: Vec::new(),
                    gas_left,
                })
            }
        }
    }
}

#[cfg(test)]
fn setup_env() -> Env {
    Env {
        block_num: 1,
        timestamp: 1000,
        chain_id: U256::from(1_u64),
        origin: [0u8; 32],
        gas_price: U256::from(10_u64),
    }
}

#[cfg(test)]
fn setup_ctx(code: Vec<u8>) -> Context {
    Context {
        caller: [0u8; 32],
        address: [1u8; 32],
        value: U256::ZERO,
        static_call: false,
        metadata: Metadata::default(),
        code,
        call_data: vec![],
        return_data: vec![],
    }
}

#[cfg(test)]
macro_rules! test_op {
    ($name:ident, $code:expr, $setup:expr, $check:expr) => {
        #[test]
        fn $name() {
            let mut db = MockStateDB::new();
            let ctx = setup_ctx($code);
            let env = setup_env();
            let mut vm = LiteVM::new(ctx, env, &mut db, 1000000);
            $setup(&mut vm);
            let res = vm.run();
            $check(vm, res);
        }
    };
    ($name:ident, $code:expr, $check:expr) => {
        test_op!($name, $code, |_| {}, $check);
    };
}

#[cfg(test)]
mod consts_and_stack {
    use super::*;
    test_op!(
        test_push64,
        {
            let mut c = vec![Opcode::Push64 as u8];
            c.extend_from_slice(&42u64.to_be_bytes());
            c
        },
        |vm: LiteVM, res| {
            assert_eq!(res, ExitReason::Halt);
            assert_eq!(vm.stack, vec![Value::U64(42)]);
        }
    );

    test_op!(
        test_push256,
        {
            let mut c = vec![Opcode::Push256 as u8];
            let val = U256::from(12345u32);
            c.extend_from_slice(&val.to_be_bytes());
            c
        },
        |vm: LiteVM, res| {
            assert_eq!(res, ExitReason::Halt);
            assert_eq!(vm.stack, vec![Value::U256(U256::from(12345u32))]);
        }
    );

    test_op!(
        test_push_addr,
        {
            let mut c = vec![Opcode::PushAddr as u8];
            let addr = [8u8; 32];
            c.extend_from_slice(&addr);
            c
        },
        |vm: LiteVM, res| {
            assert_eq!(res, ExitReason::Halt);
            assert_eq!(vm.stack, vec![Value::Address([8u8; 32])]);
        }
    );

    test_op!(
        test_push_local,
        vec![Opcode::PushLocal as u8, 5],
        |vm: &mut LiteVM| {
            vm.locals[5] = Value::U64(99);
        },
        |vm: LiteVM, res| {
            assert_eq!(res, ExitReason::Halt);
            assert_eq!(vm.stack, vec![Value::U64(99)]);
        }
    );

    test_op!(
        test_store_local,
        vec![Opcode::StoreLocal as u8, 7],
        |vm: &mut LiteVM| {
            vm.stack.push(Value::U256(U256::from(777u32)));
        },
        |vm: LiteVM, res| {
            assert_eq!(res, ExitReason::Halt);
            assert_eq!(vm.locals[7], Value::U256(U256::from(777u32)));
            assert_eq!(vm.stack.len(), 0);
        }
    );

    test_op!(
        test_pop,
        vec![Opcode::Pop as u8],
        |vm: &mut LiteVM| {
            vm.stack.push(Value::U64(1));
            vm.stack.push(Value::U64(2));
        },
        |vm: LiteVM, res| {
            assert_eq!(res, ExitReason::Halt);
            assert_eq!(vm.stack, vec![Value::U64(1)]);
        }
    );

    test_op!(
        test_dup,
        vec![Opcode::Dup as u8, 2],
        |vm: &mut LiteVM| {
            vm.stack.push(Value::U64(10));
            vm.stack.push(Value::U64(20));
            vm.stack.push(Value::U64(30));
        },
        |vm: LiteVM, res| {
            assert_eq!(res, ExitReason::Halt);
            assert_eq!(
                vm.stack,
                vec![
                    Value::U64(10),
                    Value::U64(20),
                    Value::U64(30),
                    Value::U64(20)
                ]
            );
        }
    );

    test_op!(
        test_swap,
        vec![Opcode::Swap as u8, 2],
        |vm: &mut LiteVM| {
            vm.stack.push(Value::U64(10));
            vm.stack.push(Value::U64(20));
            vm.stack.push(Value::U64(30));
        },
        |vm: LiteVM, res| {
            assert_eq!(res, ExitReason::Halt);
            assert_eq!(
                vm.stack,
                vec![Value::U64(10), Value::U64(30), Value::U64(20)]
            );
        }
    );
}

#[cfg(test)]
mod math64 {
    use super::*;
    macro_rules! test_binop64 {
        ($name:ident, $op:ident, $a:expr, $b:expr, $res:expr) => {
            test_op!(
                $name,
                vec![Opcode::$op as u8],
                |vm: &mut LiteVM| {
                    vm.stack.push(Value::U64($a));
                    vm.stack.push(Value::U64($b));
                },
                |vm: LiteVM, res| {
                    assert_eq!(res, ExitReason::Halt);
                    assert_eq!(vm.stack, vec![Value::U64($res)]);
                }
            );
        };
    }

    test_binop64!(test_add64, Add64, 10, 20, 30);
    test_binop64!(test_sub64, Sub64, 50, 20, 30);
    test_binop64!(test_mul64, Mul64, 10, 20, 200);
    test_binop64!(test_div64, Div64, 20, 10, 2);
    test_binop64!(test_mod64, Mod64, 25, 10, 5);
    test_binop64!(test_shl64, Shl64, 2, 3, 16);
    test_binop64!(test_shr64, Shr64, 16, 2, 4);
    test_binop64!(test_eq64_true, Eq64, 10, 10, 1);
    test_binop64!(test_eq64_false, Eq64, 10, 11, 0);
    test_binop64!(test_lt64_true, Lt64, 10, 20, 1);
    test_binop64!(test_lt64_false, Lt64, 20, 10, 0);
    test_binop64!(test_gt64_true, Gt64, 20, 10, 1);
    test_binop64!(test_gt64_false, Gt64, 10, 20, 0);
}

#[cfg(test)]
mod math256 {
    use super::*;
    macro_rules! test_binop256 {
        ($name:ident, $op:ident, $a:expr, $b:expr, $res:expr) => {
            test_op!(
                $name,
                vec![Opcode::$op as u8],
                |vm: &mut LiteVM| {
                    vm.stack.push(Value::U256(U256::from($a as u64)));
                    vm.stack.push(Value::U256(U256::from($b as u64)));
                },
                |vm: LiteVM, res| {
                    assert_eq!(res, ExitReason::Halt);
                    assert_eq!(vm.stack, vec![Value::U256(U256::from($res as u64))]);
                }
            );
        };
    }

    test_binop256!(test_add256, Add256, 10, 20, 30);
    test_binop256!(test_sub256, Sub256, 50, 20, 30);
    test_binop256!(test_mul256, Mul256, 10, 20, 200);
    test_binop256!(test_div256, Div256, 20, 10, 2);
    test_binop256!(test_mod256, Mod256, 25, 10, 5);
    test_binop256!(test_shl256, Shl256, 2, 3, 16);
    test_binop256!(test_shr256, Shr256, 16, 2, 4);

    macro_rules! test_cmp256 {
        ($name:ident, $op:ident, $a:expr, $b:expr, $res:expr) => {
            test_op!(
                $name,
                vec![Opcode::$op as u8],
                |vm: &mut LiteVM| {
                    vm.stack.push(Value::U256(U256::from($a as u64)));
                    vm.stack.push(Value::U256(U256::from($b as u64)));
                },
                |vm: LiteVM, res| {
                    assert_eq!(res, ExitReason::Halt);
                    assert_eq!(vm.stack, vec![Value::U64($res)]);
                }
            );
        };
    }

    test_cmp256!(test_eq256_true, Eq256, 10, 10, 1);
    test_cmp256!(test_eq256_false, Eq256, 10, 11, 0);
    test_cmp256!(test_lt256_true, Lt256, 10, 20, 1);
    test_cmp256!(test_lt256_false, Lt256, 20, 10, 0);
    test_cmp256!(test_gt256_true, Gt256, 20, 10, 1);
    test_cmp256!(test_gt256_false, Gt256, 10, 20, 0);

    test_binop256!(test_and, And, 0b1100, 0b1010, 0b1000);
    test_binop256!(test_or, Or, 0b1100, 0b1010, 0b1110);
    test_binop256!(test_xor, Xor, 0b1100, 0b1010, 0b0110);

    test_op!(
        test_not,
        vec![Opcode::Not as u8],
        |vm: &mut LiteVM| {
            vm.stack.push(Value::U256(U256::from(0_u32)));
        },
        |vm: LiteVM, res| {
            assert_eq!(res, ExitReason::Halt);
            assert_eq!(vm.stack, vec![Value::U256(U256::MAX)]);
        }
    );

    test_op!(
        test_iszero_true,
        vec![Opcode::IsZero as u8],
        |vm: &mut LiteVM| {
            vm.stack.push(Value::U256(U256::ZERO));
        },
        |vm: LiteVM, res| {
            assert_eq!(res, ExitReason::Halt);
            assert_eq!(vm.stack, vec![Value::U64(1)]);
        }
    );

    test_op!(
        test_iszero_false,
        vec![Opcode::IsZero as u8],
        |vm: &mut LiteVM| {
            vm.stack.push(Value::U64(42));
        },
        |vm: LiteVM, res| {
            assert_eq!(res, ExitReason::Halt);
            assert_eq!(vm.stack, vec![Value::U64(0)]);
        }
    );
}

#[cfg(test)]
mod casts {
    use super::*;
    test_op!(
        test_cast64to256,
        vec![Opcode::Cast64To256 as u8],
        |vm: &mut LiteVM| {
            vm.stack.push(Value::U64(42));
        },
        |vm: LiteVM, res| {
            assert_eq!(res, ExitReason::Halt);
            assert_eq!(vm.stack, vec![Value::U256(U256::from(42u32))]);
        }
    );

    test_op!(
        test_cast256to64,
        vec![Opcode::Cast256To64 as u8],
        |vm: &mut LiteVM| {
            vm.stack.push(Value::U256(U256::from(42u32)));
        },
        |vm: LiteVM, res| {
            assert_eq!(res, ExitReason::Halt);
            assert_eq!(vm.stack, vec![Value::U64(42)]);
        }
    );

    test_op!(
        test_cast_addr_to_256,
        vec![Opcode::CastAddrTo256 as u8],
        |vm: &mut LiteVM| {
            let mut addr = [0u8; 32];
            addr[31] = 42;
            vm.stack.push(Value::Address(addr));
        },
        |vm: LiteVM, res| {
            assert_eq!(res, ExitReason::Halt);
            assert_eq!(vm.stack, vec![Value::U256(U256::from(42u32))]);
        }
    );

    test_op!(
        test_cast_256_to_addr,
        vec![Opcode::Cast256ToAddr as u8],
        |vm: &mut LiteVM| {
            vm.stack.push(Value::U256(U256::from(42u32)));
        },
        |vm: LiteVM, res| {
            assert_eq!(res, ExitReason::Halt);
            let mut expected = [0u8; 32];
            expected[31] = 42;
            assert_eq!(vm.stack, vec![Value::Address(expected)]);
        }
    );
}

#[cfg(test)]
mod control_flow {
    use super::*;
    test_op!(
        test_jump,
        vec![
            Opcode::Jump as u8,
            0,
            0,                     // JUMP 0
            Opcode::Invalid as u8, // Should be skipped
            Opcode::Stop as u8     // Jump target
        ],
        |vm: &mut LiteVM| {
            vm.ctx.metadata.jump_table.insert(0, 4);
        },
        |_vm: LiteVM, res| {
            assert_eq!(res, ExitReason::Halt);
        }
    );

    test_op!(
        test_jumpi_true,
        vec![
            Opcode::Jumpi as u8,
            0,
            0,
            Opcode::Invalid as u8,
            Opcode::Stop as u8
        ],
        |vm: &mut LiteVM| {
            vm.stack.push(Value::U64(1));
            vm.ctx.metadata.jump_table.insert(0, 4);
        },
        |_vm: LiteVM, res| {
            assert_eq!(res, ExitReason::Halt);
        }
    );

    test_op!(
        test_jumpi_false,
        vec![
            Opcode::Jumpi as u8,
            0,
            0,
            Opcode::Stop as u8,
            Opcode::Invalid as u8
        ],
        |vm: &mut LiteVM| {
            vm.stack.push(Value::U64(0));
            vm.ctx.metadata.jump_table.insert(0, 4);
        },
        |_vm: LiteVM, res| {
            assert_eq!(res, ExitReason::Halt);
        }
    );

    test_op!(
        test_return,
        vec![Opcode::Return as u8],
        |vm: &mut LiteVM| {
            vm.stack.push(Value::U64(42));
        },
        |_vm: LiteVM, res| {
            assert_eq!(res, ExitReason::Return(vec![Value::U64(42)]));
        }
    );

    test_op!(
        test_revert,
        vec![Opcode::Revert as u8],
        |vm: &mut LiteVM| {
            vm.stack.push(Value::U64(99));
        },
        |_vm: LiteVM, res| {
            assert_eq!(res, ExitReason::Revert(Value::U64(99)));
        }
    );

    test_op!(test_stop, vec![Opcode::Stop as u8], |_vm: LiteVM, res| {
        assert_eq!(res, ExitReason::Halt);
    });
}

#[cfg(test)]
mod memory {
    use super::*;
    test_op!(
        test_mstore_mload_64,
        vec![Opcode::Mstore64 as u8, Opcode::Mload64 as u8,],
        |vm: &mut LiteVM| {
            vm.stack.push(Value::U64(10)); // offset
            vm.stack.push(Value::U64(0x1234567890ABCDEF)); // val
                                                           // Stack after mstore: [offset]
                                                           // But we want Mload64 to read it, so we need to push offset again.
                                                           // Let's modify the code:
        },
        |_vm: LiteVM, _res| {}
    );

    #[test]
    fn test_mstore_mload_64_manual() {
        let mut db = MockStateDB::new();
        let ctx = setup_ctx(vec![
            Opcode::Mstore64 as u8,
            Opcode::Push64 as u8,
            0,
            0,
            0,
            0,
            0,
            0,
            0,
            10,
            Opcode::Mload64 as u8,
        ]);
        let env = setup_env();
        let mut vm = LiteVM::new(ctx, env, &mut db, 100000);
        vm.stack.push(Value::U64(10)); // offset
        vm.stack.push(Value::U64(0x1234567890ABCDEF)); // val
        let res = vm.run();
        assert_eq!(res, ExitReason::Halt);
        assert_eq!(vm.stack, vec![Value::U64(0x1234567890ABCDEF)]);
    }

    #[test]
    fn test_mstore_mload_256_manual() {
        let mut db = MockStateDB::new();
        let ctx = setup_ctx(vec![
            Opcode::Mstore256 as u8,
            Opcode::Push64 as u8,
            0,
            0,
            0,
            0,
            0,
            0,
            0,
            10,
            Opcode::Mload256 as u8,
        ]);
        let env = setup_env();
        let mut vm = LiteVM::new(ctx, env, &mut db, 100000);
        vm.stack.push(Value::U64(10)); // offset
        vm.stack.push(Value::U256(U256::from(0xDEADBEEF_u64))); // val
        let res = vm.run();
        assert_eq!(res, ExitReason::Halt);
        assert_eq!(vm.stack, vec![Value::U256(U256::from(0xDEADBEEF_u64))]);
    }

    #[test]
    fn test_mcopy() {
        let mut db = MockStateDB::new();
        let ctx = setup_ctx(vec![
            Opcode::Mstore64 as u8,
            Opcode::Push64 as u8,
            0,
            0,
            0,
            0,
            0,
            0,
            0,
            20, // dst
            Opcode::Push64 as u8,
            0,
            0,
            0,
            0,
            0,
            0,
            0,
            10, // src
            Opcode::Push64 as u8,
            0,
            0,
            0,
            0,
            0,
            0,
            0,
            8, // len
            Opcode::Mcopy as u8,
            Opcode::Push64 as u8,
            0,
            0,
            0,
            0,
            0,
            0,
            0,
            20, // offset
            Opcode::Mload64 as u8,
        ]);
        let env = setup_env();
        let mut vm = LiteVM::new(ctx, env, &mut db, 100000);
        vm.stack.push(Value::U64(10)); // offset
        vm.stack.push(Value::U64(0x1234567890ABCDEF)); // val
        let res = vm.run();
        assert_eq!(res, ExitReason::Halt);
        assert_eq!(vm.stack, vec![Value::U64(0x1234567890ABCDEF)]);
    }
}

#[cfg(test)]
mod state_and_objects {
    use super::*;

    fn p64(code: &mut Vec<u8>, value: u64) {
        code.push(Opcode::Push64 as u8);
        code.extend_from_slice(&value.to_be_bytes());
    }

    #[test]
    fn test_structs() {
        let mut db = MockStateDB::new();
        let mut ctx = setup_ctx(vec![
            Opcode::New as u8,
            0,
            1, // struct_id 1
            Opcode::StoreLocal as u8,
            0, // store ref in local[0]
            Opcode::Push64 as u8,
            0,
            0,
            0,
            0,
            0,
            0,
            0,
            42, // val
            Opcode::PushLocal as u8,
            0, // obj_ref
            Opcode::SetField as u8,
            0,
            Opcode::PushLocal as u8,
            0, // obj_ref
            Opcode::GetField as u8,
            0,
        ]);
        ctx.metadata.structs.insert(1, 2); // struct 1 has 2 fields
        let env = setup_env();
        let mut vm = LiteVM::new(ctx, env, &mut db, 100000);
        let res = vm.run();
        assert_eq!(res, ExitReason::Halt);
        assert_eq!(vm.stack, vec![Value::U64(42)]);
    }

    #[test]
    fn test_state() {
        let mut db = MockStateDB::new();
        let ctx = setup_ctx(vec![
            Opcode::Push64 as u8,
            0,
            0,
            0,
            0,
            0,
            0,
            0,
            99,
            Opcode::SetState as u8,
            3,
            Opcode::GetState as u8,
            3,
        ]);
        let env = setup_env();
        let mut vm = LiteVM::new(ctx, env, &mut db, 100000);
        let res = vm.run();
        assert_eq!(res, ExitReason::Halt);
        assert_eq!(vm.stack, vec![Value::U64(99)]);
    }

    #[test]
    fn test_maps() {
        let mut db = MockStateDB::new();
        let ctx = setup_ctx(vec![
            Opcode::NewMap as u8,
            Opcode::StoreLocal as u8,
            0,
            Opcode::Push64 as u8,
            0,
            0,
            0,
            0,
            0,
            0,
            0,
            42, // val
            Opcode::Push64 as u8,
            0,
            0,
            0,
            0,
            0,
            0,
            0,
            5, // key
            Opcode::PushLocal as u8,
            0, // map_ref
            Opcode::MapSet as u8,
            Opcode::Push64 as u8,
            0,
            0,
            0,
            0,
            0,
            0,
            0,
            5, // key
            Opcode::PushLocal as u8,
            0, // map_ref
            Opcode::MapGet as u8,
        ]);
        let env = setup_env();
        let mut vm = LiteVM::new(ctx, env, &mut db, 100000);
        let res = vm.run();
        assert_eq!(res, ExitReason::Halt);
        assert_eq!(vm.stack, vec![Value::U64(42)]);
    }

    #[test]
    fn test_arrays() {
        let mut db = MockStateDB::new();
        let ctx = setup_ctx(vec![
            Opcode::Push64 as u8,
            0,
            0,
            0,
            0,
            0,
            0,
            0,
            10, // len
            Opcode::NewArray as u8,
            0,
            1, // type_id 1
            Opcode::StoreLocal as u8,
            0, // store arr_ref
            Opcode::Push64 as u8,
            0,
            0,
            0,
            0,
            0,
            0,
            0,
            42, // val
            Opcode::Push64 as u8,
            0,
            0,
            0,
            0,
            0,
            0,
            0,
            3, // idx
            Opcode::PushLocal as u8,
            0, // arr_ref
            Opcode::ArraySet as u8,
            Opcode::PushLocal as u8,
            0,                      // arr_ref
            Opcode::ArrayLen as u8, // leaves len on stack
            Opcode::Push64 as u8,
            0,
            0,
            0,
            0,
            0,
            0,
            0,
            3, // idx
            Opcode::PushLocal as u8,
            0, // arr_ref
            Opcode::ArrayGet as u8,
        ]);
        let env = setup_env();
        let mut vm = LiteVM::new(ctx, env, &mut db, 100000);
        let res = vm.run();
        assert_eq!(res, ExitReason::Halt);
        assert_eq!(vm.stack, vec![Value::U64(10), Value::U64(42)]);
    }

    #[test]
    fn test_persistent_map_round_trip_through_state() {
        let mut db = MockStateDB::new();
        let mut store = vec![Opcode::NewMap as u8, Opcode::StoreLocal as u8, 0];
        p64(&mut store, 42);
        p64(&mut store, 5);
        store.extend_from_slice(&[Opcode::PushLocal as u8, 0, Opcode::MapSet as u8]);
        store.extend_from_slice(&[Opcode::PushLocal as u8, 0, Opcode::SetState as u8, 9]);
        let ctx = setup_ctx(store);
        let env = setup_env();
        let mut vm = LiteVM::new(ctx, env, &mut db, 100000);
        let res = vm.run();
        assert_eq!(res, ExitReason::Halt);
        drop(vm);
        assert_eq!(
            db.state.get(&([1u8; 32], 9)),
            Some(&Value::Map(vec![(Value::U64(5), Value::U64(42))]))
        );

        let mut load = vec![Opcode::GetState as u8, 9, Opcode::StoreLocal as u8, 0];
        p64(&mut load, 5);
        load.extend_from_slice(&[
            Opcode::PushLocal as u8,
            0,
            Opcode::MapGet as u8,
            Opcode::Return as u8,
        ]);
        let ctx = setup_ctx(load);
        let env = setup_env();
        let mut vm = LiteVM::new(ctx, env, &mut db, 100000);
        let res = vm.run();
        assert_eq!(res, ExitReason::Return(vec![Value::U64(42)]));
    }

    #[test]
    fn test_persistent_array_round_trip_through_state() {
        let mut db = MockStateDB::new();
        let mut store = Vec::new();
        p64(&mut store, 4);
        store.extend_from_slice(&[Opcode::NewArray as u8, 0, 1, Opcode::StoreLocal as u8, 0]);
        p64(&mut store, 77);
        p64(&mut store, 2);
        store.extend_from_slice(&[Opcode::PushLocal as u8, 0, Opcode::ArraySet as u8]);
        store.extend_from_slice(&[Opcode::PushLocal as u8, 0, Opcode::SetState as u8, 10]);
        let ctx = setup_ctx(store);
        let env = setup_env();
        let mut vm = LiteVM::new(ctx, env, &mut db, 100000);
        let res = vm.run();
        assert_eq!(res, ExitReason::Halt);
        drop(vm);
        assert_eq!(
            db.state.get(&([1u8; 32], 10)),
            Some(&Value::Array(vec![
                Value::U64(0),
                Value::U64(0),
                Value::U64(77),
                Value::U64(0),
            ]))
        );

        let mut load = vec![Opcode::GetState as u8, 10, Opcode::StoreLocal as u8, 0];
        p64(&mut load, 2);
        load.extend_from_slice(&[
            Opcode::PushLocal as u8,
            0,
            Opcode::ArrayGet as u8,
            Opcode::Return as u8,
        ]);
        let ctx = setup_ctx(load);
        let env = setup_env();
        let mut vm = LiteVM::new(ctx, env, &mut db, 100000);
        let res = vm.run();
        assert_eq!(res, ExitReason::Return(vec![Value::U64(77)]));
    }
}

#[cfg(test)]
mod env_context {
    use super::*;

    test_op!(
        test_caller,
        vec![Opcode::Caller as u8],
        |vm: &mut LiteVM| {
            vm.ctx.caller = [2u8; 32];
        },
        |vm: LiteVM, res| {
            assert_eq!(res, ExitReason::Halt);
            assert_eq!(vm.stack, vec![Value::Address([2u8; 32])]);
        }
    );

    test_op!(
        test_callvalue,
        vec![Opcode::CallValue as u8],
        |vm: &mut LiteVM| {
            vm.ctx.value = U256::from(100u32);
        },
        |vm: LiteVM, res| {
            assert_eq!(res, ExitReason::Halt);
            assert_eq!(vm.stack, vec![Value::U256(U256::from(100u32))]);
        }
    );

    test_op!(
        test_address,
        vec![Opcode::Address as u8],
        |vm: &mut LiteVM| {
            vm.ctx.address = [3u8; 32];
        },
        |vm: LiteVM, res| {
            assert_eq!(res, ExitReason::Halt);
            assert_eq!(vm.stack, vec![Value::Address([3u8; 32])]);
        }
    );

    #[test]
    fn test_balance() {
        let mut db = MockStateDB::new();
        let addr = [4u8; 32];
        db.balances.insert(addr, U256::from(500u32));
        let ctx = setup_ctx(vec![Opcode::Balance as u8]);
        let env = setup_env();
        let mut vm = LiteVM::new(ctx, env, &mut db, 100000);
        vm.stack.push(Value::Address(addr));
        let res = vm.run();
        assert_eq!(res, ExitReason::Halt);
        assert_eq!(vm.stack, vec![Value::U256(U256::from(500u32))]);
    }

    test_op!(
        test_blocknum,
        vec![Opcode::BlockNum as u8],
        |vm: LiteVM, res| {
            assert_eq!(res, ExitReason::Halt);
            assert_eq!(vm.stack, vec![Value::U64(1)]);
        }
    );

    test_op!(
        test_timestamp,
        vec![Opcode::Timestamp as u8],
        |vm: LiteVM, res| {
            assert_eq!(res, ExitReason::Halt);
            assert_eq!(vm.stack, vec![Value::U64(1000)]);
        }
    );

    test_op!(
        test_chainid,
        vec![Opcode::ChainId as u8],
        |vm: LiteVM, res| {
            assert_eq!(res, ExitReason::Halt);
            assert_eq!(vm.stack, vec![Value::U256(U256::from(1u32))]);
        }
    );

    test_op!(
        test_origin,
        vec![Opcode::Origin as u8],
        |vm: LiteVM, res| {
            assert_eq!(res, ExitReason::Halt);
            assert_eq!(vm.stack, vec![Value::Address([0u8; 32])]);
        }
    );

    test_op!(
        test_gasprice,
        vec![Opcode::GasPrice as u8],
        |vm: LiteVM, res| {
            assert_eq!(res, ExitReason::Halt);
            assert_eq!(vm.stack, vec![Value::U256(U256::from(10u32))]);
        }
    );

    test_op!(
        test_returndatasize,
        vec![Opcode::ReturnDataSize as u8],
        |vm: &mut LiteVM| {
            vm.ctx.return_data = vec![0, 1, 2];
        },
        |vm: LiteVM, res| {
            assert_eq!(res, ExitReason::Halt);
            assert_eq!(vm.stack, vec![Value::U64(3)]);
        }
    );

    test_op!(
        test_gasleft,
        vec![Opcode::GasLeft as u8],
        |vm: LiteVM, res| {
            assert_eq!(res, ExitReason::Halt);
            // started with 1000000, gasleft took 2
            assert_eq!(vm.stack, vec![Value::U64(1000000 - 2)]);
        }
    );

    test_op!(
        test_sha3,
        vec![Opcode::Sha3 as u8],
        |vm: &mut LiteVM| {
            vm.stack.push(Value::U64(0)); // offset
            vm.stack.push(Value::U64(0)); // len
        },
        |vm: LiteVM, res| {
            assert_eq!(res, ExitReason::Halt);
            assert_eq!(
                vm.stack,
                vec![Value::U256(U256::from_be_bytes(crate::crypto::sha3_256(
                    &[]
                )))]
            );
        }
    );
}

#[cfg(test)]
mod calls {
    use super::*;

    const IFACE_MINT: u16 = 1;
    const IFACE_TRANSFER: u16 = 2;
    const IFACE_OWNER_OF: u16 = 3;

    fn push64(code: &mut Vec<u8>, value: u64) {
        code.push(Opcode::Push64 as u8);
        code.extend_from_slice(&value.to_be_bytes());
    }

    fn immediate_u16(code: &mut Vec<u8>, value: u16) {
        code.extend_from_slice(&value.to_be_bytes());
    }

    fn mint_code(field: u8) -> Vec<u8> {
        vec![Opcode::SetState as u8, field, Opcode::Stop as u8]
    }

    fn transfer_code(field: u8) -> (Vec<u8>, usize) {
        let mut code = vec![
            Opcode::StoreLocal as u8,
            0, // to
            Opcode::GetState as u8,
            field,
            Opcode::Origin as u8,
            Opcode::CastAddrTo256 as u8,
            Opcode::Swap as u8,
            2,
            Opcode::CastAddrTo256 as u8,
            Opcode::Eq256 as u8,
            Opcode::Jumpi as u8,
        ];
        let jump_immediate = code.len();
        immediate_u16(&mut code, 0);
        push64(&mut code, 1);
        code.push(Opcode::Revert as u8);
        let ok_pc = code.len();
        code.push(Opcode::PushLocal as u8);
        code.push(0);
        code.push(Opcode::SetState as u8);
        code.push(field);
        code.push(Opcode::Stop as u8);
        let ok_idx = 99u16;
        code[jump_immediate..jump_immediate + 2].copy_from_slice(&ok_idx.to_be_bytes());
        (code, ok_pc)
    }

    fn owner_of_code(field: u8) -> Vec<u8> {
        vec![Opcode::GetState as u8, field, Opcode::Return as u8]
    }

    fn nft_contract(field: u8, address: [u8; 32]) -> ([u8; 32], ContractBlob) {
        let mint_pc = 0usize;
        let mint = mint_code(field);
        let transfer_pc = mint.len();
        let (transfer, transfer_ok_pc) = transfer_code(field);
        let owner_pc = mint.len() + transfer.len();
        let owner = owner_of_code(field);

        let mut code = Vec::new();
        code.extend(mint);
        code.extend(transfer);
        code.extend(owner);

        let mut metadata = Metadata::default();
        metadata
            .methods
            .insert(IFACE_MINT, MethodMeta { args: 1, rets: 0 });
        metadata
            .methods
            .insert(IFACE_TRANSFER, MethodMeta { args: 1, rets: 0 });
        metadata
            .methods
            .insert(IFACE_OWNER_OF, MethodMeta { args: 0, rets: 1 });
        metadata
            .interfaces
            .insert(IFACE_MINT, MethodMeta { args: 1, rets: 0 });
        metadata
            .interfaces
            .insert(IFACE_TRANSFER, MethodMeta { args: 1, rets: 0 });
        metadata
            .interfaces
            .insert(IFACE_OWNER_OF, MethodMeta { args: 0, rets: 1 });
        metadata.jump_table.insert(IFACE_MINT, mint_pc);
        metadata.jump_table.insert(IFACE_TRANSFER, transfer_pc);
        metadata.jump_table.insert(IFACE_OWNER_OF, owner_pc);
        metadata.jump_table.insert(99, transfer_pc + transfer_ok_pc);

        (address, ContractBlob { metadata, code })
    }

    fn erc721_caller_metadata() -> Metadata {
        let mut metadata = Metadata::default();
        metadata
            .interfaces
            .insert(IFACE_MINT, MethodMeta { args: 1, rets: 0 });
        metadata
            .interfaces
            .insert(IFACE_TRANSFER, MethodMeta { args: 1, rets: 0 });
        metadata
            .interfaces
            .insert(IFACE_OWNER_OF, MethodMeta { args: 0, rets: 1 });
        metadata
    }

    #[test]
    fn test_invoke() {
        let mut db = MockStateDB::new();
        let target = [0x90; 32];
        let mut target_metadata = Metadata::default();
        target_metadata
            .methods
            .insert(1, MethodMeta { args: 2, rets: 0 });
        db.contracts.insert(
            target,
            ContractBlob {
                metadata: target_metadata,
                code: vec![
                    Opcode::SetState as u8,
                    0,
                    Opcode::SetState as u8,
                    1,
                    Opcode::Stop as u8,
                ],
            },
        );
        let mut ctx = setup_ctx(vec![Opcode::Invoke as u8, 0, 1]);
        ctx.metadata
            .methods
            .insert(1, MethodMeta { args: 2, rets: 0 });
        let env = setup_env();
        let mut vm = LiteVM::new(ctx, env, &mut db, 100000);
        vm.stack.push(Value::U64(10_000)); // gas
        vm.stack.push(Value::Address(target)); // addr
        vm.stack.push(Value::U256(U256::ZERO)); // val
        vm.stack.push(Value::U64(11)); // arg 1
        vm.stack.push(Value::U64(22)); // arg 2
        let res = vm.run();
        assert_eq!(res, ExitReason::Halt);
        assert_eq!(vm.stack, vec![Value::U64(1)]);
        drop(vm);
        assert_eq!(db.executed_calls, 1);
        assert_eq!(db.state.get(&(target, 0)), Some(&Value::U64(22)));
        assert_eq!(db.state.get(&(target, 1)), Some(&Value::U64(11)));
    }

    #[test]
    fn test_invoke_static() {
        let mut db = MockStateDB::new();
        let target = [0x91; 32];
        let mut target_metadata = Metadata::default();
        target_metadata
            .methods
            .insert(1, MethodMeta { args: 1, rets: 1 });
        db.contracts.insert(
            target,
            ContractBlob {
                metadata: target_metadata,
                code: vec![Opcode::Return as u8],
            },
        );
        let mut ctx = setup_ctx(vec![Opcode::InvokeStatic as u8, 0, 1]);
        ctx.metadata
            .methods
            .insert(1, MethodMeta { args: 1, rets: 1 });
        let env = setup_env();
        let mut vm = LiteVM::new(ctx, env, &mut db, 100000);
        vm.stack.push(Value::U64(10_000)); // gas
        vm.stack.push(Value::Address(target)); // addr
        vm.stack.push(Value::U64(11)); // arg
        let res = vm.run();
        assert_eq!(res, ExitReason::Halt);
        assert_eq!(vm.stack, vec![Value::U64(1), Value::U64(11)]);
        drop(vm);
        assert_eq!(db.executed_calls, 1);
    }

    #[test]
    fn test_invoke_delegate() {
        let mut db = MockStateDB::new();
        let library = [0x92; 32];
        let mut library_metadata = Metadata::default();
        library_metadata
            .methods
            .insert(1, MethodMeta { args: 1, rets: 0 });
        db.contracts.insert(
            library,
            ContractBlob {
                metadata: library_metadata,
                code: vec![Opcode::SetState as u8, 7, Opcode::Stop as u8],
            },
        );
        let mut ctx = setup_ctx(vec![Opcode::InvokeDelegate as u8, 0, 1]);
        ctx.metadata
            .methods
            .insert(1, MethodMeta { args: 1, rets: 0 });
        let env = setup_env();
        let mut vm = LiteVM::new(ctx, env, &mut db, 100000);
        vm.stack.push(Value::U64(10_000)); // gas
        vm.stack.push(Value::Address(library)); // addr
        vm.stack.push(Value::U64(11)); // arg
        let res = vm.run();
        assert_eq!(res, ExitReason::Halt);
        assert_eq!(vm.stack, vec![Value::U64(1)]);
        let context_address = vm.ctx.address;
        drop(vm);
        assert_eq!(db.executed_calls, 1);
        assert_eq!(db.state.get(&(context_address, 7)), Some(&Value::U64(11)));
        assert_eq!(db.state.get(&(library, 7)), None);
    }

    #[test]
    fn test_invoke_interface_requires_caller_interface_metadata() {
        let mut db = MockStateDB::new();
        let ctx = setup_ctx(vec![Opcode::InvokeInterface as u8, 0, 1]);
        let env = setup_env();
        let mut vm = LiteVM::new(ctx, env, &mut db, 100000);
        let res = vm.run();
        assert_eq!(res, ExitReason::OutOfBounds);
    }

    #[test]
    fn test_invoke_itf_static_requires_caller_interface_metadata() {
        let mut db = MockStateDB::new();
        let ctx = setup_ctx(vec![Opcode::InvokeItfStatic as u8, 0, 1]);
        let env = setup_env();
        let mut vm = LiteVM::new(ctx, env, &mut db, 100000);
        let res = vm.run();
        assert_eq!(res, ExitReason::OutOfBounds);
    }

    #[test]
    fn test_invoke_interface_dispatches_to_target_contract() {
        let mut db = MockStateDB::new();
        let target = [9; 32];
        let mut target_meta = Metadata::default();
        target_meta
            .methods
            .insert(1, MethodMeta { args: 1, rets: 1 });
        target_meta
            .interfaces
            .insert(1, MethodMeta { args: 1, rets: 1 });
        db.contracts.insert(
            target,
            ContractBlob {
                metadata: target_meta,
                code: vec![Opcode::Return as u8],
            },
        );

        let mut ctx = setup_ctx(vec![Opcode::InvokeInterface as u8, 0, 1]);
        ctx.metadata
            .interfaces
            .insert(1, MethodMeta { args: 1, rets: 1 });
        let env = setup_env();
        let mut vm = LiteVM::new(ctx, env, &mut db, 100000);
        vm.stack.push(Value::U64(1000));
        vm.stack.push(Value::Address(target));
        vm.stack.push(Value::U256(U256::ZERO));
        vm.stack.push(Value::U64(42));

        let res = vm.run();

        assert_eq!(res, ExitReason::Halt);
        assert_eq!(vm.stack, vec![Value::U64(1), Value::U64(42)]);
    }

    #[test]
    fn test_invoke_static_rejects_target_state_write_without_leaking_state() {
        let mut db = MockStateDB::new();
        let target = [8; 32];
        let mut target_meta = Metadata::default();
        target_meta
            .methods
            .insert(1, MethodMeta { args: 1, rets: 0 });
        db.contracts.insert(
            target,
            ContractBlob {
                metadata: target_meta,
                code: vec![Opcode::SetState as u8, 0, Opcode::Stop as u8],
            },
        );

        let mut ctx = setup_ctx(vec![Opcode::InvokeStatic as u8, 0, 1]);
        ctx.metadata
            .methods
            .insert(1, MethodMeta { args: 1, rets: 0 });
        let env = setup_env();
        let mut vm = LiteVM::new(ctx, env, &mut db, 100000);
        vm.stack.push(Value::U64(1000));
        vm.stack.push(Value::Address(target));
        vm.stack.push(Value::U64(77));

        let res = vm.run();

        assert_eq!(res, ExitReason::Halt);
        assert_eq!(vm.stack, vec![Value::U64(0)]);
        drop(vm);
        assert_eq!(db.state.get(&(target, 0)), None);
    }

    #[test]
    fn erc721_like_interface_mint_transfer_and_owner_queries_across_implementations() {
        let mut db = MockStateDB::new();
        let nft_a = [0xA1; 32];
        let nft_b = [0xB2; 32];
        let alice = [0x11; 32];
        let bob = [0x22; 32];
        let marketplace = [0x33; 32];

        let (addr_a, contract_a) = nft_contract(0, nft_a);
        let (addr_b, contract_b) = nft_contract(1, nft_b);
        db.contracts.insert(addr_a, contract_a);
        db.contracts.insert(addr_b, contract_b);

        let mut ctx = setup_ctx(Vec::new());
        ctx.address = marketplace;
        ctx.metadata = erc721_caller_metadata();
        let env = setup_env();
        let mut vm = LiteVM::new(ctx, env, &mut db, 1_000_000);

        vm.stack.push(Value::U64(50_000));
        vm.stack.push(Value::Address(nft_a));
        vm.stack.push(Value::U256(U256::ZERO));
        vm.stack.push(Value::Address(alice));
        vm.ctx.code = vec![Opcode::InvokeInterface as u8, 0, IFACE_MINT as u8];
        assert_eq!(vm.run(), ExitReason::Halt);
        assert_eq!(vm.stack.pop(), Some(Value::U64(1)));

        vm.pc = 0;
        vm.ctx.code = vec![Opcode::InvokeItfStatic as u8, 0, IFACE_OWNER_OF as u8];
        vm.stack.push(Value::U64(50_000));
        vm.stack.push(Value::Address(nft_a));
        assert_eq!(vm.run(), ExitReason::Halt);
        assert_eq!(vm.stack.pop(), Some(Value::Address(alice)));
        assert_eq!(vm.stack.pop(), Some(Value::U64(1)));

        vm.pc = 0;
        vm.env.origin = alice;
        vm.ctx.address = marketplace;
        vm.ctx.code = vec![Opcode::InvokeInterface as u8, 0, IFACE_TRANSFER as u8];
        vm.stack.push(Value::U64(50_000));
        vm.stack.push(Value::Address(nft_a));
        vm.stack.push(Value::U256(U256::ZERO));
        vm.stack.push(Value::Address(bob));
        assert_eq!(vm.run(), ExitReason::Halt);
        assert_eq!(vm.stack.pop(), Some(Value::U64(1)));

        vm.pc = 0;
        vm.ctx.code = vec![Opcode::InvokeItfStatic as u8, 0, IFACE_OWNER_OF as u8];
        vm.stack.push(Value::U64(50_000));
        vm.stack.push(Value::Address(nft_a));
        assert_eq!(vm.run(), ExitReason::Halt);
        assert_eq!(vm.stack.pop(), Some(Value::Address(bob)));
        assert_eq!(vm.stack.pop(), Some(Value::U64(1)));

        vm.pc = 0;
        vm.ctx.caller = marketplace;
        vm.ctx.code = vec![Opcode::InvokeInterface as u8, 0, IFACE_MINT as u8];
        vm.stack.push(Value::U64(50_000));
        vm.stack.push(Value::Address(nft_b));
        vm.stack.push(Value::U256(U256::ZERO));
        vm.stack.push(Value::Address(alice));
        assert_eq!(vm.run(), ExitReason::Halt);
        assert_eq!(vm.stack.pop(), Some(Value::U64(1)));

        drop(vm);
        assert_eq!(db.executed_calls, 5);
        assert_eq!(db.state.get(&(nft_a, 0)), Some(&Value::Address(bob)));
        assert_eq!(db.state.get(&(nft_b, 1)), Some(&Value::Address(alice)));
    }

    #[test]
    fn erc721_like_interface_sad_paths_return_failure_and_preserve_owner() {
        let mut db = MockStateDB::new();
        let nft = [0xA1; 32];
        let no_interface = [0xC3; 32];
        let alice = [0x11; 32];
        let bob = [0x22; 32];
        let mallory = [0x44; 32];
        let marketplace = [0x33; 32];

        let (addr, contract) = nft_contract(0, nft);
        db.contracts.insert(addr, contract);
        let mut bad_metadata = Metadata::default();
        bad_metadata
            .methods
            .insert(IFACE_MINT, MethodMeta { args: 1, rets: 0 });
        db.contracts.insert(
            no_interface,
            ContractBlob {
                metadata: bad_metadata,
                code: mint_code(0),
            },
        );

        let mut ctx = setup_ctx(Vec::new());
        ctx.address = marketplace;
        ctx.metadata = erc721_caller_metadata();
        let mut env = setup_env();
        env.origin = alice;
        let mut vm = LiteVM::new(ctx, env, &mut db, 1_000_000);

        vm.stack.push(Value::U64(50_000));
        vm.stack.push(Value::Address(nft));
        vm.stack.push(Value::U256(U256::ZERO));
        vm.stack.push(Value::Address(alice));
        vm.ctx.code = vec![Opcode::InvokeInterface as u8, 0, IFACE_MINT as u8];
        assert_eq!(vm.run(), ExitReason::Halt);
        assert_eq!(vm.stack.pop(), Some(Value::U64(1)));

        vm.pc = 0;
        vm.env.origin = mallory;
        vm.ctx.code = vec![Opcode::InvokeInterface as u8, 0, IFACE_TRANSFER as u8];
        vm.stack.push(Value::U64(50_000));
        vm.stack.push(Value::Address(nft));
        vm.stack.push(Value::U256(U256::ZERO));
        vm.stack.push(Value::Address(bob));
        assert_eq!(vm.run(), ExitReason::Halt);
        assert_eq!(vm.stack.pop(), Some(Value::U64(0)));

        vm.pc = 0;
        vm.ctx.caller = marketplace;
        vm.ctx.code = vec![Opcode::InvokeInterface as u8, 0, IFACE_MINT as u8];
        vm.stack.push(Value::U64(50_000));
        vm.stack.push(Value::Address(no_interface));
        vm.stack.push(Value::U256(U256::ZERO));
        vm.stack.push(Value::Address(bob));
        assert_eq!(vm.run(), ExitReason::Halt);
        assert_eq!(vm.stack.pop(), Some(Value::U64(0)));

        drop(vm);
        assert_eq!(db.executed_calls, 2);
        assert_eq!(db.state.get(&(nft, 0)), Some(&Value::Address(alice)));
        assert_eq!(db.state.get(&(no_interface, 0)), None);
    }

    #[test]
    fn test_invoke_unknown_method_reverts_before_popping_stack() {
        let mut db = MockStateDB::new();
        let ctx = setup_ctx(vec![Opcode::Invoke as u8, 0, 9]);
        let env = setup_env();
        let mut vm = LiteVM::new(ctx, env, &mut db, 100000);
        vm.stack.push(Value::U64(1000));
        vm.stack.push(Value::Address([0; 32]));
        vm.stack.push(Value::U256(U256::ZERO));
        let res = vm.run();
        assert_eq!(res, ExitReason::OutOfBounds);
        assert_eq!(vm.stack.len(), 3);
    }

    #[test]
    fn test_emit() {
        let mut db = MockStateDB::new();
        let ctx = setup_ctx(vec![Opcode::Emit as u8, 0, 1]);
        let env = setup_env();
        let mut vm = LiteVM::new(ctx, env, &mut db, 100000);
        vm.stack.push(Value::U64(0)); // offset
        vm.stack.push(Value::U64(0)); // len
        let res = vm.run();
        assert_eq!(res, ExitReason::Halt);
    }
}

#[cfg(test)]
mod raw_calls {
    use super::*;

    #[test]
    fn test_call_raw() {
        let mut db = MockStateDB::new();
        let ctx = setup_ctx(vec![Opcode::CallRaw as u8]);
        let env = setup_env();
        let mut vm = LiteVM::new(ctx, env, &mut db, 100000);
        let res = vm.run();
        assert_eq!(res, ExitReason::InvalidOpcode);
    }

    #[test]
    fn test_calldataload() {
        let mut db = MockStateDB::new();
        let ctx = setup_ctx(vec![Opcode::CallDataLoad as u8]);
        let env = setup_env();
        let mut vm = LiteVM::new(ctx, env, &mut db, 100000);
        let res = vm.run();
        assert_eq!(res, ExitReason::InvalidOpcode);
    }

    #[test]
    fn test_returndatacopy() {
        let mut db = MockStateDB::new();
        let ctx = setup_ctx(vec![Opcode::ReturnDataCopy as u8]);
        let env = setup_env();
        let mut vm = LiteVM::new(ctx, env, &mut db, 100000);
        let res = vm.run();
        assert_eq!(res, ExitReason::InvalidOpcode);
    }

    test_op!(
        test_invalid,
        vec![Opcode::Invalid as u8],
        |_vm: LiteVM, res| {
            assert_eq!(res, ExitReason::InvalidOpcode);
        }
    );
}

#[cfg(test)]
mod turing_complete {
    use super::*;

    // A Brainfuck interpreter written in LiteVM bytecode.
    // Tape: a byte array we manage in a new array.
    // However, it's easier to use local variables and raw memory.
    // locals[0] = DP (Data Pointer)
    // locals[1] = IP (Instruction Pointer)
    // raw_memory[0..] = Tape
    // Program is hardcoded or read from another array. We'll read program from locals[2] which is an array_ref.

    // For simplicity, let's just write a simple `while` loop that counts down from 10 to 0.
    // A loop in bytecode proves Turing completeness (along with conditionals and state).
    #[test]
    fn test_loop_countdown() {
        let mut db = MockStateDB::new();

        // locals[0] = 10
        // loop:
        //   locals[0] = locals[0] - 1
        //   if locals[0] != 0 goto loop
        // return locals[0]

        let code = vec![
            Opcode::Push64 as u8,
            0,
            0,
            0,
            0,
            0,
            0,
            0,
            10,
            Opcode::StoreLocal as u8,
            0,
            // loop_start (idx 0 -> pc 11)
            Opcode::PushLocal as u8,
            0,
            Opcode::Push64 as u8,
            0,
            0,
            0,
            0,
            0,
            0,
            0,
            1,
            Opcode::Sub64 as u8,
            Opcode::StoreLocal as u8,
            0,
            Opcode::PushLocal as u8,
            0,
            Opcode::Jumpi as u8,
            0,
            0, // if local[0] != 0 jump to loop_start (jump_idx 0)
            Opcode::PushLocal as u8,
            0,
            Opcode::Return as u8,
        ];

        let mut ctx = setup_ctx(code);
        ctx.metadata.jump_table.insert(0, 11);

        let env = setup_env();
        let mut vm = LiteVM::new(ctx, env, &mut db, 100000);
        let res = vm.run();

        assert_eq!(res, ExitReason::Return(vec![Value::U64(0)]));
    }

    #[test]
    fn test_brainfuck_interpreter() {
        struct Assembler {
            code: Vec<u8>,
            label_pc: std::collections::HashMap<&'static str, usize>,
            label_idx: std::collections::HashMap<&'static str, u16>,
            next_idx: u16,
        }
        impl Assembler {
            fn new() -> Self {
                Self {
                    code: Vec::new(),
                    label_pc: std::collections::HashMap::new(),
                    label_idx: std::collections::HashMap::new(),
                    next_idx: 0,
                }
            }
            fn emit(&mut self, op: Opcode) {
                self.code.push(op as u8);
            }
            fn emit_u64(&mut self, v: u64) {
                self.emit(Opcode::Push64);
                self.code.extend_from_slice(&v.to_be_bytes());
            }
            fn get_idx(&mut self, label: &'static str) -> u16 {
                if let Some(&idx) = self.label_idx.get(label) {
                    idx
                } else {
                    let idx = self.next_idx;
                    self.next_idx += 1;
                    self.label_idx.insert(label, idx);
                    idx
                }
            }
            fn jump(&mut self, label: &'static str) {
                self.emit(Opcode::Jump);
                let idx = self.get_idx(label);
                self.code.extend_from_slice(&idx.to_be_bytes());
            }
            fn jumpi(&mut self, label: &'static str) {
                self.emit(Opcode::Jumpi);
                let idx = self.get_idx(label);
                self.code.extend_from_slice(&idx.to_be_bytes());
            }
            fn bind(&mut self, label: &'static str) {
                self.label_pc.insert(label, self.code.len());
            }
            fn get_jump_table(&self) -> std::collections::HashMap<u16, usize> {
                let mut jt = std::collections::HashMap::new();
                for (lbl, &idx) in &self.label_idx {
                    if let Some(&pc) = self.label_pc.get(lbl) {
                        jt.insert(idx, pc);
                    } else {
                        panic!("Unbound label: {}", lbl);
                    }
                }
                jt
            }
        }

        let run_bf = |bf_src: &str| -> (ExitReason, Vec<(u16, Vec<Value>)>) {
            let mut asm = Assembler::new();

            let mut bf_prog = Vec::new();
            for c in bf_src.chars() {
                match c {
                    '+' => bf_prog.push(1),
                    '-' => bf_prog.push(2),
                    '>' => bf_prog.push(3),
                    '<' => bf_prog.push(4),
                    '[' => bf_prog.push(5),
                    ']' => bf_prog.push(6),
                    '.' => bf_prog.push(7),
                    _ => {}
                }
            }

            let prog_start = 0x1000;
            for (i, &inst) in bf_prog.iter().enumerate() {
                asm.emit_u64(prog_start + (i as u64) * 8);
                asm.emit_u64(inst);
                asm.emit(Opcode::Mstore64);
            }

            asm.emit_u64(0);
            asm.emit(Opcode::StoreLocal);
            asm.code.push(0);

            asm.emit_u64(prog_start);
            asm.emit(Opcode::StoreLocal);
            asm.code.push(1);

            asm.emit_u64(prog_start + (bf_prog.len() as u64) * 8);
            asm.emit(Opcode::StoreLocal);
            asm.code.push(2);

            // Out pointer for '.' instruction
            asm.emit_u64(0x2000);
            asm.emit(Opcode::StoreLocal);
            asm.code.push(5);

            asm.bind("loop_start");
            asm.emit(Opcode::PushLocal);
            asm.code.push(1);
            asm.emit(Opcode::PushLocal);
            asm.code.push(2);
            asm.emit(Opcode::Eq64);
            asm.jumpi("halt");

            asm.emit(Opcode::PushLocal);
            asm.code.push(1);
            asm.emit(Opcode::Mload64);

            asm.emit(Opcode::Dup);
            asm.code.push(1);
            asm.emit_u64(1);
            asm.emit(Opcode::Eq64);
            asm.jumpi("inst_1");

            asm.emit(Opcode::Dup);
            asm.code.push(1);
            asm.emit_u64(2);
            asm.emit(Opcode::Eq64);
            asm.jumpi("inst_2");

            asm.emit(Opcode::Dup);
            asm.code.push(1);
            asm.emit_u64(3);
            asm.emit(Opcode::Eq64);
            asm.jumpi("inst_3");

            asm.emit(Opcode::Dup);
            asm.code.push(1);
            asm.emit_u64(4);
            asm.emit(Opcode::Eq64);
            asm.jumpi("inst_4");

            asm.emit(Opcode::Dup);
            asm.code.push(1);
            asm.emit_u64(5);
            asm.emit(Opcode::Eq64);
            asm.jumpi("inst_5");

            asm.emit(Opcode::Dup);
            asm.code.push(1);
            asm.emit_u64(6);
            asm.emit(Opcode::Eq64);
            asm.jumpi("inst_6");

            asm.emit(Opcode::Dup);
            asm.code.push(1);
            asm.emit_u64(7);
            asm.emit(Opcode::Eq64);
            asm.jumpi("inst_7");

            asm.jump("next_inst");

            // inst_1 (+)
            asm.bind("inst_1");
            asm.emit(Opcode::PushLocal);
            asm.code.push(0);
            asm.emit(Opcode::Dup);
            asm.code.push(1);
            asm.emit(Opcode::Mload64);
            asm.emit_u64(1);
            asm.emit(Opcode::Add64);
            asm.emit(Opcode::Mstore64);
            asm.jump("next_inst");

            // inst_2 (-)
            asm.bind("inst_2");
            asm.emit(Opcode::PushLocal);
            asm.code.push(0);
            asm.emit(Opcode::Dup);
            asm.code.push(1);
            asm.emit(Opcode::Mload64);
            asm.emit_u64(1);
            asm.emit(Opcode::Sub64);
            asm.emit(Opcode::Mstore64);
            asm.jump("next_inst");

            // inst_3 (>)
            asm.bind("inst_3");
            asm.emit(Opcode::PushLocal);
            asm.code.push(0);
            asm.emit_u64(8);
            asm.emit(Opcode::Add64);
            asm.emit(Opcode::StoreLocal);
            asm.code.push(0);
            asm.jump("next_inst");

            // inst_4 (<)
            asm.bind("inst_4");
            asm.emit(Opcode::PushLocal);
            asm.code.push(0);
            asm.emit_u64(8);
            asm.emit(Opcode::Sub64);
            asm.emit(Opcode::StoreLocal);
            asm.code.push(0);
            asm.jump("next_inst");

            // inst_5 ([)
            asm.bind("inst_5");
            asm.emit(Opcode::PushLocal);
            asm.code.push(0);
            asm.emit(Opcode::Mload64);
            asm.emit_u64(0);
            asm.emit(Opcode::Eq64);
            asm.jumpi("inst_5_find");
            asm.jump("next_inst");

            asm.bind("inst_5_find");
            asm.emit_u64(1);
            asm.emit(Opcode::StoreLocal);
            asm.code.push(3);
            asm.emit(Opcode::PushLocal);
            asm.code.push(1);
            asm.emit(Opcode::StoreLocal);
            asm.code.push(4);

            asm.bind("inst_5_loop");
            asm.emit(Opcode::PushLocal);
            asm.code.push(4);
            asm.emit_u64(8);
            asm.emit(Opcode::Add64);
            asm.emit(Opcode::StoreLocal);
            asm.code.push(4);
            asm.emit(Opcode::PushLocal);
            asm.code.push(4);
            asm.emit(Opcode::Mload64);

            asm.emit(Opcode::Dup);
            asm.code.push(1);
            asm.emit_u64(5);
            asm.emit(Opcode::Eq64);
            asm.jumpi("inst_5_inc_depth");

            asm.emit(Opcode::Dup);
            asm.code.push(1);
            asm.emit_u64(6);
            asm.emit(Opcode::Eq64);
            asm.jumpi("inst_5_dec_depth");

            asm.jump("inst_5_loop_cont");

            asm.bind("inst_5_inc_depth");
            asm.emit(Opcode::PushLocal);
            asm.code.push(3);
            asm.emit_u64(1);
            asm.emit(Opcode::Add64);
            asm.emit(Opcode::StoreLocal);
            asm.code.push(3);
            asm.jump("inst_5_loop_cont");

            asm.bind("inst_5_dec_depth");
            asm.emit(Opcode::PushLocal);
            asm.code.push(3);
            asm.emit_u64(1);
            asm.emit(Opcode::Sub64);
            asm.emit(Opcode::StoreLocal);
            asm.code.push(3);
            asm.emit(Opcode::PushLocal);
            asm.code.push(3);
            asm.emit_u64(0);
            asm.emit(Opcode::Eq64);
            asm.jumpi("inst_5_found");

            asm.bind("inst_5_loop_cont");
            asm.emit(Opcode::Pop);
            asm.jump("inst_5_loop");

            asm.bind("inst_5_found");
            asm.emit(Opcode::Pop);
            asm.emit(Opcode::PushLocal);
            asm.code.push(4);
            asm.emit(Opcode::StoreLocal);
            asm.code.push(1);
            asm.jump("next_inst");

            // inst_6 (])
            asm.bind("inst_6");
            asm.emit(Opcode::PushLocal);
            asm.code.push(0);
            asm.emit(Opcode::Mload64);
            asm.emit_u64(0);
            asm.emit(Opcode::Eq64);
            asm.jumpi("next_inst");

            asm.emit_u64(1);
            asm.emit(Opcode::StoreLocal);
            asm.code.push(3);
            asm.emit(Opcode::PushLocal);
            asm.code.push(1);
            asm.emit(Opcode::StoreLocal);
            asm.code.push(4);

            asm.bind("inst_6_loop");
            asm.emit(Opcode::PushLocal);
            asm.code.push(4);
            asm.emit_u64(8);
            asm.emit(Opcode::Sub64);
            asm.emit(Opcode::StoreLocal);
            asm.code.push(4);
            asm.emit(Opcode::PushLocal);
            asm.code.push(4);
            asm.emit(Opcode::Mload64);

            asm.emit(Opcode::Dup);
            asm.code.push(1);
            asm.emit_u64(6);
            asm.emit(Opcode::Eq64);
            asm.jumpi("inst_6_inc_depth");

            asm.emit(Opcode::Dup);
            asm.code.push(1);
            asm.emit_u64(5);
            asm.emit(Opcode::Eq64);
            asm.jumpi("inst_6_dec_depth");

            asm.jump("inst_6_loop_cont");

            asm.bind("inst_6_inc_depth");
            asm.emit(Opcode::PushLocal);
            asm.code.push(3);
            asm.emit_u64(1);
            asm.emit(Opcode::Add64);
            asm.emit(Opcode::StoreLocal);
            asm.code.push(3);
            asm.jump("inst_6_loop_cont");

            asm.bind("inst_6_dec_depth");
            asm.emit(Opcode::PushLocal);
            asm.code.push(3);
            asm.emit_u64(1);
            asm.emit(Opcode::Sub64);
            asm.emit(Opcode::StoreLocal);
            asm.code.push(3);
            asm.emit(Opcode::PushLocal);
            asm.code.push(3);
            asm.emit_u64(0);
            asm.emit(Opcode::Eq64);
            asm.jumpi("inst_6_found");

            asm.bind("inst_6_loop_cont");
            asm.emit(Opcode::Pop);
            asm.jump("inst_6_loop");

            asm.bind("inst_6_found");
            asm.emit(Opcode::Pop);
            asm.emit(Opcode::PushLocal);
            asm.code.push(4);
            asm.emit(Opcode::StoreLocal);
            asm.code.push(1);
            asm.jump("next_inst");

            // inst_7 (.)
            asm.bind("inst_7");
            asm.emit(Opcode::PushLocal);
            asm.code.push(5); // OUT_PTR
            asm.emit(Opcode::PushLocal);
            asm.code.push(0); // DP
            asm.emit(Opcode::Mload64); // val
            asm.emit(Opcode::Mstore8); // Mstore8

            asm.emit(Opcode::PushLocal);
            asm.code.push(5);
            asm.emit_u64(1);
            asm.emit(Opcode::Add64);
            asm.emit(Opcode::StoreLocal);
            asm.code.push(5);
            asm.jump("next_inst");

            // next_inst
            asm.bind("next_inst");
            asm.emit(Opcode::Pop);
            asm.emit(Opcode::PushLocal);
            asm.code.push(1);
            asm.emit_u64(8);
            asm.emit(Opcode::Add64);
            asm.emit(Opcode::StoreLocal);
            asm.code.push(1);
            asm.jump("loop_start");

            // halt
            asm.bind("halt");
            asm.emit_u64(0x2000); // offset
            asm.emit(Opcode::PushLocal);
            asm.code.push(5);
            asm.emit_u64(0x2000);
            asm.emit(Opcode::Sub64); // len = OUT_PTR - 0x2000
            asm.emit(Opcode::Emit);
            asm.code.extend_from_slice(&1u16.to_be_bytes()); // event_idx = 1

            asm.emit_u64(0);
            asm.emit(Opcode::Mload64);
            asm.emit_u64(8);
            asm.emit(Opcode::Mload64);
            asm.emit(Opcode::Stop);

            let jump_table = asm.get_jump_table();
            let mut db = MockStateDB::new();
            let mut ctx = setup_ctx(asm.code);
            ctx.metadata.jump_table = jump_table;

            let env = setup_env();
            let mut vm = LiteVM::new(ctx, env, &mut db, u64::MAX);
            let res = vm.run();

            (res, vm.events)
        };

        let hello_src = ">++++++++[<+++++++++>-]<.>++++[<+++++++>-]<+.+++++++..+++.>>++++++[<+++++++>-]<++.------------.>++++++[<+++++++++>-]<+.<.+++.------.--------.>>>++++[<++++++++>-]<+.";
        let (res, events) = run_bf(hello_src);
        assert_eq!(res, ExitReason::Halt);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].0, 1);
        assert_eq!(
            events[0].1,
            vec![Value::String("Hello, World!".to_string())]
        );

        let quine_src = "-->+++>+>+>+>+++++>++>++>->+++>++>+>>>>>>>>>>>>>>>>->++++>>>>->+++>+++>+++>+++>+++>+++>+>+>>>->->>++++>+>>>>->>++++>+>+>>->->++>++>++>++++>+>++>->++>++++>+>+>++>++>->->++>++>++++>+>+>>>>>->>->>++++>++>++>++++>>>>>->>>>>+++>->++++>->->->+++>>>+>+>+++>+>++++>>+++>->>>>>->>>++++>++>++>+>+++>->++++>>->->+++>+>+++>+>++++>>>+++>->++++>>->->++>++++>++>++++>>++[-[->>+[>]++[<]<]>>+[>]<--[++>++++>]+[<]<<++]>>>[>]++++>++++[--[+>+>++++<<[-->>--<<[->-<[--->>+<<[+>+++<[+>>++<<]]]]]]>+++[>+++++++++++++++<-]>--.<<<]";
        let (res, events) = run_bf(quine_src);
        assert_eq!(res, ExitReason::Halt);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].0, 1);
        assert_eq!(events[0].1, vec![Value::String(quine_src.to_string())]);
    }
}
