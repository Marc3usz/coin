use ethnum::U256;
use serde::{Deserialize, Serialize};
use sha3::{Digest, Sha3_256};
use std::collections::HashMap;

use super::opcode::Opcode;

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Value {
    U64(u64),
    U256(U256),
    Address([u8; 32]),
    Ref(u32),
    ArrayRef(u32),
    MapRef(u32),
    String(String),
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum HeapObject {
    Struct(Vec<Value>),
    Array(Vec<Value>),
    Map(HashMap<Value, Value>),
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MethodMeta {
    pub args: usize,
    pub rets: usize,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ContractBlob {
    pub metadata: Metadata,
    pub code: Vec<u8>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Metadata {
    pub structs: HashMap<u16, usize>,
    pub jump_table: HashMap<u16, usize>,
    pub methods: HashMap<u16, MethodMeta>,
    pub interfaces: HashMap<u16, MethodMeta>,
}

pub struct Context {
    pub caller: [u8; 32],
    pub address: [u8; 32],
    pub value: U256,
    pub static_call: bool,
    pub metadata: Metadata,
    pub code: Vec<u8>,
    pub call_data: Vec<u8>,
    pub return_data: Vec<u8>,
}

pub struct Env {
    pub block_num: u64,
    pub timestamp: u64,
    pub chain_id: U256,
    pub origin: [u8; 32],
    pub gas_price: U256,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CallKind {
    Standard,
    Static,
    Delegate,
}

pub struct CallRequest {
    pub kind: CallKind,
    pub interface: bool,
    pub code_address: [u8; 32],
    pub context_address: [u8; 32],
    pub caller: [u8; 32],
    pub value: U256,
    pub static_call: bool,
    pub method_idx: u16,
    pub args: Vec<Value>,
    pub gas: u64,
    pub env: Env,
}

pub struct CallResult {
    pub success: bool,
    pub return_values: Vec<Value>,
    pub gas_left: u64,
}

const CONTRACT_BLOB_MAGIC: &[u8; 4] = b"LVM1";

pub fn encode_contract_blob(blob: &ContractBlob) -> anyhow::Result<Vec<u8>> {
    let metadata = bincode::serialize(&blob.metadata)?;
    anyhow::ensure!(metadata.len() <= u32::MAX as usize, "metadata too large");
    let mut out =
        Vec::with_capacity(CONTRACT_BLOB_MAGIC.len() + 4 + metadata.len() + blob.code.len());
    out.extend_from_slice(CONTRACT_BLOB_MAGIC);
    out.extend_from_slice(&(metadata.len() as u32).to_be_bytes());
    out.extend_from_slice(&metadata);
    out.extend_from_slice(&blob.code);
    Ok(out)
}

pub fn decode_contract_blob(bytes: &[u8]) -> anyhow::Result<ContractBlob> {
    if bytes.len() < 8 || &bytes[..4] != CONTRACT_BLOB_MAGIC {
        return Ok(ContractBlob {
            metadata: Metadata::default(),
            code: bytes.to_vec(),
        });
    }

    let metadata_len = u32::from_be_bytes(bytes[4..8].try_into().unwrap()) as usize;
    let metadata_end = 8usize
        .checked_add(metadata_len)
        .ok_or_else(|| anyhow::anyhow!("contract metadata length overflow"))?;
    anyhow::ensure!(metadata_end <= bytes.len(), "truncated contract metadata");
    Ok(ContractBlob {
        metadata: bincode::deserialize(&bytes[8..metadata_end])?,
        code: bytes[metadata_end..].to_vec(),
    })
}

pub struct Arena {
    pub objects: Vec<HeapObject>,
    pub raw_memory: Vec<u8>,
    pub max_memory: usize,
}

impl Arena {
    pub fn new(max_memory: usize) -> Self {
        Self {
            objects: Vec::new(),
            raw_memory: Vec::new(),
            max_memory,
        }
    }

    pub fn alloc(&mut self, obj: HeapObject) -> Result<u32, ExitReason> {
        if self.objects.len() >= self.max_memory {
            return Err(ExitReason::OutOfMemory);
        }
        let id = self.objects.len() as u32;
        self.objects.push(obj);
        Ok(id)
    }

    pub fn ensure_raw_len(&mut self, len: usize) -> Result<(), ExitReason> {
        if len > self.max_memory {
            return Err(ExitReason::OutOfMemory);
        }
        if len > self.raw_memory.len() {
            self.raw_memory.resize(len, 0);
        }
        Ok(())
    }
}

pub trait StateDB {
    fn get_state(&mut self, address: &[u8; 32], field_idx: u8) -> Value;
    fn set_state(
        &mut self,
        address: &[u8; 32],
        field_idx: u8,
        value: Value,
    ) -> Result<(), ExitReason>;
    fn get_balance(&self, address: &[u8; 32]) -> U256;
    fn call_contract(&mut self, request: CallRequest) -> Result<CallResult, ExitReason>;
}

impl Clone for Env {
    fn clone(&self) -> Self {
        Self {
            block_num: self.block_num,
            timestamp: self.timestamp,
            chain_id: self.chain_id,
            origin: self.origin,
            gas_price: self.gas_price,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ExitReason {
    Halt,
    Return(Vec<Value>),
    Revert(Value),
    OutOfGas,
    OutOfMemory,
    InvalidOpcode,
    StackUnderflow,
    OutOfBounds,
    TypeError,
    DivideByZero,
    StaticCallViolation,
    ContractNotFound,
}

pub struct LiteVM<'a> {
    pub stack: Vec<Value>,
    pub locals: Vec<Value>,
    pub pc: usize,
    pub gas: u64,
    pub arena: Arena,
    pub ctx: Context,
    pub env: Env,
    pub state_db: &'a mut dyn StateDB,
    pub events: Vec<(u16, Vec<Value>)>,
}

impl<'a> LiteVM<'a> {
    pub fn new(ctx: Context, env: Env, state_db: &'a mut dyn StateDB, gas_limit: u64) -> Self {
        Self {
            stack: Vec::new(),
            locals: vec![Value::U64(0); 256],
            pc: 0,
            gas: gas_limit,
            arena: Arena::new(64 * 1024),
            ctx,
            env,
            state_db,
            events: Vec::new(),
        }
    }

    pub fn pop(&mut self) -> Result<Value, ExitReason> {
        self.stack.pop().ok_or(ExitReason::StackUnderflow)
    }

    pub fn push(&mut self, val: Value) {
        self.stack.push(val);
    }

    fn read_u8(&mut self) -> Result<u8, ExitReason> {
        if self.pc >= self.ctx.code.len() {
            return Err(ExitReason::OutOfBounds);
        }
        let val = self.ctx.code[self.pc];
        self.pc += 1;
        Ok(val)
    }

    fn read_u16(&mut self) -> Result<u16, ExitReason> {
        if self.pc + 2 > self.ctx.code.len() {
            return Err(ExitReason::OutOfBounds);
        }
        let val = u16::from_be_bytes([self.ctx.code[self.pc], self.ctx.code[self.pc + 1]]);
        self.pc += 2;
        Ok(val)
    }

    fn require_gas(&mut self, amount: u64) -> Result<(), ExitReason> {
        if self.gas < amount {
            return Err(ExitReason::OutOfGas);
        }
        self.gas -= amount;
        Ok(())
    }

    fn checked_range_end(start: usize, len: usize) -> Result<usize, ExitReason> {
        start.checked_add(len).ok_or(ExitReason::OutOfBounds)
    }

    fn pop_u64(&mut self) -> Result<u64, ExitReason> {
        match self.pop()? {
            Value::U64(v) => Ok(v),
            _ => Err(ExitReason::TypeError),
        }
    }

    fn pop_u256(&mut self) -> Result<U256, ExitReason> {
        match self.pop()? {
            Value::U256(v) => Ok(v),
            _ => Err(ExitReason::TypeError),
        }
    }

    fn pop_address(&mut self) -> Result<[u8; 32], ExitReason> {
        match self.pop()? {
            Value::Address(v) => Ok(v),
            _ => Err(ExitReason::TypeError),
        }
    }

    fn pop_call_args(&mut self, args: usize) -> Result<Vec<Value>, ExitReason> {
        let mut values = Vec::with_capacity(args);
        for _ in 0..args {
            values.push(self.pop()?);
        }
        values.reverse();
        Ok(values)
    }

    fn apply_call_result(
        &mut self,
        result: CallResult,
        expected_rets: usize,
    ) -> Result<(), ExitReason> {
        self.gas = self.gas.saturating_add(result.gas_left);
        self.push(Value::U64(if result.success { 1 } else { 0 }));
        if result.success {
            if result.return_values.len() != expected_rets {
                return Err(ExitReason::TypeError);
            }
            for value in result.return_values {
                self.push(value);
            }
        }
        Ok(())
    }

    pub fn step(&mut self) -> Result<(), ExitReason> {
        if self.pc >= self.ctx.code.len() {
            return Err(ExitReason::Halt);
        }

        let op_byte = self.read_u8()?;
        let op = std::convert::TryInto::<Opcode>::try_into(op_byte)
            .map_err(|_| ExitReason::InvalidOpcode)?;

        match op {
            Opcode::Push64 => {
                self.require_gas(3)?;
                if self.pc + 8 > self.ctx.code.len() {
                    return Err(ExitReason::OutOfBounds);
                }
                let mut buf = [0u8; 8];
                buf.copy_from_slice(&self.ctx.code[self.pc..self.pc + 8]);
                self.push(Value::U64(u64::from_be_bytes(buf)));
                self.pc += 8;
            }
            Opcode::Push256 => {
                self.require_gas(5)?;
                if self.pc + 32 > self.ctx.code.len() {
                    return Err(ExitReason::OutOfBounds);
                }
                let mut buf = [0u8; 32];
                buf.copy_from_slice(&self.ctx.code[self.pc..self.pc + 32]);
                let val = U256::from_be_bytes(buf);
                self.push(Value::U256(val));
                self.pc += 32;
            }
            Opcode::PushAddr => {
                self.require_gas(4)?;
                if self.pc + 32 > self.ctx.code.len() {
                    return Err(ExitReason::OutOfBounds);
                }
                let mut buf = [0u8; 32];
                buf.copy_from_slice(&self.ctx.code[self.pc..self.pc + 32]);
                self.push(Value::Address(buf));
                self.pc += 32;
            }
            Opcode::PushLocal => {
                self.require_gas(2)?;
                let idx = self.read_u8()? as usize;
                if idx >= self.locals.len() {
                    return Err(ExitReason::OutOfBounds);
                }
                self.push(self.locals[idx].clone());
            }
            Opcode::StoreLocal => {
                self.require_gas(2)?;
                let idx = self.read_u8()? as usize;
                let val = self.pop()?;
                if idx >= self.locals.len() {
                    return Err(ExitReason::OutOfBounds);
                }
                self.locals[idx] = val;
            }
            Opcode::Pop => {
                self.require_gas(1)?;
                self.pop()?;
            }
            Opcode::Dup => {
                let n = self.read_u8()? as usize;
                self.require_gas(2 + n as u64)?;
                if n == 0 || self.stack.len() < n {
                    return Err(ExitReason::StackUnderflow);
                }
                let val = self.stack[self.stack.len() - n].clone();
                self.push(val);
            }
            Opcode::Swap => {
                let n = self.read_u8()? as usize;
                self.require_gas(2 + n as u64)?;
                let len = self.stack.len();
                if n == 0 || len < n {
                    return Err(ExitReason::StackUnderflow);
                }
                self.stack.swap(len - 1, len - n);
            }
            Opcode::Add64 => {
                self.require_gas(3)?;
                let b = self.pop()?;
                let a = self.pop()?;
                if let (Value::U64(va), Value::U64(vb)) = (a, b) {
                    self.push(Value::U64(va.wrapping_add(vb)));
                } else {
                    return Err(ExitReason::TypeError);
                }
            }
            Opcode::Sub64 => {
                self.require_gas(3)?;
                let b = self.pop()?;
                let a = self.pop()?;
                if let (Value::U64(va), Value::U64(vb)) = (a, b) {
                    self.push(Value::U64(va.wrapping_sub(vb)));
                } else {
                    return Err(ExitReason::TypeError);
                }
            }
            Opcode::Mul64 => {
                self.require_gas(5)?;
                let b = self.pop()?;
                let a = self.pop()?;
                if let (Value::U64(va), Value::U64(vb)) = (a, b) {
                    self.push(Value::U64(va.wrapping_mul(vb)));
                } else {
                    return Err(ExitReason::TypeError);
                }
            }
            Opcode::Div64 => {
                self.require_gas(5)?;
                let b = self.pop()?;
                let a = self.pop()?;
                if let (Value::U64(va), Value::U64(vb)) = (a, b) {
                    if vb == 0 {
                        return Err(ExitReason::DivideByZero);
                    }
                    self.push(Value::U64(va / vb));
                } else {
                    return Err(ExitReason::TypeError);
                }
            }
            Opcode::Mod64 => {
                self.require_gas(5)?;
                let b = self.pop()?;
                let a = self.pop()?;
                if let (Value::U64(va), Value::U64(vb)) = (a, b) {
                    if vb == 0 {
                        return Err(ExitReason::DivideByZero);
                    }
                    self.push(Value::U64(va % vb));
                } else {
                    return Err(ExitReason::TypeError);
                }
            }
            Opcode::Shl64 => {
                self.require_gas(4)?;
                let shift = self.pop()?;
                let val = self.pop()?;
                if let (Value::U64(v), Value::U64(s)) = (val, shift) {
                    self.push(Value::U64(v.wrapping_shl(s as u32)));
                } else {
                    return Err(ExitReason::TypeError);
                }
            }
            Opcode::Shr64 => {
                self.require_gas(4)?;
                let shift = self.pop()?;
                let val = self.pop()?;
                if let (Value::U64(v), Value::U64(s)) = (val, shift) {
                    self.push(Value::U64(v.wrapping_shr(s as u32)));
                } else {
                    return Err(ExitReason::TypeError);
                }
            }
            Opcode::Eq64 => {
                self.require_gas(3)?;
                let b = self.pop()?;
                let a = self.pop()?;
                if let (Value::U64(va), Value::U64(vb)) = (a, b) {
                    self.push(Value::U64(if va == vb { 1 } else { 0 }));
                } else {
                    return Err(ExitReason::TypeError);
                }
            }
            Opcode::Lt64 => {
                self.require_gas(3)?;
                let b = self.pop()?;
                let a = self.pop()?;
                if let (Value::U64(va), Value::U64(vb)) = (a, b) {
                    self.push(Value::U64(if va < vb { 1 } else { 0 }));
                } else {
                    return Err(ExitReason::TypeError);
                }
            }
            Opcode::Gt64 => {
                self.require_gas(3)?;
                let b = self.pop()?;
                let a = self.pop()?;
                if let (Value::U64(va), Value::U64(vb)) = (a, b) {
                    self.push(Value::U64(if va > vb { 1 } else { 0 }));
                } else {
                    return Err(ExitReason::TypeError);
                }
            }
            Opcode::Add256 => {
                self.require_gas(4)?;
                let b = self.pop()?;
                let a = self.pop()?;
                if let (Value::U256(va), Value::U256(vb)) = (a, b) {
                    self.push(Value::U256(va.wrapping_add(vb)));
                } else {
                    return Err(ExitReason::TypeError);
                }
            }
            Opcode::Sub256 => {
                self.require_gas(4)?;
                let b = self.pop()?;
                let a = self.pop()?;
                if let (Value::U256(va), Value::U256(vb)) = (a, b) {
                    self.push(Value::U256(va.wrapping_sub(vb)));
                } else {
                    return Err(ExitReason::TypeError);
                }
            }
            Opcode::Mul256 => {
                self.require_gas(6)?;
                let b = self.pop()?;
                let a = self.pop()?;
                if let (Value::U256(va), Value::U256(vb)) = (a, b) {
                    self.push(Value::U256(va.wrapping_mul(vb)));
                } else {
                    return Err(ExitReason::TypeError);
                }
            }
            Opcode::Div256 => {
                self.require_gas(6)?;
                let b = self.pop()?;
                let a = self.pop()?;
                if let (Value::U256(va), Value::U256(vb)) = (a, b) {
                    if vb == U256::ZERO {
                        return Err(ExitReason::DivideByZero);
                    }
                    self.push(Value::U256(va / vb));
                } else {
                    return Err(ExitReason::TypeError);
                }
            }
            Opcode::Mod256 => {
                self.require_gas(6)?;
                let b = self.pop()?;
                let a = self.pop()?;
                if let (Value::U256(va), Value::U256(vb)) = (a, b) {
                    if vb == U256::ZERO {
                        return Err(ExitReason::DivideByZero);
                    }
                    self.push(Value::U256(va % vb));
                } else {
                    return Err(ExitReason::TypeError);
                }
            }
            Opcode::Shl256 => {
                self.require_gas(4)?;
                let shift = self.pop()?;
                let val = self.pop()?;
                if let (Value::U256(v), Value::U256(s)) = (val, shift) {
                    let shift_u32 = if s > U256::from(255_u32) {
                        256
                    } else {
                        s.as_u32()
                    };
                    if shift_u32 >= 256 {
                        self.push(Value::U256(U256::ZERO));
                    } else {
                        self.push(Value::U256(v << shift_u32));
                    }
                } else {
                    return Err(ExitReason::TypeError);
                }
            }
            Opcode::Shr256 => {
                self.require_gas(4)?;
                let shift = self.pop()?;
                let val = self.pop()?;
                if let (Value::U256(v), Value::U256(s)) = (val, shift) {
                    let shift_u32 = if s > U256::from(255_u32) {
                        256
                    } else {
                        s.as_u32()
                    };
                    if shift_u32 >= 256 {
                        self.push(Value::U256(U256::ZERO));
                    } else {
                        self.push(Value::U256(v >> shift_u32));
                    }
                } else {
                    return Err(ExitReason::TypeError);
                }
            }
            Opcode::Eq256 => {
                self.require_gas(4)?;
                let b = self.pop()?;
                let a = self.pop()?;
                if let (Value::U256(va), Value::U256(vb)) = (a, b) {
                    self.push(Value::U64(if va == vb { 1 } else { 0 }));
                } else {
                    return Err(ExitReason::TypeError);
                }
            }
            Opcode::Lt256 => {
                self.require_gas(4)?;
                let b = self.pop()?;
                let a = self.pop()?;
                if let (Value::U256(va), Value::U256(vb)) = (a, b) {
                    self.push(Value::U64(if va < vb { 1 } else { 0 }));
                } else {
                    return Err(ExitReason::TypeError);
                }
            }
            Opcode::Gt256 => {
                self.require_gas(4)?;
                let b = self.pop()?;
                let a = self.pop()?;
                if let (Value::U256(va), Value::U256(vb)) = (a, b) {
                    self.push(Value::U64(if va > vb { 1 } else { 0 }));
                } else {
                    return Err(ExitReason::TypeError);
                }
            }
            Opcode::And => {
                self.require_gas(3)?;
                let b = self.pop()?;
                let a = self.pop()?;
                if let (Value::U256(va), Value::U256(vb)) = (a, b) {
                    self.push(Value::U256(va & vb));
                } else {
                    return Err(ExitReason::TypeError);
                }
            }
            Opcode::Or => {
                self.require_gas(3)?;
                let b = self.pop()?;
                let a = self.pop()?;
                if let (Value::U256(va), Value::U256(vb)) = (a, b) {
                    self.push(Value::U256(va | vb));
                } else {
                    return Err(ExitReason::TypeError);
                }
            }
            Opcode::Xor => {
                self.require_gas(3)?;
                let b = self.pop()?;
                let a = self.pop()?;
                if let (Value::U256(va), Value::U256(vb)) = (a, b) {
                    self.push(Value::U256(va ^ vb));
                } else {
                    return Err(ExitReason::TypeError);
                }
            }
            Opcode::Not => {
                self.require_gas(3)?;
                let a = self.pop()?;
                if let Value::U256(va) = a {
                    self.push(Value::U256(!va));
                } else {
                    return Err(ExitReason::TypeError);
                }
            }
            Opcode::IsZero => {
                self.require_gas(2)?;
                let a = self.pop()?;
                match a {
                    Value::U64(va) => self.push(Value::U64(if va == 0 { 1 } else { 0 })),
                    Value::U256(va) => self.push(Value::U64(if va == U256::ZERO { 1 } else { 0 })),
                    _ => return Err(ExitReason::TypeError),
                }
            }
            Opcode::Cast64To256 => {
                self.require_gas(2)?;
                let a = self.pop()?;
                if let Value::U64(va) = a {
                    self.push(Value::U256(U256::from(va)));
                } else {
                    return Err(ExitReason::TypeError);
                }
            }
            Opcode::Cast256To64 => {
                self.require_gas(3)?;
                let a = self.pop()?;
                if let Value::U256(va) = a {
                    self.push(Value::U64(va.as_u64()));
                } else {
                    return Err(ExitReason::TypeError);
                }
            }
            Opcode::CastAddrTo256 => {
                self.require_gas(2)?;
                let a = self.pop()?;
                if let Value::Address(va) = a {
                    self.push(Value::U256(U256::from_be_bytes(va)));
                } else {
                    return Err(ExitReason::TypeError);
                }
            }
            Opcode::Cast256ToAddr => {
                self.require_gas(3)?;
                let a = self.pop()?;
                if let Value::U256(va) = a {
                    let buf = va.to_be_bytes();
                    self.push(Value::Address(buf));
                } else {
                    return Err(ExitReason::TypeError);
                }
            }
            Opcode::Jump => {
                self.require_gas(5)?;
                let jump_idx = self.read_u16()?;
                if let Some(&offset) = self.ctx.metadata.jump_table.get(&jump_idx) {
                    self.pc = offset;
                } else {
                    return Err(ExitReason::OutOfBounds);
                }
            }
            Opcode::Jumpi => {
                self.require_gas(6)?;
                let jump_idx = self.read_u16()?;
                let cond = self.pop()?;
                let is_true = match cond {
                    Value::U64(v) => v != 0,
                    Value::U256(v) => v != U256::ZERO,
                    _ => return Err(ExitReason::TypeError),
                };
                if is_true {
                    if let Some(&offset) = self.ctx.metadata.jump_table.get(&jump_idx) {
                        self.pc = offset;
                    } else {
                        return Err(ExitReason::OutOfBounds);
                    }
                }
            }
            Opcode::Return => {
                self.require_gas(8)?;
                let mut rets = Vec::new();
                while let Ok(v) = self.pop() {
                    rets.push(v);
                }
                rets.reverse();
                return Err(ExitReason::Return(rets));
            }
            Opcode::Revert => {
                self.require_gas(5)?;
                let err_code = self.pop()?;
                return Err(ExitReason::Revert(err_code));
            }
            Opcode::Stop => {
                self.require_gas(1)?;
                return Err(ExitReason::Halt);
            }
            // Mload and friends
            Opcode::Mload8 => {
                self.require_gas(2)?;
                let offset = self.pop()?;
                if let Value::U64(off) = offset {
                    let off = off as usize;
                    let end = Self::checked_range_end(off, 1)?;
                    if end > self.arena.raw_memory.len() {
                        return Err(ExitReason::OutOfBounds);
                    }
                    let val = self.arena.raw_memory[off];
                    self.push(Value::U64(val as u64));
                } else {
                    return Err(ExitReason::TypeError);
                }
            }
            Opcode::Mstore8 => {
                self.require_gas(3)?;
                let val = self.pop()?;
                let offset = self.pop()?;
                if let (Value::U64(off), Value::U64(v)) = (offset, val) {
                    let off = off as usize;
                    let end = Self::checked_range_end(off, 1)?;
                    self.arena.ensure_raw_len(end)?;
                    self.arena.raw_memory[off] = (v & 0xFF) as u8;
                } else {
                    return Err(ExitReason::TypeError);
                }
            }
            Opcode::Mload64 => {
                self.require_gas(4)?;
                let offset = self.pop()?;
                if let Value::U64(off) = offset {
                    let off = off as usize;
                    let end = Self::checked_range_end(off, 8)?;
                    if end > self.arena.raw_memory.len() {
                        return Err(ExitReason::OutOfBounds);
                    }
                    let mut buf = [0u8; 8];
                    buf.copy_from_slice(&self.arena.raw_memory[off..off + 8]);
                    self.push(Value::U64(u64::from_be_bytes(buf)));
                } else {
                    return Err(ExitReason::TypeError);
                }
            }
            Opcode::Mstore64 => {
                self.require_gas(5)?;
                let val = self.pop()?;
                let offset = self.pop()?;
                if let (Value::U64(off), Value::U64(v)) = (offset, val) {
                    let off = off as usize;
                    let end = Self::checked_range_end(off, 8)?;
                    self.arena.ensure_raw_len(end)?;
                    self.arena.raw_memory[off..end].copy_from_slice(&v.to_be_bytes());
                } else {
                    return Err(ExitReason::TypeError);
                }
            }
            Opcode::Mload256 => {
                self.require_gas(5)?;
                let offset = self.pop()?;
                if let Value::U64(off) = offset {
                    let off = off as usize;
                    let end = Self::checked_range_end(off, 32)?;
                    if end > self.arena.raw_memory.len() {
                        return Err(ExitReason::OutOfBounds);
                    }
                    let mut buf = [0u8; 32];
                    buf.copy_from_slice(&self.arena.raw_memory[off..off + 32]);
                    self.push(Value::U256(U256::from_be_bytes(buf)));
                } else {
                    return Err(ExitReason::TypeError);
                }
            }
            Opcode::Mstore256 => {
                self.require_gas(6)?;
                let val = self.pop()?;
                let offset = self.pop()?;
                if let (Value::U64(off), Value::U256(v)) = (offset, val) {
                    let off = off as usize;
                    let end = Self::checked_range_end(off, 32)?;
                    self.arena.ensure_raw_len(end)?;
                    let buf = v.to_be_bytes();
                    self.arena.raw_memory[off..end].copy_from_slice(&buf);
                } else {
                    return Err(ExitReason::TypeError);
                }
            }
            Opcode::Mcopy => {
                self.require_gas(3)?;
                let len = self.pop()?;
                let src = self.pop()?;
                let dst = self.pop()?;
                if let (Value::U64(l), Value::U64(s), Value::U64(d)) = (len, src, dst) {
                    let l = l as usize;
                    let s = s as usize;
                    let d = d as usize;
                    self.require_gas(l as u64 / 32)?; // dummy dyn gas
                    let src_end = Self::checked_range_end(s, l)?;
                    let dst_end = Self::checked_range_end(d, l)?;
                    if src_end > self.arena.raw_memory.len() {
                        return Err(ExitReason::OutOfBounds);
                    }
                    self.arena.ensure_raw_len(dst_end)?;
                    self.arena.raw_memory.copy_within(s..src_end, d);
                } else {
                    return Err(ExitReason::TypeError);
                }
            }
            Opcode::New => {
                self.require_gas(20)?;
                let struct_id = self.read_u16()?;
                if let Some(&num_fields) = self.ctx.metadata.structs.get(&struct_id) {
                    let obj = HeapObject::Struct(vec![Value::U64(0); num_fields]);
                    let id = self.arena.alloc(obj)?;
                    self.push(Value::Ref(id));
                } else {
                    return Err(ExitReason::OutOfBounds);
                }
            }
            Opcode::GetField => {
                self.require_gas(5)?;
                let field_idx = self.read_u8()? as usize;
                let obj_ref = self.pop()?;
                if let Value::Ref(id) = obj_ref {
                    if let Some(HeapObject::Struct(fields)) = self.arena.objects.get(id as usize) {
                        if field_idx < fields.len() {
                            self.push(fields[field_idx].clone());
                        } else {
                            return Err(ExitReason::OutOfBounds);
                        }
                    } else {
                        return Err(ExitReason::TypeError);
                    }
                } else {
                    return Err(ExitReason::TypeError);
                }
            }
            Opcode::SetField => {
                self.require_gas(5)?;
                let field_idx = self.read_u8()? as usize;
                let obj_ref = self.pop()?;
                let val = self.pop()?;
                if let Value::Ref(id) = obj_ref {
                    if let Some(HeapObject::Struct(fields)) =
                        self.arena.objects.get_mut(id as usize)
                    {
                        if field_idx < fields.len() {
                            fields[field_idx] = val;
                        } else {
                            return Err(ExitReason::OutOfBounds);
                        }
                    } else {
                        return Err(ExitReason::TypeError);
                    }
                } else {
                    return Err(ExitReason::TypeError);
                }
            }
            Opcode::GetState => {
                self.require_gas(100)?;
                let field_idx = self.read_u8()?;
                let val = self.state_db.get_state(&self.ctx.address, field_idx);
                self.push(val);
            }
            Opcode::SetState => {
                self.require_gas(2000)?;
                if self.ctx.static_call {
                    return Err(ExitReason::StaticCallViolation);
                }
                let field_idx = self.read_u8()?;
                let val = self.pop()?;
                self.state_db.set_state(&self.ctx.address, field_idx, val)?;
            }
            Opcode::NewMap => {
                self.require_gas(15)?;
                let obj = HeapObject::Map(HashMap::new());
                let id = self.arena.alloc(obj)?;
                self.push(Value::MapRef(id));
            }
            Opcode::MapGet => {
                self.require_gas(100)?;
                let map_ref = self.pop()?;
                let key = self.pop()?;
                if let Value::MapRef(id) = map_ref {
                    if let Some(HeapObject::Map(map)) = self.arena.objects.get(id as usize) {
                        let val = map.get(&key).cloned().unwrap_or(Value::U64(0));
                        self.push(val);
                    } else {
                        return Err(ExitReason::TypeError);
                    }
                } else {
                    return Err(ExitReason::TypeError);
                }
            }
            Opcode::MapSet => {
                self.require_gas(2000)?;
                if self.ctx.static_call {
                    return Err(ExitReason::StaticCallViolation);
                }
                let map_ref = self.pop()?;
                let key = self.pop()?;
                let val = self.pop()?;
                if let Value::MapRef(id) = map_ref {
                    if let Some(HeapObject::Map(map)) = self.arena.objects.get_mut(id as usize) {
                        map.insert(key, val);
                    } else {
                        return Err(ExitReason::TypeError);
                    }
                } else {
                    return Err(ExitReason::TypeError);
                }
            }
            Opcode::NewArray => {
                self.require_gas(20)?;
                let _type_id = self.read_u16()?;
                let len_val = self.pop()?;
                if let Value::U64(len) = len_val {
                    let obj = HeapObject::Array(vec![Value::U64(0); len as usize]);
                    let id = self.arena.alloc(obj)?;
                    self.push(Value::ArrayRef(id));
                } else {
                    return Err(ExitReason::TypeError);
                }
            }
            Opcode::ArrayGet => {
                self.require_gas(5)?;
                let arr_ref = self.pop()?;
                let idx = self.pop()?;
                if let (Value::ArrayRef(id), Value::U64(i)) = (arr_ref, idx) {
                    if let Some(HeapObject::Array(arr)) = self.arena.objects.get(id as usize) {
                        if (i as usize) < arr.len() {
                            self.push(arr[i as usize].clone());
                        } else {
                            return Err(ExitReason::OutOfBounds);
                        }
                    } else {
                        return Err(ExitReason::TypeError);
                    }
                } else {
                    return Err(ExitReason::TypeError);
                }
            }
            Opcode::ArraySet => {
                self.require_gas(5)?;
                let arr_ref = self.pop()?;
                let idx = self.pop()?;
                let val = self.pop()?;
                if let (Value::ArrayRef(id), Value::U64(i)) = (arr_ref, idx) {
                    if let Some(HeapObject::Array(arr)) = self.arena.objects.get_mut(id as usize) {
                        if (i as usize) < arr.len() {
                            arr[i as usize] = val;
                        } else {
                            return Err(ExitReason::OutOfBounds);
                        }
                    } else {
                        return Err(ExitReason::TypeError);
                    }
                } else {
                    return Err(ExitReason::TypeError);
                }
            }
            Opcode::ArrayLen => {
                self.require_gas(3)?;
                let arr_ref = self.pop()?;
                if let Value::ArrayRef(id) = arr_ref {
                    if let Some(HeapObject::Array(arr)) = self.arena.objects.get(id as usize) {
                        self.push(Value::U64(arr.len() as u64));
                    } else {
                        return Err(ExitReason::TypeError);
                    }
                } else {
                    return Err(ExitReason::TypeError);
                }
            }
            Opcode::Caller => {
                self.require_gas(2)?;
                self.push(Value::Address(self.ctx.caller));
            }
            Opcode::CallValue => {
                self.require_gas(2)?;
                self.push(Value::U256(self.ctx.value));
            }
            Opcode::Address => {
                self.require_gas(2)?;
                self.push(Value::Address(self.ctx.address));
            }
            Opcode::Balance => {
                self.require_gas(100)?;
                let addr = self.pop()?;
                if let Value::Address(a) = addr {
                    self.push(Value::U256(self.state_db.get_balance(&a)));
                } else {
                    return Err(ExitReason::TypeError);
                }
            }
            Opcode::BlockNum => {
                self.require_gas(2)?;
                self.push(Value::U64(self.env.block_num));
            }
            Opcode::Timestamp => {
                self.require_gas(2)?;
                self.push(Value::U64(self.env.timestamp));
            }
            Opcode::Sha3 => {
                self.require_gas(30)?;
                let len = self.pop()?;
                let offset = self.pop()?;
                if let (Value::U64(l), Value::U64(off)) = (len, offset) {
                    let l = l as usize;
                    let off = off as usize;
                    let end = Self::checked_range_end(off, l)?;
                    if end > self.arena.raw_memory.len() {
                        return Err(ExitReason::OutOfBounds);
                    }
                    let hash = Sha3_256::digest(&self.arena.raw_memory[off..end]);
                    let mut buf = [0u8; 32];
                    buf.copy_from_slice(&hash);
                    self.push(Value::U256(U256::from_be_bytes(buf)));
                } else {
                    return Err(ExitReason::TypeError);
                }
            }
            Opcode::ChainId => {
                self.require_gas(2)?;
                self.push(Value::U256(self.env.chain_id));
            }
            Opcode::Origin => {
                self.require_gas(2)?;
                self.push(Value::Address(self.env.origin));
            }
            Opcode::GasLeft => {
                self.require_gas(2)?;
                self.push(Value::U64(self.gas));
            }
            Opcode::GasPrice => {
                self.require_gas(2)?;
                self.push(Value::U256(self.env.gas_price));
            }
            Opcode::ReturnDataSize => {
                self.require_gas(2)?;
                self.push(Value::U64(self.ctx.return_data.len() as u64));
            }
            Opcode::Invoke => {
                self.require_gas(700)?;
                let method_idx = self.read_u16()?;
                let meta = self
                    .ctx
                    .metadata
                    .methods
                    .get(&method_idx)
                    .ok_or(ExitReason::OutOfBounds)?
                    .clone();
                let args = self.pop_call_args(meta.args)?;
                let value = self.pop_u256()?;
                let address = self.pop_address()?;
                let gas = self.pop_u64()?;
                self.require_gas(gas)?;
                let result = self.state_db.call_contract(CallRequest {
                    kind: CallKind::Standard,
                    interface: false,
                    code_address: address,
                    context_address: address,
                    caller: self.ctx.address,
                    value,
                    static_call: false,
                    method_idx,
                    args,
                    gas,
                    env: self.env.clone(),
                })?;
                self.apply_call_result(result, meta.rets)?;
            }
            Opcode::InvokeStatic => {
                self.require_gas(700)?;
                let method_idx = self.read_u16()?;
                let meta = self
                    .ctx
                    .metadata
                    .methods
                    .get(&method_idx)
                    .ok_or(ExitReason::OutOfBounds)?
                    .clone();
                let args = self.pop_call_args(meta.args)?;
                let address = self.pop_address()?;
                let gas = self.pop_u64()?;
                self.require_gas(gas)?;
                let result = self.state_db.call_contract(CallRequest {
                    kind: CallKind::Static,
                    interface: false,
                    code_address: address,
                    context_address: address,
                    caller: self.ctx.address,
                    value: U256::ZERO,
                    static_call: true,
                    method_idx,
                    args,
                    gas,
                    env: self.env.clone(),
                })?;
                self.apply_call_result(result, meta.rets)?;
            }
            Opcode::InvokeDelegate => {
                self.require_gas(700)?;
                let method_idx = self.read_u16()?;
                let meta = self
                    .ctx
                    .metadata
                    .methods
                    .get(&method_idx)
                    .ok_or(ExitReason::OutOfBounds)?
                    .clone();
                let args = self.pop_call_args(meta.args)?;
                let address = self.pop_address()?;
                let gas = self.pop_u64()?;
                self.require_gas(gas)?;
                let result = self.state_db.call_contract(CallRequest {
                    kind: CallKind::Delegate,
                    interface: false,
                    code_address: address,
                    context_address: self.ctx.address,
                    caller: self.ctx.caller,
                    value: self.ctx.value,
                    static_call: self.ctx.static_call,
                    method_idx,
                    args,
                    gas,
                    env: self.env.clone(),
                })?;
                self.apply_call_result(result, meta.rets)?;
            }
            Opcode::InvokeInterface => {
                self.require_gas(800)?;
                let interface_idx = self.read_u16()?;
                let meta = self
                    .ctx
                    .metadata
                    .interfaces
                    .get(&interface_idx)
                    .ok_or(ExitReason::OutOfBounds)?
                    .clone();
                let args = self.pop_call_args(meta.args)?;
                let value = self.pop_u256()?;
                let address = self.pop_address()?;
                let gas = self.pop_u64()?;
                self.require_gas(gas)?;
                let result = self.state_db.call_contract(CallRequest {
                    kind: CallKind::Standard,
                    interface: true,
                    code_address: address,
                    context_address: address,
                    caller: self.ctx.address,
                    value,
                    static_call: false,
                    method_idx: interface_idx,
                    args,
                    gas,
                    env: self.env.clone(),
                })?;
                self.apply_call_result(result, meta.rets)?;
            }
            Opcode::InvokeItfStatic => {
                self.require_gas(800)?;
                let interface_idx = self.read_u16()?;
                let meta = self
                    .ctx
                    .metadata
                    .interfaces
                    .get(&interface_idx)
                    .ok_or(ExitReason::OutOfBounds)?
                    .clone();
                let args = self.pop_call_args(meta.args)?;
                let address = self.pop_address()?;
                let gas = self.pop_u64()?;
                self.require_gas(gas)?;
                let result = self.state_db.call_contract(CallRequest {
                    kind: CallKind::Static,
                    interface: true,
                    code_address: address,
                    context_address: address,
                    caller: self.ctx.address,
                    value: U256::ZERO,
                    static_call: true,
                    method_idx: interface_idx,
                    args,
                    gas,
                    env: self.env.clone(),
                })?;
                self.apply_call_result(result, meta.rets)?;
            }
            Opcode::Emit => {
                self.require_gas(375)?;
                let event_idx = self.read_u16()?;
                let len = self.pop()?;
                let offset = self.pop()?;
                if let (Value::U64(off), Value::U64(l)) = (offset, len) {
                    let start = off as usize;
                    let end = Self::checked_range_end(start, l as usize)?;
                    self.arena.ensure_raw_len(end)?;
                    let buf = &self.arena.raw_memory[start..end];
                    let s = String::from_utf8_lossy(buf).into_owned();
                    self.events.push((event_idx, vec![Value::String(s)]));
                } else {
                    return Err(ExitReason::TypeError);
                }
            }
            Opcode::CallRaw => {
                self.require_gas(700)?;
                let _ret_sz = self.pop_u64()?;
                let _ret_off = self.pop_u64()?;
                let _arg_sz = self.pop_u64()?;
                let _arg_off = self.pop_u64()?;
                let _val = self.pop_u256()?;
                let _addr = self.pop_address()?;
                let _gas = self.pop_u64()?;
                self.push(Value::U64(0));
            }
            Opcode::CallDataLoad => {
                self.require_gas(4)?;
                let offset = self.pop()?;
                if let Value::U64(off) = offset {
                    let start = off as usize;
                    let mut buf = [0u8; 32];
                    if start < self.ctx.call_data.len() {
                        let available = (self.ctx.call_data.len() - start).min(32);
                        buf[..available]
                            .copy_from_slice(&self.ctx.call_data[start..start + available]);
                    }
                    self.push(Value::U256(U256::from_be_bytes(buf)));
                } else {
                    return Err(ExitReason::TypeError);
                }
            }
            Opcode::ReturnDataCopy => {
                self.require_gas(3)?;
                let len = self.pop()?;
                let src = self.pop()?;
                let dst = self.pop()?;
                if let (Value::U64(l), Value::U64(s), Value::U64(d)) = (len, src, dst) {
                    let l = l as usize;
                    let s = s as usize;
                    let d = d as usize;
                    let src_end = Self::checked_range_end(s, l)?;
                    let dst_end = Self::checked_range_end(d, l)?;
                    if src_end > self.ctx.return_data.len() {
                        return Err(ExitReason::OutOfBounds);
                    }
                    self.arena.ensure_raw_len(dst_end)?;
                    self.arena.raw_memory[d..dst_end]
                        .copy_from_slice(&self.ctx.return_data[s..src_end]);
                } else {
                    return Err(ExitReason::TypeError);
                }
            }
            Opcode::Invalid => return Err(ExitReason::InvalidOpcode),
        }
        Ok(())
    }

    pub fn run(&mut self) -> ExitReason {
        loop {
            if let Err(e) = self.step() {
                return e;
            }
        }
    }
}
