use coin::{encode_contract_blob, ContractBlob, Metadata, MethodMeta, Opcode};

const INIT: u16 = 0;
const MINT: u16 = 1;
const TRANSFER: u16 = 2;
const OWNER_OF: u16 = 3;
const TRANSFER_OK: u16 = 100;

fn p64(code: &mut Vec<u8>, value: u64) {
    code.push(Opcode::Push64 as u8);
    code.extend_from_slice(&value.to_be_bytes());
}

fn imm16(code: &mut Vec<u8>, value: u16) {
    code.extend_from_slice(&value.to_be_bytes());
}

fn emit_text(code: &mut Vec<u8>, event: u16, offset: u64, text: &[u8]) {
    for (i, byte) in text.iter().enumerate() {
        p64(code, offset + i as u64);
        p64(code, *byte as u64);
        code.push(Opcode::Mstore8 as u8);
    }
    p64(code, offset);
    p64(code, text.len() as u64);
    code.push(Opcode::Emit as u8);
    imm16(code, event);
}

fn main() -> anyhow::Result<()> {
    let mut code = Vec::new();
    let mut metadata = Metadata::default();

    let init_pc = code.len();
    code.push(Opcode::NewMap as u8);
    code.push(Opcode::SetState as u8);
    code.push(0);
    emit_text(&mut code, 0, 0, b"init");
    code.push(Opcode::Stop as u8);

    let mint_pc = code.len();
    code.push(Opcode::StoreLocal as u8);
    code.push(1); // to
    code.push(Opcode::StoreLocal as u8);
    code.push(0); // token_id
    code.push(Opcode::GetState as u8);
    code.push(0);
    code.push(Opcode::StoreLocal as u8);
    code.push(2); // owners map
    code.push(Opcode::PushLocal as u8);
    code.push(1); // value owner
    code.push(Opcode::PushLocal as u8);
    code.push(0); // key token_id
    code.push(Opcode::PushLocal as u8);
    code.push(2); // map
    code.push(Opcode::MapSet as u8);
    code.push(Opcode::PushLocal as u8);
    code.push(2);
    code.push(Opcode::SetState as u8);
    code.push(0);
    emit_text(&mut code, 1, 16, b"mint");
    code.push(Opcode::Stop as u8);

    let transfer_pc = code.len();
    code.push(Opcode::StoreLocal as u8);
    code.push(1); // to
    code.push(Opcode::StoreLocal as u8);
    code.push(0); // token_id
    code.push(Opcode::GetState as u8);
    code.push(0);
    code.push(Opcode::StoreLocal as u8);
    code.push(2); // owners map
    code.push(Opcode::PushLocal as u8);
    code.push(0);
    code.push(Opcode::PushLocal as u8);
    code.push(2);
    code.push(Opcode::MapGet as u8); // current owner
    code.push(Opcode::Origin as u8);
    code.push(Opcode::CastAddrTo256 as u8);
    code.push(Opcode::Swap as u8);
    code.push(2);
    code.push(Opcode::CastAddrTo256 as u8);
    code.push(Opcode::Eq256 as u8);
    code.push(Opcode::Jumpi as u8);
    imm16(&mut code, TRANSFER_OK);
    p64(&mut code, 403);
    code.push(Opcode::Revert as u8);
    let transfer_ok_pc = code.len();
    code.push(Opcode::PushLocal as u8);
    code.push(1); // new owner
    code.push(Opcode::PushLocal as u8);
    code.push(0); // token_id
    code.push(Opcode::PushLocal as u8);
    code.push(2); // map
    code.push(Opcode::MapSet as u8);
    code.push(Opcode::PushLocal as u8);
    code.push(2);
    code.push(Opcode::SetState as u8);
    code.push(0);
    emit_text(&mut code, 2, 32, b"transfer");
    code.push(Opcode::Stop as u8);

    let owner_pc = code.len();
    code.push(Opcode::StoreLocal as u8);
    code.push(0); // token_id
    code.push(Opcode::GetState as u8);
    code.push(0);
    code.push(Opcode::StoreLocal as u8);
    code.push(1);
    code.push(Opcode::PushLocal as u8);
    code.push(0);
    code.push(Opcode::PushLocal as u8);
    code.push(1);
    code.push(Opcode::MapGet as u8);
    code.push(Opcode::Return as u8);

    metadata
        .methods
        .insert(INIT, MethodMeta { args: 0, rets: 0 });
    metadata
        .methods
        .insert(MINT, MethodMeta { args: 2, rets: 0 });
    metadata
        .methods
        .insert(TRANSFER, MethodMeta { args: 2, rets: 0 });
    metadata
        .methods
        .insert(OWNER_OF, MethodMeta { args: 1, rets: 1 });
    metadata.jump_table.insert(INIT, init_pc);
    metadata.jump_table.insert(MINT, mint_pc);
    metadata.jump_table.insert(TRANSFER, transfer_pc);
    metadata.jump_table.insert(OWNER_OF, owner_pc);
    metadata.jump_table.insert(TRANSFER_OK, transfer_ok_pc);

    let blob = encode_contract_blob(&ContractBlob { metadata, code })?;
    println!("{}", hex::encode(blob));
    Ok(())
}
