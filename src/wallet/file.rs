use crate::crypto::{address_from_public_key, Address};
use k256::ecdsa::SigningKey;
use rand_core::OsRng;
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WalletFile {
    pub private_key_hex: String,
    pub public_key_hex: String,
    pub address_hex: String,
}

impl WalletFile {
    pub fn generate() -> Self {
        let key = SigningKey::random(&mut OsRng);
        Self::from_key(&key)
    }

    pub fn from_key(key: &SigningKey) -> Self {
        let private_key_hex = hex::encode(key.to_bytes());
        let public_key = key.verifying_key().to_encoded_point(true);
        let public_key_hex = hex::encode(public_key.as_bytes());
        let address_hex = hex::encode(address_from_public_key(public_key.as_bytes()));
        Self {
            private_key_hex,
            public_key_hex,
            address_hex,
        }
    }

    pub fn save(&self, path: &Path) -> anyhow::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, toml::to_string_pretty(self)?)?;
        Ok(())
    }

    pub fn load(path: &Path) -> anyhow::Result<Self> {
        Ok(toml::from_str(&std::fs::read_to_string(path)?)?)
    }

    pub fn signing_key(&self) -> anyhow::Result<SigningKey> {
        let bytes = hex::decode(&self.private_key_hex)?;
        Ok(SigningKey::from_slice(&bytes)?)
    }

    pub fn address(&self) -> anyhow::Result<Address> {
        let bytes = hex::decode(&self.address_hex)?;
        anyhow::ensure!(bytes.len() == 32, "address must be 32 bytes");
        let mut out = [0u8; 32];
        out.copy_from_slice(&bytes);
        Ok(out)
    }
}
