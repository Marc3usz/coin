use crate::crypto::Hash;
use crate::types::Transaction;
use std::cmp::Ordering;
use std::collections::HashMap;

#[derive(Default)]
pub struct Mempool {
    txs: HashMap<Hash, Transaction>,
}

impl Mempool {
    pub fn insert(&mut self, tx: Transaction) -> anyhow::Result<Hash> {
        let hash = tx.hash()?;
        self.txs.entry(hash).or_insert(tx);
        Ok(hash)
    }

    pub fn remove(&mut self, hash: &Hash) {
        self.txs.remove(hash);
    }

    pub fn contains(&self, hash: &Hash) -> bool {
        self.txs.contains_key(hash)
    }

    pub fn ordered(&self) -> anyhow::Result<Vec<Transaction>> {
        let mut keyed = self
            .txs
            .values()
            .cloned()
            .map(|tx| {
                let size = tx.size()?.max(1) as u128;
                let score = tx.mining_tip / size;
                let hash = tx.hash()?;
                Ok((score, hash, tx))
            })
            .collect::<anyhow::Result<Vec<_>>>()?;
        keyed.sort_by(|a, b| match b.0.cmp(&a.0) {
            Ordering::Equal => a.1.cmp(&b.1),
            other => other,
        });
        Ok(keyed.into_iter().map(|(_, _, tx)| tx).collect())
    }

    pub fn all(&self) -> Vec<Transaction> {
        self.txs.values().cloned().collect()
    }
}
