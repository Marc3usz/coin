use crate::crypto::{sha3_256, Address, Hash};
use k256::ecdsa::{signature::Verifier, Signature, VerifyingKey};
use serde::{Deserialize, Serialize};

use super::Amount;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Transaction {
    pub from: Address,
    pub to: Option<Address>,
    pub value: Amount,
    pub gas_limit: u64,
    pub max_gas_price: Amount,
    pub mining_tip: Amount,
    pub expiration_height: Option<u64>,
    pub payload: Vec<u8>,
    pub account_index: u64,
    pub nonce: u64,
    pub public_key: Vec<u8>,
    pub signature: Vec<u8>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TxSignData<'a> {
    pub chain_id: u64,
    pub from: Address,
    pub to: Option<Address>,
    pub value: Amount,
    pub gas_limit: u64,
    pub max_gas_price: Amount,
    pub mining_tip: Amount,
    pub expiration_height: Option<u64>,
    pub payload: &'a [u8],
    pub account_index: u64,
    pub nonce: u64,
    pub public_key: &'a [u8],
}

impl Transaction {
    pub fn signing_bytes(&self, chain_id: u64) -> anyhow::Result<Vec<u8>> {
        Ok(bincode::serialize(&TxSignData {
            chain_id,
            from: self.from,
            to: self.to,
            value: self.value,
            gas_limit: self.gas_limit,
            max_gas_price: self.max_gas_price,
            mining_tip: self.mining_tip,
            expiration_height: self.expiration_height,
            payload: &self.payload,
            account_index: self.account_index,
            nonce: self.nonce,
            public_key: &self.public_key,
        })?)
    }

    pub fn signing_hash(&self, chain_id: u64) -> anyhow::Result<Hash> {
        Ok(sha3_256(&self.signing_bytes(chain_id)?))
    }

    pub fn hash(&self) -> anyhow::Result<Hash> {
        Ok(sha3_256(&bincode::serialize(self)?))
    }

    pub fn size(&self) -> anyhow::Result<usize> {
        Ok(bincode::serialize(self)?.len())
    }

    pub fn verify_signature(&self, chain_id: u64) -> anyhow::Result<()> {
        let derived = crate::crypto::address_from_public_key(&self.public_key);
        anyhow::ensure!(
            derived == self.from,
            "sender address does not match public key"
        );
        let verifying_key = VerifyingKey::from_sec1_bytes(&self.public_key)?;
        let signature = Signature::from_slice(&self.signature)?;
        verifying_key.verify(&self.signing_hash(chain_id)?, &signature)?;
        Ok(())
    }
}
