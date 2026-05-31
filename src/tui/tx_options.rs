use crate::types::Transaction;
use crate::wallet::{sign_tx, WalletFile};

pub fn parse_u64(text: &str, label: &str) -> Result<u64, String> {
    text.trim()
        .parse::<u64>()
        .map_err(|_| format!("invalid {label}"))
}

pub fn parse_u128(text: &str, label: &str) -> Result<u128, String> {
    text.trim()
        .parse::<u128>()
        .map_err(|_| format!("invalid {label}"))
}

pub fn sign_with_optional_grind(
    tx: Transaction,
    chain_id: u64,
    wallet: &WalletFile,
    iterations: u64,
) -> anyhow::Result<Transaction> {
    let mut best = sign_tx(tx.clone(), chain_id, wallet)?;
    let mut best_hash = best.hash()?;
    for nonce in 1..=iterations {
        let mut candidate = tx.clone();
        candidate.nonce = nonce;
        let signed = sign_tx(candidate, chain_id, wallet)?;
        let hash = signed.hash()?;
        if hash < best_hash {
            best_hash = hash;
            best = signed;
        }
    }
    Ok(best)
}
