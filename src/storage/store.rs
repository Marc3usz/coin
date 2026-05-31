use crate::crypto::{hex_hash, Address, Hash};
use crate::types::{Account, Block, BlockReceipts, StateDiff, Transaction};
use crate::{CallRequest, CallResult, ExitReason, StateDB, Value};
use ethnum::U256;
use rocksdb::{Options, DB};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::RwLock;

use super::files::{file_bytes, verify_prefixed_file, write_file_with_prefix};
use super::keys::*;

pub struct ChainStore {
    db: Option<DB>,
    mem: Option<RwLock<BTreeMap<Vec<u8>, Vec<u8>>>>,
    pub data_dir: PathBuf,
}

impl ChainStore {
    pub fn open(data_dir: impl AsRef<Path>) -> anyhow::Result<Self> {
        let data_dir = data_dir.as_ref().to_path_buf();
        eprintln!("[ChainStore] data_dir={}", data_dir.display());
        eprintln!("[ChainStore] create rocksdb dir");
        std::fs::create_dir_all(data_dir.join("rocksdb"))?;
        eprintln!("[ChainStore] create blocks dir");
        std::fs::create_dir_all(data_dir.join("blocks"))?;
        eprintln!("[ChainStore] create receipts dir");
        std::fs::create_dir_all(data_dir.join("receipts"))?;

        if std::env::var("COIN_USE_MEMORY_STORE").is_ok() {
            return Ok(Self {
                db: None,
                mem: Some(RwLock::new(BTreeMap::new())),
                data_dir,
            });
        }

        eprintln!("[ChainStore] open rocksdb");
        let mut opts = Options::default();
        opts.create_if_missing(true);
        eprintln!("[ChainStore] call DB::open");
        let db = DB::open(&opts, data_dir.join("rocksdb")).map_err(|err| {
            anyhow::anyhow!(
                "failed to open RocksDB at {}: {err}",
                data_dir.join("rocksdb").display()
            )
        })?;
        eprintln!("[ChainStore] DB::open ok");
        Ok(Self {
            db: Some(db),
            mem: None,
            data_dir,
        })
    }

    fn db_get(&self, key: &[u8]) -> anyhow::Result<Option<Vec<u8>>> {
        if let Some(mem) = &self.mem {
            Ok(mem.read().unwrap().get(key).cloned())
        } else if let Some(db) = &self.db {
            db.get(key).map_err(Into::into)
        } else {
            unreachable!()
        }
    }

    fn db_put(&self, key: &[u8], value: &[u8]) -> anyhow::Result<()> {
        if let Some(mem) = &self.mem {
            mem.write().unwrap().insert(key.to_vec(), value.to_vec());
            Ok(())
        } else if let Some(db) = &self.db {
            db.put(key, value).map_err(Into::into)
        } else {
            unreachable!()
        }
    }

    fn db_delete(&self, key: &[u8]) -> anyhow::Result<()> {
        if let Some(mem) = &self.mem {
            mem.write().unwrap().remove(key);
            Ok(())
        } else if let Some(db) = &self.db {
            db.delete(key).map_err(Into::into)
        } else {
            unreachable!()
        }
    }

    pub fn head_hash(&self) -> anyhow::Result<Option<Hash>> {
        self.db_get(KEY_HEAD)?.map(|v| decode_32(&v)).transpose()
    }

    pub fn height(&self) -> anyhow::Result<u64> {
        Ok(self
            .db_get(KEY_HEIGHT)?
            .map(|v| decode_u64(&v))
            .transpose()?
            .unwrap_or(0))
    }

    pub fn set_head(&self, hash: Hash, height: u64) -> anyhow::Result<()> {
        self.db_put(KEY_HEAD, &hash)?;
        self.db_put(KEY_HEIGHT, &height.to_be_bytes())?;
        Ok(())
    }

    pub fn get_account(&self, address: &Address) -> anyhow::Result<Account> {
        Ok(self
            .db_get(&account_key(address))?
            .map(|v| bincode::deserialize(&v))
            .transpose()?
            .unwrap_or_default())
    }

    pub fn put_account(
        &self,
        address: &Address,
        account: &Account,
        diffs: &mut Vec<StateDiff>,
    ) -> anyhow::Result<()> {
        let key = account_key(address);
        let before = self.db_get(&key)?;
        let after = bincode::serialize(account)?;
        self.db_put(&key, &after)?;
        diffs.push(StateDiff {
            key,
            before,
            after: Some(after),
        });
        Ok(())
    }

    pub fn code(&self, hash: &Hash) -> anyhow::Result<Option<Vec<u8>>> {
        self.db_get(&code_key(hash))
    }

    pub fn put_code(
        &self,
        hash: &Hash,
        code: &[u8],
        diffs: &mut Vec<StateDiff>,
    ) -> anyhow::Result<()> {
        let key = code_key(hash);
        let before = self.db_get(&key)?;
        self.db_put(&key, code)?;
        diffs.push(StateDiff {
            key,
            before,
            after: Some(code.to_vec()),
        });
        Ok(())
    }

    pub fn put_block(&self, block: &Block) -> anyhow::Result<()> {
        let hash = block.hash()?;
        let bytes = bincode::serialize(block)?;
        let height_path = self
            .data_dir
            .join("blocks")
            .join(format!("{:020}.block", block.header.height));
        let hash_path = self
            .data_dir
            .join("blocks")
            .join(format!("{}.block", hex_hash(&hash)));
        write_file_with_prefix(&height_path, b"CBLK", &bytes)?;
        if !hash_path.exists() {
            std::fs::hard_link(&height_path, &hash_path)
                .or_else(|_| std::fs::write(&hash_path, file_bytes(b"CBLK", &bytes)))?;
        }
        self.db_put(&block_hash_key(&hash), &bytes)?;
        self.db_put(&height_key(block.header.height), &hash)?;
        Ok(())
    }

    pub fn put_block_by_hash_only(&self, block: &Block) -> anyhow::Result<()> {
        let hash = block.hash()?;
        let bytes = bincode::serialize(block)?;
        let hash_path = self
            .data_dir
            .join("blocks")
            .join(format!("{}.block", hex_hash(&hash)));
        if !hash_path.exists() {
            std::fs::write(&hash_path, file_bytes(b"CBLK", &bytes))?;
        }
        self.db_put(&block_hash_key(&hash), &bytes)?;
        Ok(())
    }

    pub fn get_block_by_hash(&self, hash: &Hash) -> anyhow::Result<Option<Block>> {
        self.db_get(&block_hash_key(hash))?
            .map(|v| bincode::deserialize(&v).map_err(Into::into))
            .transpose()
    }

    pub fn get_block_by_height(&self, height: u64) -> anyhow::Result<Option<Block>> {
        let Some(hash) = self.db_get(&height_key(height))? else {
            return Ok(None);
        };
        self.get_block_by_hash(&decode_32(&hash)?)
    }

    pub fn put_receipts(&self, receipts: &BlockReceipts) -> anyhow::Result<()> {
        let bytes = bincode::serialize(receipts)?;
        let path = self
            .data_dir
            .join("receipts")
            .join(format!("{}.receipt", hex_hash(&receipts.block_hash)));
        write_file_with_prefix(&path, b"CRCP", &bytes)?;
        self.db_put(&receipt_key(&receipts.block_hash), &bytes)?;
        Ok(())
    }

    pub fn get_receipts(&self, block_hash: &Hash) -> anyhow::Result<Option<BlockReceipts>> {
        self.db_get(&receipt_key(block_hash))?
            .map(|v| bincode::deserialize(&v).map_err(Into::into))
            .transpose()
    }

    pub fn put_diffs(&self, block_hash: &Hash, diffs: &[StateDiff]) -> anyhow::Result<()> {
        self.db_put(&diff_key(block_hash), &bincode::serialize(diffs)?)?;
        Ok(())
    }

    pub fn get_diffs(&self, block_hash: &Hash) -> anyhow::Result<Option<Vec<StateDiff>>> {
        self.db_get(&diff_key(block_hash))?
            .map(|v| bincode::deserialize(&v).map_err(Into::into))
            .transpose()
    }

    pub fn rollback_diffs(&self, diffs: &[StateDiff]) -> anyhow::Result<()> {
        for diff in diffs.iter().rev() {
            match &diff.before {
                Some(value) => self.db_put(&diff.key, value)?,
                None => self.db_delete(&diff.key)?,
            }
        }
        Ok(())
    }

    pub fn get_vm_state_value(&self, address: &Address, field_idx: u8) -> Value {
        self.db_get(&vm_state_key(address, field_idx))
            .ok()
            .flatten()
            .and_then(|v| bincode::deserialize(&v).ok())
            .unwrap_or(Value::U64(0))
    }

    pub fn put_vm_state_value(
        &self,
        address: &Address,
        field_idx: u8,
        value: &Value,
        diffs: &mut Vec<StateDiff>,
    ) -> anyhow::Result<()> {
        let key = vm_state_key(address, field_idx);
        let before = self.db_get(&key)?;
        let after = bincode::serialize(value)?;
        self.db_put(&key, &after)?;
        diffs.push(StateDiff {
            key,
            before,
            after: Some(after),
        });
        Ok(())
    }

    pub fn put_mempool_tx(&self, tx: &Transaction) -> anyhow::Result<()> {
        self.db_put(&mempool_key(&tx.hash()?), &bincode::serialize(tx)?)?;
        Ok(())
    }

    pub fn remove_mempool_tx(&self, hash: &Hash) -> anyhow::Result<()> {
        self.db_delete(&mempool_key(hash))?;
        Ok(())
    }

    pub fn verify_files(&self) -> anyhow::Result<()> {
        for dir in ["blocks", "receipts"] {
            let dir = self.data_dir.join(dir);
            if !dir.exists() {
                continue;
            }
            for entry in std::fs::read_dir(dir)? {
                verify_prefixed_file(&entry?.path())?;
            }
        }
        Ok(())
    }
}

impl StateDB for ChainStore {
    fn get_state(&mut self, address: &[u8; 32], field_idx: u8) -> Value {
        self.get_vm_state_value(address, field_idx)
    }

    fn set_state(
        &mut self,
        address: &[u8; 32],
        field_idx: u8,
        value: Value,
    ) -> Result<(), ExitReason> {
        self.db_put(
            &vm_state_key(address, field_idx),
            &bincode::serialize(&value).map_err(|_| ExitReason::TypeError)?,
        )
        .map_err(|_| ExitReason::ContractNotFound)
    }

    fn get_balance(&self, address: &[u8; 32]) -> U256 {
        self.get_account(address)
            .map(|a| U256::from(a.balance))
            .unwrap_or(U256::ZERO)
    }

    fn call_contract(&mut self, request: CallRequest) -> Result<CallResult, ExitReason> {
        crate::vm::call_contract_with_overlay(self, request)
    }
}
