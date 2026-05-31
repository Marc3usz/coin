use crate::crypto::Address;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct NodeConfig {
    pub chain_id: u64,
    pub listen_addr: String,
    pub peers: Vec<String>,
    pub mine: bool,
    pub reject_zero_tip: bool,
    pub block_gas_limit: u64,
    pub miner_address: Option<Address>,
    pub wallet_path: PathBuf,
    pub config_dir: PathBuf,
    pub data_dir: PathBuf,
}

impl Default for NodeConfig {
    fn default() -> Self {
        let home = std::env::var("HOME")
            .or_else(|_| std::env::var("USERPROFILE"))
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from("."));
        let config_dir = home.join(".config").join("coin-node");
        let data_dir = home.join(".local").join("share").join("coin-node");
        Self {
            chain_id: 1,
            listen_addr: "0.0.0.0:12367".to_string(),
            peers: Vec::new(),
            mine: true,
            reject_zero_tip: false,
            block_gas_limit: 30_000_000,
            miner_address: None,
            wallet_path: config_dir.join("wallet.toml"),
            config_dir,
            data_dir,
        }
    }
}

impl NodeConfig {
    pub fn load(path: Option<PathBuf>) -> anyhow::Result<Self> {
        let default = Self::default();
        let path = path.unwrap_or_else(|| default.config_dir.join("config.toml"));
        if !path.exists() {
            return Ok(default);
        }
        let text = std::fs::read_to_string(path)?;
        let raw: PartialNodeConfig = toml::from_str(&text)?;
        let config_dir = raw.config_dir.unwrap_or_else(|| default.config_dir.clone());
        let data_dir = raw.data_dir.unwrap_or_else(|| default.data_dir.clone());
        let wallet_path = raw
            .wallet_path
            .unwrap_or_else(|| config_dir.join("wallet.toml"));
        Ok(Self {
            chain_id: raw.chain_id.unwrap_or(default.chain_id),
            listen_addr: raw.listen_addr.unwrap_or(default.listen_addr),
            peers: raw.peers.unwrap_or(default.peers),
            mine: raw.mine.unwrap_or(default.mine),
            reject_zero_tip: raw.reject_zero_tip.unwrap_or(default.reject_zero_tip),
            block_gas_limit: raw.block_gas_limit.unwrap_or(default.block_gas_limit),
            miner_address: raw.miner_address.or(default.miner_address),
            wallet_path,
            config_dir,
            data_dir,
        })
    }

    pub fn ensure_dirs(&self) -> anyhow::Result<()> {
        std::fs::create_dir_all(&self.config_dir)?;
        std::fs::create_dir_all(&self.data_dir)?;
        std::fs::create_dir_all(self.data_dir.join("blocks"))?;
        std::fs::create_dir_all(self.data_dir.join("receipts"))?;
        Ok(())
    }

    pub fn save(&self, path: impl AsRef<Path>) -> anyhow::Result<()> {
        let path = path.as_ref();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, toml::to_string_pretty(self)?)?;
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, Default)]
struct PartialNodeConfig {
    chain_id: Option<u64>,
    listen_addr: Option<String>,
    peers: Option<Vec<String>>,
    mine: Option<bool>,
    reject_zero_tip: Option<bool>,
    block_gas_limit: Option<u64>,
    miner_address: Option<Address>,
    wallet_path: Option<PathBuf>,
    config_dir: Option<PathBuf>,
    data_dir: Option<PathBuf>,
}
