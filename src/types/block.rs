use crate::crypto::{merkle_root, sha3_256, triple_sha3_256, Address, Hash};
use serde::{Deserialize, Serialize};

use super::{Amount, Transaction, GENESIS_GAS_PRICE, MAGIC, PROTOCOL_VERSION};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct BlockHeader {
    pub magic: u32,
    pub protocol_version: u16,
    pub chain_id: u64,
    pub height: u64,
    pub timestamp: u64,
    pub prev_block_hash: Hash,
    pub nbits: u32,
    pub nonce: u64,
    pub tx_count: u32,
    pub block_body_size: u64,
    pub block_gas_limit: u64,
    pub gas_price: Amount,
    pub gas_used: u64,
    pub miner_address: Address,
    pub tx_root: Hash,
    pub receipt_root: Hash,
}

impl BlockHeader {
    pub fn hash(&self) -> anyhow::Result<Hash> {
        Ok(triple_sha3_256(&bincode::serialize(self)?))
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Block {
    pub header: BlockHeader,
    pub transactions: Vec<Transaction>,
}

impl Block {
    pub fn body_bytes(&self) -> anyhow::Result<Vec<u8>> {
        Ok(bincode::serialize(&self.transactions)?)
    }

    pub fn hash(&self) -> anyhow::Result<Hash> {
        self.header.hash()
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Receipt {
    pub tx_hash: Hash,
    pub success: bool,
    pub gas_used: u64,
    pub gas_burned: Amount,
    pub mining_tip_paid: Amount,
    pub exit_reason: String,
    pub events: Vec<(u16, Vec<String>)>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct BlockReceipts {
    pub block_hash: Hash,
    pub receipts: Vec<Receipt>,
}

pub fn tx_root(txs: &[Transaction]) -> anyhow::Result<Hash> {
    let hashes = txs
        .iter()
        .map(Transaction::hash)
        .collect::<anyhow::Result<Vec<_>>>()?;
    Ok(merkle_root(hashes))
}

pub fn receipt_root(receipts: &[Receipt]) -> anyhow::Result<Hash> {
    let hashes = receipts
        .iter()
        .map(|r| Ok(sha3_256(&bincode::serialize(r)?)))
        .collect::<anyhow::Result<Vec<_>>>()?;
    Ok(merkle_root(hashes))
}

pub fn genesis_header(chain_id: u64, _miner_address: Address) -> BlockHeader {
    BlockHeader {
        magic: MAGIC,
        protocol_version: PROTOCOL_VERSION,
        chain_id,
        height: 0,
        timestamp: 1,
        prev_block_hash: [0; 32],
        nbits: 0x20ffffff,
        nonce: 0,
        tx_count: 0,
        block_body_size: 0,
        block_gas_limit: 30_000_000,
        gas_price: GENESIS_GAS_PRICE,
        gas_used: 0,
        miner_address: [0; 32],
        tx_root: merkle_root(vec![]),
        receipt_root: merkle_root(vec![]),
    }
}
