use clap::{Parser, Subcommand};
use coin::chain::ChainCore;
use coin::config::NodeConfig;
use coin::crypto::{decode_hash, hex_hash, Address};
use coin::node::{gossip_block_header, serve, NodeServer};
use coin::types::Transaction;
use coin::wallet::{sign_tx, WalletFile};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

#[derive(Parser)]
#[command(name = "coin-node")]
struct Cli {
    #[arg(long)]
    config: Option<PathBuf>,
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    Run {
        #[arg(long)]
        mine: Option<bool>,
    },
    Keygen {
        #[arg(long)]
        out: Option<PathBuf>,
    },
    SubmitTx {
        #[arg(long)]
        node: String,
        #[arg(long)]
        wallet: Option<PathBuf>,
        #[arg(long)]
        to: Option<String>,
        #[arg(long, default_value_t = 0)]
        value: u128,
        #[arg(long, default_value_t = 100_000)]
        gas_limit: u64,
        #[arg(long, default_value_t = 1000)]
        max_gas_price: u128,
        #[arg(long, default_value_t = 1)]
        mining_tip: u128,
        #[arg(long)]
        payload_hex: Option<String>,
        #[arg(long, default_value_t = 0)]
        account_index: u64,
        #[arg(long, default_value_t = 0)]
        nonce: u64,
        #[arg(long)]
        expiration_height: Option<u64>,
    },
    PrintChain,
    Account {
        #[arg(long, default_value = "http://127.0.0.1:12367")]
        node: String,
        address: Option<String>,
    },
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let config_path = cli.config;
    let mut cfg = NodeConfig::load(config_path.clone())?;
    match cli.command {
        Commands::Run { mine } => {
            if let Some(mine) = mine {
                cfg.mine = mine;
            }
            cfg.ensure_dirs()?;
            if cfg.miner_address.is_none() && cfg.wallet_path.exists() {
                cfg.miner_address = Some(WalletFile::load(&cfg.wallet_path)?.address()?);
            }
            let core = ChainCore::open(cfg.clone())?;
            let node = Arc::new(Mutex::new(NodeServer::new(core)));
            if cfg.mine {
                let mining_node = node.clone();
                tokio::spawn(async move {
                    loop {
                        let mined = {
                            let mut node = mining_node.lock().unwrap();
                            let miner = node.core.cfg.miner_address.unwrap_or([0; 32]);
                            let peers = node.peers.iter().cloned().collect::<Vec<_>>();
                            let listen = format!(
                                "http://{}",
                                node.core.cfg.listen_addr.replace("0.0.0.0", "127.0.0.1")
                            );
                            match node.core.mine_next_block(miner) {
                                Ok(block) => Some((peers, listen, block)),
                                Err(err) => {
                                    eprintln!("mining failed: {err}");
                                    None
                                }
                            }
                        };
                        if let Some((peers, listen, block)) = mined {
                            println!(
                                "mined block {} {}",
                                block.header.height,
                                hex_hash(&block.hash().unwrap_or([0; 32]))
                            );
                            gossip_block_header(peers, listen, &block).await;
                        }
                        tokio::time::sleep(Duration::from_secs(32)).await;
                    }
                });
            }
            serve(node).await?;
        }
        Commands::Keygen { out } => {
            let cfg = NodeConfig::load(config_path.clone())?;
            let path = out.unwrap_or(cfg.wallet_path);
            let wallet = WalletFile::generate();
            wallet.save(&path)?;
            println!("address: {}", wallet.address_hex);
            println!("wallet: {}", path.display());
        }
        Commands::SubmitTx {
            node,
            wallet,
            to,
            value,
            gas_limit,
            max_gas_price,
            mining_tip,
            payload_hex,
            account_index,
            nonce,
            expiration_height,
        } => {
            let cfg = NodeConfig::load(config_path.clone())?;
            let wallet = WalletFile::load(&wallet.unwrap_or(cfg.wallet_path))?;
            let to = parse_optional_address(to)?;
            let payload = payload_hex
                .map(|p| hex::decode(p.trim_start_matches("0x")))
                .transpose()?
                .unwrap_or_default();
            let tx = Transaction {
                from: wallet.address()?,
                to,
                value,
                gas_limit,
                max_gas_price,
                mining_tip,
                expiration_height,
                payload,
                account_index,
                nonce,
                public_key: Vec::new(),
                signature: Vec::new(),
            };
            let tx = sign_tx(tx, cfg.chain_id, &wallet)?;
            let res = reqwest::Client::new()
                .post(format!("{}/tx", node.trim_end_matches('/')))
                .json(&tx)
                .send()
                .await?;
            println!("{}", res.text().await?);
        }
        Commands::PrintChain => {
            let core = ChainCore::open(cfg)?;
            let height = core.store.height()?;
            for h in 0..=height {
                if let Some(block) = core.store.get_block_by_height(h)? {
                    println!("Height: {}", block.header.height);
                    println!("Hash: {}", hex_hash(&block.hash()?));
                    println!("Transactions: {}", block.transactions.len());
                    println!();
                }
            }
        }
        Commands::Account { node, address } => {
            let addr = match address {
                Some(a) => a,
                None => {
                    let wallet = WalletFile::load(&cfg.wallet_path)?;
                    wallet.address_hex.clone()
                }
            };
            let res = reqwest::Client::new()
                .get(format!("{}/account/{}", node.trim_end_matches('/'), addr))
                .send()
                .await?;
            println!("{}", res.text().await?);
        }
    }
    Ok(())
}

fn parse_optional_address(value: Option<String>) -> anyhow::Result<Option<Address>> {
    value.map(|v| decode_hash(&v)).transpose()
}
