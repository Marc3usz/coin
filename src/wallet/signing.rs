use crate::types::Transaction;
use k256::ecdsa::{signature::Signer, Signature};

use super::WalletFile;

pub fn sign_tx(
    mut tx: Transaction,
    chain_id: u64,
    wallet: &WalletFile,
) -> anyhow::Result<Transaction> {
    tx.public_key = hex::decode(&wallet.public_key_hex)?;
    tx.from = wallet.address()?;
    tx.signature.clear();
    let key = wallet.signing_key()?;
    let sig: Signature = key.sign(&tx.signing_hash(chain_id)?);
    tx.signature = sig.to_bytes().to_vec();
    Ok(tx)
}
