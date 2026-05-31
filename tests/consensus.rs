use coin::chain::{contract_address, ChainCore};
use coin::config::NodeConfig;
use coin::crypto::{
    address_from_public_key, hash_leq_target, hex_hash, nbits_to_target, scale_target, sha3_256,
    target_to_nbits,
};
use coin::mempool::Mempool;
use coin::types::{
    block_reward, genesis_header, next_gas_price, receipt_root, tx_root, Account, Block,
    BlockHeader, BlockReceipts, Receipt, Transaction, BASE_REWARD, GENESIS_GAS_PRICE, MAGIC,
    MIN_GAS_PRICE, PROTOCOL_VERSION, TAIL_REWARD,
};
use coin::wallet::{sign_tx, WalletFile};
use coin::{
    encode_contract_blob, encode_contract_call, ContractBlob, ContractCallKind,
    ContractCallPayload, Metadata, MethodMeta, Opcode, Value,
};
use ethnum::U256;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

static TEST_DIR_COUNTER: AtomicU64 = AtomicU64::new(0);

fn tx(tip: u128, nonce: u64) -> Transaction {
    let wallet = WalletFile::generate();
    let mut tx = Transaction {
        from: [0; 32],
        to: Some([1; 32]),
        value: 0,
        gas_limit: 1000,
        max_gas_price: 1000,
        mining_tip: tip,
        expiration_height: None,
        payload: Vec::new(),
        account_index: 0,
        nonce,
        public_key: Vec::new(),
        signature: Vec::new(),
    };
    tx = sign_tx(tx, 1, &wallet).unwrap();
    tx
}

fn temp_data_dir(name: &str) -> PathBuf {
    std::env::set_var("COIN_USE_MEMORY_STORE", "1");
    println!("IN temp_data_dir: {}", name);
    let unique = TEST_DIR_COUNTER.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!(
        "coin-test-{}-{}-{}",
        name,
        std::process::id(),
        unique
    ))
}

fn test_config(name: &str) -> NodeConfig {
    println!("IN test_config: {}", name);
    let data_dir = temp_data_dir(name);
    NodeConfig {
        data_dir: data_dir.clone(),
        config_dir: data_dir.join("config"),
        wallet_path: data_dir.join("config").join("wallet.toml"),
        mine: false,
        ..NodeConfig::default()
    }
}

fn empty_child(parent: &Block, nbits: u32, gas_used: u64) -> Block {
    let mut block = Block {
        header: BlockHeader {
            magic: MAGIC,
            protocol_version: PROTOCOL_VERSION,
            chain_id: parent.header.chain_id,
            height: parent.header.height + 1,
            timestamp: parent.header.timestamp + 1,
            prev_block_hash: parent.hash().unwrap(),
            nbits,
            nonce: 0,
            tx_count: 0,
            block_body_size: 0,
            block_gas_limit: parent.header.block_gas_limit,
            gas_price: next_gas_price(
                parent.header.gas_price,
                parent.header.gas_used,
                parent.header.block_gas_limit,
            ),
            gas_used,
            miner_address: [7; 32],
            tx_root: tx_root(&[]).unwrap(),
            receipt_root: receipt_root(&[]).unwrap(),
        },
        transactions: Vec::new(),
    };
    block.header.block_body_size = block.body_bytes().unwrap().len() as u64;
    block
}

fn valid_empty_child(parent: &Block, miner: [u8; 32]) -> Block {
    let height = parent.header.height + 1;
    let reward = block_reward(height);
    let reward_receipt = Receipt {
        tx_hash: [0; 32],
        success: true,
        gas_used: 0,
        gas_burned: 0,
        mining_tip_paid: 0,
        exit_reason: "block_reward".to_string(),
        events: vec![(0, vec![reward.to_string()])],
    };
    let mut block = Block {
        header: BlockHeader {
            magic: MAGIC,
            protocol_version: PROTOCOL_VERSION,
            chain_id: parent.header.chain_id,
            height,
            timestamp: parent.header.timestamp + 1,
            prev_block_hash: parent.hash().unwrap(),
            nbits: parent.header.nbits,
            nonce: 0,
            tx_count: 0,
            block_body_size: 0,
            block_gas_limit: parent.header.block_gas_limit,
            gas_price: next_gas_price(
                parent.header.gas_price,
                parent.header.gas_used,
                parent.header.block_gas_limit,
            ),
            gas_used: 0,
            miner_address: miner,
            tx_root: tx_root(&[]).unwrap(),
            receipt_root: receipt_root(&[reward_receipt]).unwrap(),
        },
        transactions: Vec::new(),
    };
    block.header.block_body_size = block.body_bytes().unwrap().len() as u64;
    let target = nbits_to_target(block.header.nbits);
    while !hash_leq_target(&block.header.hash().unwrap(), &target) {
        block.header.nonce += 1;
    }
    block
}

fn make_head_easy_to_mine(core: &mut ChainCore) {
    let miner = core.cfg.miner_address.unwrap_or([0; 32]);
    let mut header = BlockHeader {
        nbits: 0x20ffffff,
        ..genesis_header(core.cfg.chain_id, miner)
    };
    println!("make_head_easy_to_mine: mining genesis child");
    let target = nbits_to_target(header.nbits);
    while !hash_leq_target(&header.hash().unwrap(), &target) {
        header.nonce += 1;
    }
    println!("make_head_easy_to_mine: done mining");
    let block = Block {
        header,
        transactions: Vec::new(),
    };
    let hash = block.hash().unwrap();
    core.store.put_block(&block).unwrap();
    core.store
        .put_receipts(&BlockReceipts {
            block_hash: hash,
            receipts: Vec::new(),
        })
        .unwrap();
    core.store.set_head(hash, 0).unwrap();
}

fn fund(core: &mut ChainCore, wallet: &WalletFile, balance: u128) {
    let mut diffs = Vec::new();
    core.store
        .put_account(
            &wallet.address().unwrap(),
            &Account {
                balance,
                account_index: 0,
                code_hash: None,
            },
            &mut diffs,
        )
        .unwrap();
}

fn signed_tx(
    core: &ChainCore,
    wallet: &WalletFile,
    to: Option<[u8; 32]>,
    value: u128,
    payload: Vec<u8>,
    account_index: u64,
    nonce: u64,
) -> Transaction {
    sign_tx(
        Transaction {
            from: [0; 32],
            to,
            value,
            gas_limit: 250_000,
            max_gas_price: u128::MAX / 4,
            mining_tip: 17,
            expiration_height: None,
            payload,
            account_index,
            nonce,
            public_key: Vec::new(),
            signature: Vec::new(),
        },
        core.cfg.chain_id,
        wallet,
    )
    .unwrap()
}

fn submit_and_mine(core: &mut ChainCore, tx: Transaction, miner: [u8; 32]) -> Block {
    let result = core.submit_tx(tx, 1);
    assert!(result.accepted, "submit failed: {:?}", result.error);
    core.mine_next_block(miner).unwrap()
}

fn push64(code: &mut Vec<u8>, value: u64) {
    code.push(Opcode::Push64 as u8);
    code.extend_from_slice(&value.to_be_bytes());
}

fn push256(code: &mut Vec<u8>, value: U256) {
    code.push(Opcode::Push256 as u8);
    code.extend_from_slice(&value.to_be_bytes());
}

fn push_addr(code: &mut Vec<u8>, address: [u8; 32]) {
    code.push(Opcode::PushAddr as u8);
    code.extend_from_slice(&address);
}

fn immediate_u16(code: &mut Vec<u8>, value: u16) {
    code.extend_from_slice(&value.to_be_bytes());
}

fn call_payload(kind: ContractCallKind, method_idx: u16, args: Vec<Value>) -> Vec<u8> {
    encode_contract_call(&ContractCallPayload {
        kind,
        method_idx,
        args,
    })
    .unwrap()
}

fn set_state_contract(field: u8) -> Vec<u8> {
    let mut metadata = Metadata::default();
    metadata.methods.insert(1, MethodMeta { args: 1, rets: 0 });
    metadata.jump_table.insert(1, 0);
    encode_contract_blob(&ContractBlob {
        metadata,
        code: vec![Opcode::SetState as u8, field, Opcode::Stop as u8],
    })
    .unwrap()
}

fn reverting_after_write_contract(field: u8) -> Vec<u8> {
    let mut metadata = Metadata::default();
    metadata.methods.insert(1, MethodMeta { args: 1, rets: 0 });
    metadata.jump_table.insert(1, 0);
    let mut code = Vec::new();
    code.push(Opcode::SetState as u8);
    code.push(field);
    push64(&mut code, 1);
    code.push(Opcode::Revert as u8);
    encode_contract_blob(&ContractBlob { metadata, code }).unwrap()
}

fn cast_address_contract(field: u8) -> Vec<u8> {
    let mut metadata = Metadata::default();
    metadata.methods.insert(1, MethodMeta { args: 1, rets: 0 });
    metadata.jump_table.insert(1, 0);
    encode_contract_blob(&ContractBlob {
        metadata,
        code: vec![
            Opcode::CastAddrTo256 as u8,
            Opcode::SetState as u8,
            field,
            Opcode::Stop as u8,
        ],
    })
    .unwrap()
}

fn nft_target_contract(has_interface: bool) -> Vec<u8> {
    let mut metadata = Metadata::default();
    metadata.methods.insert(1, MethodMeta { args: 1, rets: 0 });
    if has_interface {
        metadata
            .interfaces
            .insert(1, MethodMeta { args: 1, rets: 0 });
    }
    metadata.jump_table.insert(1, 0);
    encode_contract_blob(&ContractBlob {
        metadata,
        code: vec![Opcode::SetState as u8, 0, Opcode::Stop as u8],
    })
    .unwrap()
}

fn interface_mint_caller_contract(target: [u8; 32]) -> Vec<u8> {
    let mut metadata = Metadata::default();
    metadata.methods.insert(1, MethodMeta { args: 1, rets: 0 });
    metadata
        .interfaces
        .insert(1, MethodMeta { args: 1, rets: 0 });
    metadata.jump_table.insert(1, 0);
    let mut code = Vec::new();
    code.push(Opcode::StoreLocal as u8);
    code.push(0);
    push64(&mut code, 100_000);
    push_addr(&mut code, target);
    push256(&mut code, U256::ZERO);
    code.push(Opcode::PushLocal as u8);
    code.push(0);
    code.push(Opcode::InvokeInterface as u8);
    immediate_u16(&mut code, 1);
    code.push(Opcode::SetState as u8);
    code.push(9);
    code.push(Opcode::Stop as u8);
    encode_contract_blob(&ContractBlob { metadata, code }).unwrap()
}

fn static_interface_caller_contract(target: [u8; 32]) -> Vec<u8> {
    let mut metadata = Metadata::default();
    metadata.methods.insert(1, MethodMeta { args: 1, rets: 0 });
    metadata
        .interfaces
        .insert(1, MethodMeta { args: 1, rets: 0 });
    metadata.jump_table.insert(1, 0);
    let mut code = Vec::new();
    code.push(Opcode::StoreLocal as u8);
    code.push(0);
    push64(&mut code, 100_000);
    push_addr(&mut code, target);
    code.push(Opcode::PushLocal as u8);
    code.push(0);
    code.push(Opcode::InvokeItfStatic as u8);
    immediate_u16(&mut code, 1);
    code.push(Opcode::SetState as u8);
    code.push(9);
    code.push(Opcode::Stop as u8);
    encode_contract_blob(&ContractBlob { metadata, code }).unwrap()
}

#[test]
fn signed_tx_verifies_and_hash_includes_signature() {
    let wallet = WalletFile::generate();
    let tx = sign_tx(tx(10, 1), 1, &wallet).unwrap();
    assert_eq!(tx.from, address_from_public_key(&tx.public_key));
    tx.verify_signature(1).unwrap();

    let hash = tx.hash().unwrap();
    let mut changed = tx.clone();
    changed.signature[0] ^= 1;
    assert_ne!(hash, changed.hash().unwrap());
}

#[test]
fn end_to_end_wallet_address_signature_and_tamper_rejection() {
    let mut core = ChainCore::open(test_config("e2e-signing")).unwrap();
    make_head_easy_to_mine(&mut core);
    let wallet = WalletFile::generate();
    fund(&mut core, &wallet, 1_000_000_000);

    let tx = signed_tx(&core, &wallet, Some([0x51; 32]), 123, Vec::new(), 0, 99);
    assert_eq!(tx.from, wallet.address().unwrap());
    assert_eq!(tx.from, address_from_public_key(&tx.public_key));
    tx.verify_signature(core.cfg.chain_id).unwrap();
    assert!(tx.verify_signature(core.cfg.chain_id + 1).is_err());

    let mut tampered_value = tx.clone();
    tampered_value.value += 1;
    assert!(!core.submit_tx(tampered_value, 1).accepted);

    let mut tampered_sender = tx.clone();
    tampered_sender.from = [0x99; 32];
    let result = core.submit_tx(tampered_sender, 1);
    assert!(!result.accepted);
    assert!(result
        .error
        .unwrap()
        .contains("sender address does not match public key"));

    let accepted = core.submit_tx(tx.clone(), 1);
    assert!(accepted.accepted, "submit failed: {:?}", accepted.error);
    assert_eq!(
        core.mempool.ordered().unwrap()[0].hash().unwrap(),
        tx.hash().unwrap()
    );
}

#[test]
fn end_to_end_native_transfer_creates_accounts_pays_fees_and_advances_indexes() {
    let mut core = ChainCore::open(test_config("e2e-native-transfer")).unwrap();
    make_head_easy_to_mine(&mut core);
    let wallet = WalletFile::generate();
    let sender = wallet.address().unwrap();
    let receiver = [0x52; 32];
    let miner = [0x53; 32];
    fund(&mut core, &wallet, 1_000_000_000);

    let tx = signed_tx(&core, &wallet, Some(receiver), 12_345, Vec::new(), 0, 1);
    let tx_hash = tx.hash().unwrap();
    let block = submit_and_mine(&mut core, tx, miner);
    let block_hash = block.hash().unwrap();
    let receipts = core.store.get_receipts(&block_hash).unwrap().unwrap();

    assert_eq!(core.store.get_account(&sender).unwrap().account_index, 1);
    assert_eq!(core.store.get_account(&receiver).unwrap().balance, 12_345);
    assert_eq!(
        core.store.get_account(&sender).unwrap().balance,
        1_000_000_000 - 12_345 - 17
    );
    assert_eq!(
        core.store.get_account(&miner).unwrap().balance,
        block_reward(block.header.height) + 17
    );
    assert_eq!(
        core.store.get_account(&[0xAA; 32]).unwrap(),
        Account::default()
    );
    assert!(core.mempool.ordered().unwrap().is_empty());
    assert_eq!(receipts.receipts[0].tx_hash, tx_hash);
    assert!(receipts.receipts[0].success);
    assert_eq!(receipts.receipts[0].exit_reason, "native");
    assert_eq!(receipts.receipts[1].exit_reason, "block_reward");
}

#[test]
fn mempool_orders_by_tip_per_byte_then_hash() {
    let low = tx(1, 1);
    let high = tx(10_000, 2);
    let mut mempool = Mempool::default();
    mempool.insert(low.clone()).unwrap();
    mempool.insert(high.clone()).unwrap();
    let ordered = mempool.ordered().unwrap();
    assert_eq!(ordered[0].hash().unwrap(), high.hash().unwrap());
    assert_eq!(ordered[1].hash().unwrap(), low.hash().unwrap());
}

#[test]
fn gas_price_adjusts_toward_full_blocks() {
    assert_eq!(next_gas_price(GENESIS_GAS_PRICE, 0, 1000), 750);
    assert_eq!(next_gas_price(10, 1000, 1000), 11);
    assert!(next_gas_price(10, 500, 1000) < 10);
    assert_eq!(next_gas_price(1, 0, 1000), MIN_GAS_PRICE);
}

#[test]
fn reward_halves_to_tail_emission() {
    assert_eq!(block_reward(0), BASE_REWARD);
    assert_eq!(block_reward(1_000_000), BASE_REWARD / 2);
    assert_eq!(block_reward(40_000_000), TAIL_REWARD);
}

#[test]
fn compact_nbits_easy_is_easy_enough_for_genesis_mvp() {
    let target = nbits_to_target(0x20ffffff);
    assert!(hash_leq_target(&[0; 32], &target));
}

#[test]
fn scale_target_tightens_and_relaxes_predictably() {
    let initial = nbits_to_target(0x20ffffff);
    assert_eq!(target_to_nbits(&scale_target(initial, 1, 4)), 0x203fffff);
    assert_eq!(target_to_nbits(&scale_target(initial, 4, 1)), 0x20ffffff);
}

#[test]
fn retarget_tightens_when_blocks_are_too_fast() {
    let mut core = ChainCore::open(test_config("retarget-fast-blocks")).unwrap();
    let initial = core.head().unwrap().header.nbits;
    assert_eq!(initial, 0x20ffffff);

    for _ in 0..128 {
        core.mine_next_block([0x44; 32]).unwrap();
    }

    let head = core.head().unwrap();
    assert_eq!(head.header.height, 128);
    assert_eq!(head.header.nbits, 0x203fffff);
    assert!(
        nbits_to_target(head.header.nbits) < nbits_to_target(initial),
        "fast retarget should lower target: initial=0x{initial:08x}, actual=0x{:08x}",
        head.header.nbits
    );
}

#[test]
fn accept_block_rejects_unexpected_nbits() {
    println!("STARTING TEST");
    let cfg = test_config("bad-nbits");
    println!("CFG CREATED: {:?}", cfg.data_dir);
    let mut core = ChainCore::open(cfg).unwrap();
    println!("OPENED CORE");
    let parent = core.head().unwrap();
    println!("GOT HEAD");
    let block = empty_child(&parent, 0, 0);
    println!("CREATED BLOCK");

    let err = core.accept_block(block).unwrap_err().to_string();
    println!("ERR: {}", err);

    assert!(
        err.contains("bad nbits"),
        "unexpected validation error: {err}"
    );
}

#[test]
fn accept_block_rejects_header_gas_used_mismatch() {
    let mut core = ChainCore::open(test_config("gas-used-mismatch")).unwrap();
    make_head_easy_to_mine(&mut core);
    let parent = core.head().unwrap();
    let mut block = empty_child(&parent, parent.header.nbits, 1);
    block.header.gas_used = 1;
    let target = nbits_to_target(parent.header.nbits);
    while !hash_leq_target(&block.header.hash().unwrap(), &target) {
        block.header.nonce += 1;
    }

    let err = core.accept_block(block).unwrap_err().to_string();

    assert!(
        err.contains("gas used mismatch"),
        "unexpected validation error: {err}"
    );
}

#[test]
fn consensus_rejects_bad_block_roots_without_advancing_head() {
    let mut core = ChainCore::open(test_config("bad-block-roots")).unwrap();
    make_head_easy_to_mine(&mut core);
    let parent = core.head().unwrap();
    let parent_hash = parent.hash().unwrap();

    let mut bad_tx_root = valid_empty_child(&parent, [0x81; 32]);
    bad_tx_root.header.tx_root = [0x01; 32];
    let target = nbits_to_target(bad_tx_root.header.nbits);
    while !hash_leq_target(&bad_tx_root.header.hash().unwrap(), &target) {
        bad_tx_root.header.nonce += 1;
    }
    let err = core.accept_block(bad_tx_root).unwrap_err().to_string();
    assert!(err.contains("tx root mismatch"), "err was: {err}");
    assert_eq!(core.head().unwrap().hash().unwrap(), parent_hash);

    let mut bad_receipt_root = valid_empty_child(&parent, [0x82; 32]);
    bad_receipt_root.header.receipt_root = [0x02; 32];
    let target = nbits_to_target(bad_receipt_root.header.nbits);
    while !hash_leq_target(&bad_receipt_root.header.hash().unwrap(), &target) {
        bad_receipt_root.header.nonce += 1;
    }
    let err = core.accept_block(bad_receipt_root).unwrap_err().to_string();
    assert!(err.contains("receipt root mismatch"), "err was: {err}");
    assert_eq!(core.head().unwrap().hash().unwrap(), parent_hash);
    assert_eq!(
        core.store.get_account(&[0x82; 32]).unwrap(),
        Account::default()
    );
}

#[test]
fn execute_block_rejects_fee_overflow() {
    let mut core = ChainCore::open(test_config("fee-overflow")).unwrap();
    make_head_easy_to_mine(&mut core);
    let parent = core.head().unwrap();

    let wallet = WalletFile::generate();
    let mut bad_tx = tx(u128::MAX, 1);
    bad_tx.value = 1;
    bad_tx = sign_tx(bad_tx, core.cfg.chain_id, &wallet).unwrap();

    let mut block = empty_child(&parent, parent.header.nbits, 0);
    block.transactions.push(bad_tx);
    block.header.tx_count = 1;
    block.header.block_body_size = block.body_bytes().unwrap().len() as u64;
    block.header.tx_root = tx_root(&block.transactions).unwrap();
    let target = nbits_to_target(parent.header.nbits);
    while !hash_leq_target(&block.header.hash().unwrap(), &target) {
        block.header.nonce += 1;
    }

    let err = core.accept_block(block).unwrap_err().to_string();
    assert!(
        err.contains("fee overflow")
            || err.contains("block gas exceeded")
            || err.contains("insufficient balance"),
        "err was: {}",
        err
    );
}

#[test]
fn rejected_block_rolls_back_partial_state_changes() {
    let mut core = ChainCore::open(test_config("atomic-block-execution")).unwrap();
    make_head_easy_to_mine(&mut core);
    let parent = core.head().unwrap();

    let wallet = WalletFile::generate();
    let sender = wallet.address().unwrap();
    let mut setup_diffs = Vec::new();
    core.store
        .put_account(
            &sender,
            &Account {
                balance: 2_000_000,
                account_index: 0,
                code_hash: None,
            },
            &mut setup_diffs,
        )
        .unwrap();

    let valid_tx = sign_tx(
        Transaction {
            from: [0; 32],
            to: Some([2; 32]),
            value: 100,
            gas_limit: 1000,
            max_gas_price: 1000,
            mining_tip: 10,
            expiration_height: None,
            payload: Vec::new(),
            account_index: 0,
            nonce: 1,
            public_key: Vec::new(),
            signature: Vec::new(),
        },
        core.cfg.chain_id,
        &wallet,
    )
    .unwrap();
    let invalid_tx = sign_tx(
        Transaction {
            account_index: 0,
            nonce: 2,
            ..valid_tx.clone()
        },
        core.cfg.chain_id,
        &wallet,
    )
    .unwrap();

    let mut block = empty_child(&parent, parent.header.nbits, 0);
    block.transactions = vec![valid_tx, invalid_tx];
    block.header.tx_count = block.transactions.len() as u32;
    block.header.block_body_size = block.body_bytes().unwrap().len() as u64;
    block.header.tx_root = tx_root(&block.transactions).unwrap();
    let target = nbits_to_target(parent.header.nbits);
    while !hash_leq_target(&block.header.hash().unwrap(), &target) {
        block.header.nonce += 1;
    }

    let err = core.accept_block(block).unwrap_err().to_string();

    assert!(err.contains("account index mismatch"), "err was: {err}");
    assert_eq!(core.store.get_account(&sender).unwrap().balance, 2_000_000);
    assert_eq!(core.store.get_account(&sender).unwrap().account_index, 0);
    assert_eq!(core.store.get_account(&[2; 32]).unwrap().balance, 0);
    assert_eq!(core.store.get_account(&[7; 32]).unwrap().balance, 0);
}

#[test]
fn mining_writes_block_and_receipt_files() {
    let mut core = ChainCore::open(test_config("mine-files")).unwrap();
    make_head_easy_to_mine(&mut core);

    let block = core
        .mine_next_block(core.cfg.miner_address.unwrap_or([0; 32]))
        .unwrap();
    let hash = block.hash().unwrap();
    let blocks_dir = core.store.data_dir.join("blocks");
    let receipts_dir = core.store.data_dir.join("receipts");

    assert!(blocks_dir
        .join(format!("{:020}.block", block.header.height))
        .exists());
    assert!(blocks_dir
        .join(format!("{}.block", hex_hash(&hash)))
        .exists());
    assert!(receipts_dir
        .join(format!("{}.receipt", hex_hash(&hash)))
        .exists());
}

#[test]
fn accept_block_rejects_expired_transaction() {
    let mut core = ChainCore::open(test_config("expired-tx")).unwrap();
    make_head_easy_to_mine(&mut core);
    let parent = core.head().unwrap();

    let wallet = WalletFile::generate();
    let sender = wallet.address().unwrap();
    let mut setup_diffs = Vec::new();
    core.store
        .put_account(
            &sender,
            &Account {
                balance: 2_000_000,
                account_index: 0,
                code_hash: None,
            },
            &mut setup_diffs,
        )
        .unwrap();

    let tx = sign_tx(
        Transaction {
            from: [0; 32],
            to: Some([3; 32]),
            value: 1,
            gas_limit: 1000,
            max_gas_price: 1000,
            mining_tip: 10,
            expiration_height: Some(parent.header.height),
            payload: Vec::new(),
            account_index: 0,
            nonce: 1,
            public_key: Vec::new(),
            signature: Vec::new(),
        },
        core.cfg.chain_id,
        &wallet,
    )
    .unwrap();

    core.mempool.insert(tx.clone()).unwrap();
    assert_eq!(core.mempool.ordered().unwrap().len(), 1);

    let mut block = empty_child(&parent, parent.header.nbits, 0);
    block.transactions.push(tx);
    block.header.tx_count = 1;
    block.header.block_body_size = block.body_bytes().unwrap().len() as u64;
    block.header.tx_root = tx_root(&block.transactions).unwrap();
    let target = nbits_to_target(parent.header.nbits);
    while !hash_leq_target(&block.header.hash().unwrap(), &target) {
        block.header.nonce += 1;
    }

    let err = core.accept_block(block).unwrap_err().to_string();
    assert!(err.contains("transaction expired"), "err was: {err}");
    assert_eq!(core.store.get_account(&sender).unwrap().balance, 2_000_000);
    assert_eq!(core.mempool.ordered().unwrap().len(), 1);
}

#[test]
fn submit_tx_rejects_empty_contract_deployment() {
    let mut core = ChainCore::open(test_config("empty-deploy-submit")).unwrap();
    let wallet = WalletFile::generate();
    let sender = wallet.address().unwrap();
    let mut setup_diffs = Vec::new();
    core.store
        .put_account(
            &sender,
            &Account {
                balance: 2_000_000,
                account_index: 0,
                code_hash: None,
            },
            &mut setup_diffs,
        )
        .unwrap();

    let tx = sign_tx(
        Transaction {
            from: [0; 32],
            to: None,
            value: 0,
            gas_limit: 1000,
            max_gas_price: 1000,
            mining_tip: 10,
            expiration_height: None,
            payload: Vec::new(),
            account_index: 0,
            nonce: 1,
            public_key: Vec::new(),
            signature: Vec::new(),
        },
        core.cfg.chain_id,
        &wallet,
    )
    .unwrap();

    let result = core.submit_tx(tx, 0);

    assert!(!result.accepted);
    assert!(result
        .error
        .unwrap()
        .contains("contract bytecode must be non-empty"));
}

#[test]
fn submit_tx_rejects_non_lvm1_contract_deployment() {
    let mut core = ChainCore::open(test_config("raw-deploy-submit")).unwrap();
    let wallet = WalletFile::generate();
    fund(&mut core, &wallet, 2_000_000);

    let tx = signed_tx(&core, &wallet, None, 0, vec![Opcode::Stop as u8], 0, 1);
    let result = core.submit_tx(tx, 0);

    assert!(!result.accepted);
    assert!(result
        .error
        .unwrap()
        .contains("contract bytecode must be LVM1"));
}

#[test]
fn end_to_end_contract_deploy_call_revert_refunds_and_preserves_state() {
    let mut core = ChainCore::open(test_config("e2e-contract-state")).unwrap();
    make_head_easy_to_mine(&mut core);
    let wallet = WalletFile::generate();
    let sender = wallet.address().unwrap();
    let miner = [0x61; 32];
    let alice = [0x62; 32];
    let bob = [0x63; 32];
    fund(&mut core, &wallet, 2_000_000_000);

    let deploy_payload = set_state_contract(0);
    let contract = contract_address(&sender, 0);
    let deploy = signed_tx(&core, &wallet, None, 777, deploy_payload.clone(), 0, 1);
    submit_and_mine(&mut core, deploy, miner);

    let contract_account = core.store.get_account(&contract).unwrap();
    assert_eq!(contract_account.balance, 777);
    assert_eq!(contract_account.code_hash, Some(sha3_256(&deploy_payload)));
    assert_eq!(
        core.store
            .code(&contract_account.code_hash.unwrap())
            .unwrap()
            .unwrap(),
        deploy_payload
    );

    let call = signed_tx(
        &core,
        &wallet,
        Some(contract),
        0,
        call_payload(ContractCallKind::Method, 1, vec![Value::Address(alice)]),
        1,
        2,
    );
    let call_block = submit_and_mine(&mut core, call, miner);
    let receipts = core
        .store
        .get_receipts(&call_block.hash().unwrap())
        .unwrap()
        .unwrap();
    assert!(receipts.receipts[0].success);
    assert_eq!(
        core.store.get_vm_state_value(&contract, 0),
        Value::Address(alice)
    );

    let reverting_payload = reverting_after_write_contract(0);
    let reverting_contract = contract_address(&sender, 2);
    let deploy_reverting = signed_tx(&core, &wallet, None, 0, reverting_payload, 2, 3);
    submit_and_mine(&mut core, deploy_reverting, miner);

    let before_sender = core.store.get_account(&sender).unwrap().balance;
    let failed_value = 555;
    let revert_call = signed_tx(
        &core,
        &wallet,
        Some(reverting_contract),
        failed_value,
        call_payload(ContractCallKind::Method, 1, vec![Value::Address(bob)]),
        3,
        4,
    );
    let failed_block = submit_and_mine(&mut core, revert_call, miner);
    let failed_receipts = core
        .store
        .get_receipts(&failed_block.hash().unwrap())
        .unwrap()
        .unwrap();

    assert!(!failed_receipts.receipts[0].success);
    assert!(failed_receipts.receipts[0].exit_reason.contains("Revert"));
    assert_eq!(
        core.store.get_vm_state_value(&reverting_contract, 0),
        Value::U64(0)
    );
    assert_eq!(
        core.store.get_account(&reverting_contract).unwrap().balance,
        0
    );
    assert_eq!(
        core.store.get_account(&sender).unwrap().balance,
        before_sender - 17 - failed_receipts.receipts[0].gas_burned
    );
}

#[test]
fn end_to_end_contract_call_rejects_structurally_invalid_lcall1() {
    let mut core = ChainCore::open(test_config("e2e-bad-lcall")).unwrap();
    make_head_easy_to_mine(&mut core);
    let wallet = WalletFile::generate();
    let sender = wallet.address().unwrap();
    let miner = [0x64; 32];
    fund(&mut core, &wallet, 2_000_000_000);

    let contract = contract_address(&sender, 0);
    let deploy = signed_tx(&core, &wallet, None, 0, cast_address_contract(0), 0, 1);
    submit_and_mine(&mut core, deploy, miner);

    let raw_payload = signed_tx(&core, &wallet, Some(contract), 0, vec![1, 2, 3], 1, 2);
    let result = core.submit_tx(raw_payload, 1);
    assert!(result.accepted, "submit failed: {:?}", result.error);
    let err = core.mine_next_block(miner).unwrap_err().to_string();
    assert!(
        err.contains("contract call payload must be LCALL1"),
        "err was: {err}"
    );
    assert_eq!(core.store.get_account(&sender).unwrap().account_index, 1);
    assert_eq!(core.store.get_vm_state_value(&contract, 0), Value::U64(0));

    core.mempool = Mempool::default();
    let missing_method = signed_tx(
        &core,
        &wallet,
        Some(contract),
        0,
        call_payload(
            ContractCallKind::Method,
            9,
            vec![Value::Address([0x65; 32])],
        ),
        1,
        3,
    );
    let result = core.submit_tx(missing_method, 1);
    assert!(result.accepted, "submit failed: {:?}", result.error);
    let err = core.mine_next_block(miner).unwrap_err().to_string();
    assert!(
        err.contains("contract call target method not found"),
        "err was: {err}"
    );
    assert_eq!(core.store.get_account(&sender).unwrap().account_index, 1);
}

#[test]
fn end_to_end_contract_runtime_type_error_is_receipted_failure() {
    let mut core = ChainCore::open(test_config("e2e-runtime-type-error")).unwrap();
    make_head_easy_to_mine(&mut core);
    let wallet = WalletFile::generate();
    let sender = wallet.address().unwrap();
    let miner = [0x66; 32];
    fund(&mut core, &wallet, 2_000_000_000);

    let contract = contract_address(&sender, 0);
    let deploy = signed_tx(&core, &wallet, None, 0, cast_address_contract(0), 0, 1);
    submit_and_mine(&mut core, deploy, miner);

    let bad_arg = signed_tx(
        &core,
        &wallet,
        Some(contract),
        0,
        call_payload(ContractCallKind::Method, 1, vec![Value::U64(123)]),
        1,
        2,
    );
    let block = submit_and_mine(&mut core, bad_arg, miner);
    let receipts = core
        .store
        .get_receipts(&block.hash().unwrap())
        .unwrap()
        .unwrap();

    assert!(!receipts.receipts[0].success);
    assert!(receipts.receipts[0].exit_reason.contains("TypeError"));
    assert_eq!(core.store.get_vm_state_value(&contract, 0), Value::U64(0));
    assert_eq!(core.store.get_account(&sender).unwrap().account_index, 2);
}

#[test]
fn end_to_end_deployed_interface_call_executes_nested_vm_and_handles_missing_interface() {
    let mut core = ChainCore::open(test_config("e2e-interface-call")).unwrap();
    make_head_easy_to_mine(&mut core);
    let wallet = WalletFile::generate();
    let sender = wallet.address().unwrap();
    let miner = [0x71; 32];
    let alice = [0x72; 32];
    let bob = [0x73; 32];
    fund(&mut core, &wallet, 3_000_000_000);

    let nft = contract_address(&sender, 0);
    let deploy_nft = signed_tx(&core, &wallet, None, 0, nft_target_contract(true), 0, 1);
    submit_and_mine(&mut core, deploy_nft, miner);

    let caller = contract_address(&sender, 1);
    let deploy_caller = signed_tx(
        &core,
        &wallet,
        None,
        0,
        interface_mint_caller_contract(nft),
        1,
        2,
    );
    submit_and_mine(&mut core, deploy_caller, miner);

    let call_tx = signed_tx(
        &core,
        &wallet,
        Some(caller),
        0,
        call_payload(ContractCallKind::Method, 1, vec![Value::Address(alice)]),
        2,
        3,
    );
    let call_block = submit_and_mine(&mut core, call_tx, miner);
    let receipts = core
        .store
        .get_receipts(&call_block.hash().unwrap())
        .unwrap()
        .unwrap();
    assert!(receipts.receipts[0].success);
    assert_eq!(
        core.store.get_vm_state_value(&nft, 0),
        Value::Address(alice)
    );
    assert_eq!(core.store.get_vm_state_value(&caller, 9), Value::U64(1));

    let no_interface = contract_address(&sender, 3);
    let deploy_no_interface = signed_tx(&core, &wallet, None, 0, nft_target_contract(false), 3, 4);
    submit_and_mine(&mut core, deploy_no_interface, miner);

    let bad_caller = contract_address(&sender, 4);
    let deploy_bad_caller = signed_tx(
        &core,
        &wallet,
        None,
        0,
        interface_mint_caller_contract(no_interface),
        4,
        5,
    );
    submit_and_mine(&mut core, deploy_bad_caller, miner);

    let bad_call_tx = signed_tx(
        &core,
        &wallet,
        Some(bad_caller),
        0,
        call_payload(ContractCallKind::Method, 1, vec![Value::Address(bob)]),
        5,
        6,
    );
    let bad_call_block = submit_and_mine(&mut core, bad_call_tx, miner);
    let bad_receipts = core
        .store
        .get_receipts(&bad_call_block.hash().unwrap())
        .unwrap()
        .unwrap();
    assert!(bad_receipts.receipts[0].success);
    assert_eq!(core.store.get_vm_state_value(&bad_caller, 9), Value::U64(0));
    assert_eq!(
        core.store.get_vm_state_value(&no_interface, 0),
        Value::U64(0)
    );
}

#[test]
fn end_to_end_contract_edge_cases_reject_non_contract_calls_and_static_writes() {
    let mut core = ChainCore::open(test_config("e2e-contract-edges")).unwrap();
    make_head_easy_to_mine(&mut core);
    let wallet = WalletFile::generate();
    let sender = wallet.address().unwrap();
    let miner = [0x91; 32];
    let plain_account = [0x92; 32];
    let alice = [0x93; 32];
    fund(&mut core, &wallet, 3_000_000_000);

    let mut diffs = Vec::new();
    core.store
        .put_account(
            &plain_account,
            &Account {
                balance: 100,
                account_index: 0,
                code_hash: None,
            },
            &mut diffs,
        )
        .unwrap();
    let bad_call = signed_tx(&core, &wallet, Some(plain_account), 0, vec![1], 0, 1);
    let result = core.submit_tx(bad_call, 1);
    assert!(result.accepted, "submit failed: {:?}", result.error);
    let err = core.mine_next_block(miner).unwrap_err().to_string();
    assert!(
        err.contains("target has no contract code"),
        "err was: {err}"
    );
    assert_eq!(core.store.get_account(&sender).unwrap().account_index, 0);
    assert_eq!(core.store.get_account(&plain_account).unwrap().balance, 100);

    core.mempool = Mempool::default();
    let nft = contract_address(&sender, 0);
    let deploy_nft = signed_tx(&core, &wallet, None, 0, nft_target_contract(true), 0, 2);
    submit_and_mine(&mut core, deploy_nft, miner);

    let static_caller = contract_address(&sender, 1);
    let deploy_static_caller = signed_tx(
        &core,
        &wallet,
        None,
        0,
        static_interface_caller_contract(nft),
        1,
        3,
    );
    submit_and_mine(&mut core, deploy_static_caller, miner);

    let static_call = signed_tx(
        &core,
        &wallet,
        Some(static_caller),
        0,
        call_payload(ContractCallKind::Method, 1, vec![Value::Address(alice)]),
        2,
        4,
    );
    let block = submit_and_mine(&mut core, static_call, miner);
    let receipts = core
        .store
        .get_receipts(&block.hash().unwrap())
        .unwrap()
        .unwrap();
    assert!(receipts.receipts[0].success);
    assert_eq!(core.store.get_vm_state_value(&nft, 0), Value::U64(0));
    assert_eq!(
        core.store.get_vm_state_value(&static_caller, 9),
        Value::U64(0)
    );
}

#[test]
fn accept_block_rejects_transaction_gas_limits_above_block_limit() {
    let mut core = ChainCore::open(test_config("tx-gas-limit-exceeded")).unwrap();
    make_head_easy_to_mine(&mut core);
    let parent = core.head().unwrap();
    let wallet = WalletFile::generate();
    let mut tx = tx(10, 1);
    tx.gas_limit = parent.header.block_gas_limit + 1;
    tx = sign_tx(tx, core.cfg.chain_id, &wallet).unwrap();

    let mut block = empty_child(&parent, parent.header.nbits, 0);
    block.transactions.push(tx);
    block.header.tx_count = 1;
    block.header.block_body_size = block.body_bytes().unwrap().len() as u64;
    block.header.tx_root = tx_root(&block.transactions).unwrap();
    let target = nbits_to_target(parent.header.nbits);
    while !hash_leq_target(&block.header.hash().unwrap(), &target) {
        block.header.nonce += 1;
    }

    let err = core.accept_block(block).unwrap_err().to_string();

    assert!(
        err.contains("transaction gas limits exceed block gas limit"),
        "err was: {err}"
    );
}

#[test]
fn accept_block_rejects_tx_below_block_gas_price() {
    let mut core = ChainCore::open(test_config("tx-max-gas-price-low")).unwrap();
    make_head_easy_to_mine(&mut core);
    let parent = core.head().unwrap();
    let wallet = WalletFile::generate();
    let sender = wallet.address().unwrap();
    let mut setup_diffs = Vec::new();
    core.store
        .put_account(
            &sender,
            &Account {
                balance: 2_000_000,
                account_index: 0,
                code_hash: None,
            },
            &mut setup_diffs,
        )
        .unwrap();

    let mut low_price_tx = tx(10, 1);
    low_price_tx.max_gas_price = 1;
    low_price_tx = sign_tx(low_price_tx, core.cfg.chain_id, &wallet).unwrap();

    let mut block = empty_child(&parent, parent.header.nbits, 0);
    block.transactions.push(low_price_tx);
    block.header.tx_count = 1;
    block.header.block_body_size = block.body_bytes().unwrap().len() as u64;
    block.header.tx_root = tx_root(&block.transactions).unwrap();
    let target = nbits_to_target(parent.header.nbits);
    while !hash_leq_target(&block.header.hash().unwrap(), &target) {
        block.header.nonce += 1;
    }

    let err = core.accept_block(block).unwrap_err().to_string();

    assert!(
        err.contains("max gas price below block gas price"),
        "err was: {err}"
    );
}
