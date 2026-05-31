use crate::config::NodeConfig;
use crate::crypto::{
    hash_leq_target, merkle_root, nbits_to_target, scale_target, sha3_256, target_to_nbits,
    Address, Hash,
};
use crate::mempool::Mempool;
use crate::storage::ChainStore;
use crate::types::*;
use crate::vm::{decode_contract_blob, execute_contract_tx, VmBlockContext};
use std::time::{SystemTime, UNIX_EPOCH};

pub struct ChainCore {
    pub cfg: NodeConfig,
    pub store: ChainStore,
    pub mempool: Mempool,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct SubmitResult {
    pub accepted: bool,
    pub tx_hash: Option<Hash>,
    pub warning: Option<String>,
    pub error: Option<String>,
}

impl ChainCore {
    pub fn open(cfg: NodeConfig) -> anyhow::Result<Self> {
        println!("  [ChainCore] ensure_dirs");
        cfg.ensure_dirs()?;
        println!("  [ChainCore] ChainStore::open");
        let store = ChainStore::open(&cfg.data_dir)?;
        println!("  [ChainCore] verify_files");
        store.verify_files()?;
        let mut core = Self {
            cfg,
            store,
            mempool: Mempool::default(),
        };
        println!("  [ChainCore] ensure_genesis");
        core.ensure_genesis()?;
        println!("  [ChainCore] Ok(core)");
        Ok(core)
    }

    pub fn ensure_genesis(&mut self) -> anyhow::Result<()> {
        if self.store.head_hash()?.is_some() {
            return Ok(());
        }
        let miner = self.cfg.miner_address.unwrap_or([0; 32]);
        let block = Block {
            header: genesis_header(self.cfg.chain_id, miner),
            transactions: Vec::new(),
        };
        let hash = block.hash()?;
        self.store.put_block(&block)?;
        self.store.put_receipts(&BlockReceipts {
            block_hash: hash,
            receipts: Vec::new(),
        })?;
        self.store.set_head(hash, 0)?;
        Ok(())
    }

    pub fn head(&self) -> anyhow::Result<Block> {
        let hash = self
            .store
            .head_hash()?
            .ok_or_else(|| anyhow::anyhow!("missing head"))?;
        self.store
            .get_block_by_hash(&hash)?
            .ok_or_else(|| anyhow::anyhow!("head block missing"))
    }

    pub fn submit_tx(&mut self, tx: Transaction, synced_peers: usize) -> SubmitResult {
        match self
            .validate_tx_stateless(&tx)
            .and_then(|_| self.validate_tx_against_state(&tx))
        {
            Ok(()) => match tx.hash().and_then(|hash| {
                self.mempool.insert(tx.clone())?;
                self.store.put_mempool_tx(&tx)?;
                Ok(hash)
            }) {
                Ok(hash) => SubmitResult {
                    accepted: true,
                    tx_hash: Some(hash),
                    warning: (synced_peers == 0)
                        .then(|| "accepted locally but no synced peers are connected".to_string()),
                    error: None,
                },
                Err(err) => SubmitResult {
                    accepted: false,
                    tx_hash: None,
                    warning: None,
                    error: Some(err.to_string()),
                },
            },
            Err(err) => SubmitResult {
                accepted: false,
                tx_hash: None,
                warning: None,
                error: Some(err.to_string()),
            },
        }
    }

    pub fn validate_tx_stateless(&self, tx: &Transaction) -> anyhow::Result<()> {
        tx.verify_signature(self.cfg.chain_id)?;
        anyhow::ensure!(tx.gas_limit > 0, "gas limit must be non-zero");
        anyhow::ensure!(
            tx.max_gas_price >= self.head()?.header.gas_price,
            "max gas price below current gas price"
        );
        if self.cfg.reject_zero_tip {
            anyhow::ensure!(tx.mining_tip > 0, "zero mining tip rejected by config");
        }
        if tx.to.is_none() {
            anyhow::ensure!(
                !tx.payload.is_empty(),
                "contract bytecode must be non-empty"
            );
            decode_contract_blob(&tx.payload)?;
        }
        anyhow::ensure!(
            !tx_expired_at(tx, self.store.height()?),
            "transaction expired"
        );
        Ok(())
    }

    pub fn validate_tx_against_state(&self, tx: &Transaction) -> anyhow::Result<()> {
        let head = self.head()?;
        let account = self.store.get_account(&tx.from)?;
        anyhow::ensure!(
            tx.account_index == account.account_index,
            "account index mismatch"
        );
        let reserve = tx
            .value
            .checked_add(tx.mining_tip)
            .and_then(|v| {
                (tx.gas_limit as u128)
                    .checked_mul(head.header.gas_price)
                    .and_then(|gas_cost| v.checked_add(gas_cost))
            })
            .ok_or_else(|| anyhow::anyhow!("fee overflow"))?;
        anyhow::ensure!(account.balance >= reserve, "insufficient balance");
        Ok(())
    }

    pub fn mine_next_block(&mut self, miner: Address) -> anyhow::Result<Block> {
        let parent = self.head()?;
        let timestamp = now_secs().max(parent.header.timestamp + 1);
        let gas_price = next_gas_price(
            parent.header.gas_price,
            parent.header.gas_used,
            parent.header.block_gas_limit,
        );
        let mut selected = Vec::new();
        let mut body_size = 0usize;
        let mut gas_limit_left = self.cfg.block_gas_limit;
        let next_height = parent.header.height + 1;
        for tx in self.mempool.ordered()? {
            let size = tx.size()?;
            if body_size + size > MAX_BLOCK_BODY_BYTES || tx.gas_limit > gas_limit_left {
                continue;
            }
            if tx_expired_at(&tx, next_height) {
                continue;
            }
            if self.validate_tx_stateless(&tx).is_err()
                || self.validate_tx_against_state(&tx).is_err()
            {
                continue;
            }
            body_size += size;
            gas_limit_left -= tx.gas_limit;
            selected.push(tx);
        }

        let tx_root = tx_root(&selected)?;
        let mut header = BlockHeader {
            magic: MAGIC,
            protocol_version: PROTOCOL_VERSION,
            chain_id: self.cfg.chain_id,
            height: parent.header.height + 1,
            timestamp,
            prev_block_hash: parent.hash()?,
            nbits: self.next_nbits(&parent)?,
            nonce: 0,
            tx_count: selected.len() as u32,
            block_body_size: body_size as u64,
            block_gas_limit: self.cfg.block_gas_limit,
            gas_price,
            gas_used: 0,
            miner_address: miner,
            tx_root,
            receipt_root: merkle_root(vec![]),
        };
        let target = nbits_to_target(header.nbits);
        while !hash_leq_target(&header.hash()?, &target) {
            header.nonce = header.nonce.wrapping_add(1);
        }
        let mut block = Block {
            header,
            transactions: selected,
        };
        let (receipts, diffs) = self.execute_block_transactions(&block)?;
        block.header.gas_used = receipts.iter().map(|r| r.gas_used).sum();
        block.header.receipt_root = receipt_root(&receipts)?;
        let target = nbits_to_target(block.header.nbits);
        while !hash_leq_target(&block.header.hash()?, &target) {
            block.header.nonce = block.header.nonce.wrapping_add(1);
        }
        self.accept_mined_block(block.clone(), receipts, diffs)?;
        Ok(block)
    }

    pub fn accept_block(&mut self, block: Block) -> anyhow::Result<()> {
        self.validate_block_header(&block)?;
        let block_hash = block.hash()?;
        let parent_hash = block.header.prev_block_hash;
        let head_hash = self.head()?.hash()?;
        if parent_hash != head_hash {
            self.store.put_block_by_hash_only(&block)?;
            if block.header.height > self.store.height()? {
                self.reorg_to(block_hash)?;
            }
            return Ok(());
        }
        let (receipts, diffs) = self.execute_block_transactions(&block)?;
        let executed_gas_used: u64 = receipts.iter().map(|r| r.gas_used).sum();
        if block.header.gas_used != executed_gas_used {
            self.store.rollback_diffs(&diffs)?;
            anyhow::bail!("gas used mismatch");
        }
        if block.header.receipt_root != receipt_root(&receipts)? {
            self.store.rollback_diffs(&diffs)?;
            anyhow::bail!("receipt root mismatch");
        }
        self.accept_mined_block(block, receipts, diffs)
    }

    fn accept_mined_block(
        &mut self,
        block: Block,
        receipts: Vec<Receipt>,
        diffs: Vec<StateDiff>,
    ) -> anyhow::Result<()> {
        let hash = block.hash()?;
        self.store.put_diffs(&hash, &diffs)?;
        for tx in &block.transactions {
            self.mempool.remove(&tx.hash()?);
            self.store.remove_mempool_tx(&tx.hash()?)?;
        }
        self.store.put_block(&block)?;
        self.store.put_receipts(&BlockReceipts {
            block_hash: hash,
            receipts,
        })?;
        self.store.set_head(hash, block.header.height)?;
        Ok(())
    }

    fn validate_block_header(&self, block: &Block) -> anyhow::Result<()> {
        anyhow::ensure!(block.header.magic == MAGIC, "bad magic");
        anyhow::ensure!(
            block.header.protocol_version == PROTOCOL_VERSION,
            "bad protocol version"
        );
        anyhow::ensure!(block.header.chain_id == self.cfg.chain_id, "bad chain id");
        anyhow::ensure!(
            block.header.tx_count as usize == block.transactions.len(),
            "tx count mismatch"
        );
        anyhow::ensure!(
            block.body_bytes()?.len() <= MAX_BLOCK_BODY_BYTES,
            "block body too large"
        );
        anyhow::ensure!(
            block.header.block_body_size == block.body_bytes()?.len() as u64,
            "body size mismatch"
        );
        anyhow::ensure!(
            block.header.block_gas_limit == self.cfg.block_gas_limit,
            "bad block gas limit"
        );
        anyhow::ensure!(
            block.header.gas_used <= block.header.block_gas_limit,
            "header gas used exceeds block gas limit"
        );
        let declared_gas: u64 = block
            .transactions
            .iter()
            .try_fold(0u64, |sum, tx| sum.checked_add(tx.gas_limit))
            .ok_or_else(|| anyhow::anyhow!("transaction gas limit overflow"))?;
        anyhow::ensure!(
            declared_gas <= block.header.block_gas_limit,
            "transaction gas limits exceed block gas limit"
        );
        anyhow::ensure!(
            block.header.tx_root == tx_root(&block.transactions)?,
            "tx root mismatch"
        );
        let parent = self
            .store
            .get_block_by_hash(&block.header.prev_block_hash)?
            .ok_or_else(|| anyhow::anyhow!("missing parent block"))?;
        self.validate_block_against_parent(block, &parent)
    }

    fn validate_block_against_parent(&self, block: &Block, parent: &Block) -> anyhow::Result<()> {
        anyhow::ensure!(
            block.header.prev_block_hash == parent.hash()?,
            "parent hash mismatch"
        );
        anyhow::ensure!(
            block.header.height == parent.header.height + 1,
            "height mismatch"
        );
        anyhow::ensure!(
            block.header.timestamp > parent.header.timestamp,
            "timestamp must increase"
        );
        anyhow::ensure!(
            block.header.timestamp <= now_secs() + 7200,
            "timestamp too far in future"
        );
        anyhow::ensure!(
            hash_leq_target(&block.header.hash()?, &nbits_to_target(block.header.nbits)),
            "insufficient proof of work"
        );
        anyhow::ensure!(block.header.nbits == self.next_nbits(parent)?, "bad nbits");
        anyhow::ensure!(
            block.header.gas_price
                == next_gas_price(
                    parent.header.gas_price,
                    parent.header.gas_used,
                    parent.header.block_gas_limit
                ),
            "bad gas price"
        );
        Ok(())
    }

    fn reorg_to(&mut self, new_tip_hash: Hash) -> anyhow::Result<()> {
        let old_height = self.store.height()?;
        let mut new_branch = Vec::new();
        let mut cursor = self
            .store
            .get_block_by_hash(&new_tip_hash)?
            .ok_or_else(|| anyhow::anyhow!("missing reorg tip"))?;

        let ancestor = loop {
            if let Some(main) = self.store.get_block_by_height(cursor.header.height)? {
                if main.hash()? == cursor.hash()? {
                    break main;
                }
            }
            if cursor.header.height == 0 {
                anyhow::bail!("reorg branch does not connect to genesis");
            }
            new_branch.push(cursor.clone());
            cursor = self
                .store
                .get_block_by_hash(&cursor.header.prev_block_hash)?
                .ok_or_else(|| anyhow::anyhow!("missing reorg parent"))?;
        };

        if new_branch.is_empty() || new_branch[0].header.height <= old_height {
            return Ok(());
        }

        let mut old_branch = Vec::new();
        for height in (ancestor.header.height + 1)..=old_height {
            if let Some(block) = self.store.get_block_by_height(height)? {
                old_branch.push(block);
            }
        }

        for block in old_branch.iter().rev() {
            let hash = block.hash()?;
            let diffs = self
                .store
                .get_diffs(&hash)?
                .ok_or_else(|| anyhow::anyhow!("missing state diffs for rollback"))?;
            self.store.rollback_diffs(&diffs)?;
        }
        self.store
            .set_head(ancestor.hash()?, ancestor.header.height)?;

        let mut applied = Vec::new();
        let apply_result = (|| -> anyhow::Result<()> {
            for block in new_branch.iter().rev() {
                self.validate_block_header(block)?;
                let (receipts, diffs) = self.execute_block_transactions(block)?;
                let executed_gas_used: u64 = receipts.iter().map(|r| r.gas_used).sum();
                anyhow::ensure!(
                    block.header.gas_used == executed_gas_used,
                    "gas used mismatch"
                );
                anyhow::ensure!(
                    block.header.receipt_root == receipt_root(&receipts)?,
                    "receipt root mismatch"
                );
                self.accept_mined_block(block.clone(), receipts, diffs)?;
                applied.push(block.clone());
            }
            Ok(())
        })();

        if let Err(err) = apply_result {
            for block in applied.iter().rev() {
                if let Some(diffs) = self.store.get_diffs(&block.hash()?)? {
                    self.store.rollback_diffs(&diffs)?;
                }
            }
            self.store
                .set_head(ancestor.hash()?, ancestor.header.height)?;
            for block in &old_branch {
                let (receipts, diffs) = self.execute_block_transactions(block)?;
                self.accept_mined_block(block.clone(), receipts, diffs)?;
            }
            return Err(err);
        }

        let new_tx_hashes = new_branch
            .iter()
            .flat_map(|block| block.transactions.iter().filter_map(|tx| tx.hash().ok()))
            .collect::<std::collections::HashSet<_>>();
        for tx in old_branch
            .iter()
            .flat_map(|block| block.transactions.iter().cloned())
        {
            if tx.hash().is_ok_and(|hash| !new_tx_hashes.contains(&hash)) {
                let _ = self.submit_tx(tx, 0);
            }
        }
        Ok(())
    }

    fn execute_block_transactions(
        &self,
        block: &Block,
    ) -> anyhow::Result<(Vec<Receipt>, Vec<StateDiff>)> {
        let mut receipts = Vec::new();
        let mut all_diffs = Vec::new();
        let mut gas_used_total = 0u64;
        let result = (|| -> anyhow::Result<()> {
            for tx in &block.transactions {
                tx.verify_signature(block.header.chain_id)?;
                anyhow::ensure!(tx.gas_limit > 0, "gas limit must be non-zero");
                anyhow::ensure!(
                    tx.max_gas_price >= block.header.gas_price,
                    "max gas price below block gas price"
                );
                anyhow::ensure!(
                    !tx_expired_at(tx, block.header.height),
                    "transaction expired"
                );
                let mut sender = self.store.get_account(&tx.from)?;
                anyhow::ensure!(
                    tx.account_index == sender.account_index,
                    "account index mismatch"
                );
                let reserve = tx
                    .value
                    .checked_add(tx.mining_tip)
                    .and_then(|v| {
                        (tx.gas_limit as u128)
                            .checked_mul(block.header.gas_price)
                            .and_then(|gas_cost| v.checked_add(gas_cost))
                    })
                    .ok_or_else(|| anyhow::anyhow!("fee overflow"))?;
                anyhow::ensure!(sender.balance >= reserve, "insufficient balance");
                sender.balance -= reserve;
                sender.account_index += 1;
                self.store.put_account(&tx.from, &sender, &mut all_diffs)?;

                let mut success = true;
                let mut exit_reason = "native".to_string();
                let mut gas_used = 0u64;
                let mut events = Vec::new();

                if tx.to.is_none() {
                    anyhow::ensure!(
                        !tx.payload.is_empty(),
                        "contract bytecode must be non-empty"
                    );
                    decode_contract_blob(&tx.payload)?;
                    let code_hash = sha3_256(&tx.payload);
                    let contract = contract_address(&tx.from, tx.account_index);
                    let mut acct = self.store.get_account(&contract)?;
                    acct.code_hash = Some(code_hash);
                    self.store
                        .put_code(&code_hash, &tx.payload, &mut all_diffs)?;
                    self.store.put_account(&contract, &acct, &mut all_diffs)?;
                } else if let Some(to) = tx.to {
                    if tx.payload.is_empty() {
                        // Simple transfers are native transactions without VM execution.
                    } else {
                        let acct = self.store.get_account(&to)?;
                        let code_hash = acct
                            .code_hash
                            .ok_or_else(|| anyhow::anyhow!("target has no contract code"))?;
                        let code = self
                            .store
                            .code(&code_hash)?
                            .ok_or_else(|| anyhow::anyhow!("missing contract code"))?;
                        let execution = execute_contract_tx(
                            &self.store,
                            tx,
                            to,
                            code,
                            VmBlockContext {
                                height: block.header.height,
                                timestamp: block.header.timestamp,
                                chain_id: block.header.chain_id,
                                gas_price: block.header.gas_price,
                            },
                            &mut all_diffs,
                        )?;
                        success = execution.success;
                        gas_used = execution.gas_used;
                        exit_reason = format!("{:?}", execution.exit_reason);
                        events = execution
                            .events
                            .into_iter()
                            .map(|(idx, vals)| {
                                (idx, vals.into_iter().map(|v| format!("{:?}", v)).collect())
                            })
                            .collect();
                    }
                }

                if success {
                    if let Some(to) = tx.to {
                        let mut receiver = self.store.get_account(&to)?;
                        receiver.balance = receiver.balance.saturating_add(tx.value);
                        self.store.put_account(&to, &receiver, &mut all_diffs)?;
                    } else {
                        let contract = contract_address(&tx.from, tx.account_index);
                        let mut receiver = self.store.get_account(&contract)?;
                        receiver.balance = receiver.balance.saturating_add(tx.value);
                        self.store
                            .put_account(&contract, &receiver, &mut all_diffs)?;
                    }
                }

                let gas_burned = (gas_used as u128).saturating_mul(block.header.gas_price);
                let mut refund =
                    ((tx.gas_limit - gas_used) as u128).saturating_mul(block.header.gas_price);
                if !success {
                    refund = refund.saturating_add(tx.value);
                }
                let mut sender = self.store.get_account(&tx.from)?;
                sender.balance = sender.balance.saturating_add(refund);
                self.store.put_account(&tx.from, &sender, &mut all_diffs)?;

                let mut miner = self.store.get_account(&block.header.miner_address)?;
                miner.balance = miner.balance.saturating_add(tx.mining_tip);
                self.store
                    .put_account(&block.header.miner_address, &miner, &mut all_diffs)?;
                gas_used_total = gas_used_total.saturating_add(gas_used);
                receipts.push(Receipt {
                    tx_hash: tx.hash()?,
                    success,
                    gas_used,
                    gas_burned,
                    mining_tip_paid: tx.mining_tip,
                    exit_reason,
                    events,
                });
            }

            anyhow::ensure!(
                gas_used_total <= block.header.block_gas_limit,
                "block gas exceeded"
            );
            let mut miner = self.store.get_account(&block.header.miner_address)?;
            let reward = block_reward(block.header.height);
            miner.balance = miner.balance.saturating_add(reward);
            self.store
                .put_account(&block.header.miner_address, &miner, &mut all_diffs)?;
            receipts.push(Receipt {
                tx_hash: [0; 32],
                success: true,
                gas_used: 0,
                gas_burned: 0,
                mining_tip_paid: 0,
                exit_reason: "block_reward".to_string(),
                events: vec![(0, vec![reward.to_string()])],
            });
            Ok(())
        })();

        if let Err(err) = result {
            self.store.rollback_diffs(&all_diffs)?;
            return Err(err);
        }

        Ok((receipts, all_diffs))
    }

    fn next_nbits(&self, parent: &Block) -> anyhow::Result<u32> {
        if parent.header.height == 0 || (parent.header.height + 1) % RETARGET_BLOCKS != 0 {
            return Ok(parent.header.nbits);
        }
        let mut start_height = parent.header.height + 1 - RETARGET_BLOCKS;
        if start_height == 0 && parent.header.height > 1 {
            // Genesis has a fixed timestamp for deterministic chain identity; do not let
            // wall-clock time since genesis distort the first live retarget window.
            start_height = 1;
        }
        let start = self
            .ancestor_at_height(parent, start_height)?
            .ok_or_else(|| anyhow::anyhow!("missing retarget block ancestor"))?;
        let actual = parent
            .header
            .timestamp
            .saturating_sub(start.header.timestamp)
            .max(1);
        let intervals = parent
            .header
            .height
            .saturating_sub(start.header.height)
            .max(1);
        let expected = intervals * TARGET_BLOCK_SECONDS;
        Ok(target_to_nbits(&scale_target(
            nbits_to_target(parent.header.nbits),
            actual,
            expected,
        )))
    }

    fn ancestor_at_height(&self, block: &Block, height: u64) -> anyhow::Result<Option<Block>> {
        if height > block.header.height {
            return Ok(None);
        }
        let mut cursor = block.clone();
        while cursor.header.height > height {
            cursor = self
                .store
                .get_block_by_hash(&cursor.header.prev_block_hash)?
                .ok_or_else(|| anyhow::anyhow!("missing ancestor block"))?;
        }
        Ok(Some(cursor))
    }
}

pub fn contract_address(sender: &Address, account_index: u64) -> Address {
    let mut data = Vec::with_capacity(40);
    data.extend_from_slice(sender);
    data.extend_from_slice(&account_index.to_be_bytes());
    sha3_256(&data)
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn tx_expired_at(tx: &Transaction, height: u64) -> bool {
    tx.expiration_height.is_some_and(|exp| height > exp)
}
