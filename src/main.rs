use clap::{Parser, Subcommand};
use coin::chain::ChainCore;
use coin::config::NodeConfig;
use coin::crypto::{decode_hash, hex_hash, Address};
use coin::node::{
    gossip_block_header, normalize_peer_url, run_lan_discovery, run_peer_sync, serve, NodeServer,
};
use coin::types::Transaction;
use coin::wallet::{sign_tx, WalletFile};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

#[derive(Parser)]
#[command(name = "coin")]
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
        no_mine: bool,
    },
    Init {
        #[arg(long)]
        out: Option<PathBuf>,
        #[arg(long)]
        config_dir: Option<PathBuf>,
        #[arg(long)]
        data_dir: Option<PathBuf>,
        #[arg(long)]
        wallet: Option<PathBuf>,
        #[arg(long, default_value = "0.0.0.0:12367")]
        listen: String,
        #[arg(long)]
        peer: Vec<String>,
        #[arg(long, default_value_t = 1)]
        chain_id: u64,
        #[arg(long)]
        no_mine: bool,
        #[arg(long)]
        force: bool,
    },
    Config {
        #[command(subcommand)]
        command: ConfigCommands,
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
    Tui,
}

#[derive(Subcommand)]
enum ConfigCommands {
    Show,
    AddPeer { peer: String },
    RemovePeer { peer: String },
    SetListen { listen: String },
    SetMining { enabled: bool },
}

pub fn start_node_background(mut cfg: NodeConfig) -> anyhow::Result<Arc<Mutex<NodeServer>>> {
    cfg.ensure_dirs()?;
    if cfg.miner_address.is_none() && cfg.wallet_path.exists() {
        cfg.miner_address = Some(WalletFile::load(&cfg.wallet_path)?.address()?);
    }
    let core = ChainCore::open(cfg.clone())?;
    let node = Arc::new(Mutex::new(NodeServer::new(core)));

    let mining_node = node.clone();
    tokio::spawn(async move {
        loop {
            let (mine, miner) = {
                let mut node = mining_node.lock().unwrap();
                node.mining.enabled = node.core.cfg.mine;
                (node.core.cfg.mine, node.core.cfg.miner_address)
            };
            if mine {
                let mined = {
                    let mut node = mining_node.lock().unwrap();
                    node.mining.in_progress = true;
                    node.mining.last_error = None;
                    let next_height = node.core.store.height().unwrap_or(0) + 1;
                    node.mining
                        .push_log(format!("mining candidate block {next_height}"));
                    let miner = miner.unwrap_or([0; 32]);
                    let peers = node.peers.iter().cloned().collect::<Vec<_>>();
                    let listen = format!(
                        "http://{}",
                        node.core.cfg.listen_addr.replace("0.0.0.0", "127.0.0.1")
                    );
                    match node.core.mine_next_block(miner) {
                        Ok(block) => {
                            let height = block.header.height;
                            let hash = hex_hash(&block.hash().unwrap_or([0; 32]));
                            node.mining.in_progress = false;
                            node.mining.last_height = height;
                            node.mining.last_hash = Some(hash.clone());
                            node.mining.mined_blocks += 1;
                            node.mining.push_log(format!("mined block {height} {hash}"));
                            Some((peers, listen, block))
                        }
                        Err(err) => {
                            node.mining.in_progress = false;
                            node.mining.last_error = Some(err.to_string());
                            node.mining.push_log(format!("mining error: {err}"));
                            None
                        }
                    }
                };
                if let Some((peers, listen, block)) = mined {
                    gossip_block_header(peers, listen, &block).await;
                    tokio::time::sleep(Duration::from_secs(4)).await;
                    continue;
                }
            }
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
    });

    let serve_node = node.clone();
    tokio::spawn(async move {
        if let Err(_e) = serve(serve_node).await {
            // We ignore errors in background for TUI, or handle gracefully
        }
    });

    let discovery_node = node.clone();
    tokio::spawn(async move {
        run_lan_discovery(discovery_node).await;
    });

    let sync_node = node.clone();
    tokio::spawn(async move {
        run_peer_sync(sync_node).await;
    });

    Ok(node)
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let config_path = cli.config;
    let mut cfg = NodeConfig::load(config_path.clone())?;
    match cli.command {
        Commands::Run { no_mine } => {
            if no_mine {
                cfg.mine = false;
            }
            println!("node listening on {}", cfg.listen_addr);
            println!("mining: {}", if cfg.mine { "enabled" } else { "disabled" });
            if !cfg.peers.is_empty() {
                println!("configured peers:");
                for peer in &cfg.peers {
                    println!("  {peer}");
                }
            }

            let _node = start_node_background(cfg)?;
            // Keep the main thread alive forever
            loop {
                tokio::time::sleep(Duration::from_secs(3600)).await;
            }
        }
        Commands::Init {
            out,
            config_dir,
            data_dir,
            wallet,
            listen,
            peer,
            chain_id,
            no_mine,
            force,
        } => {
            let default = NodeConfig::default();
            let config_dir = config_dir.unwrap_or(default.config_dir);
            let data_dir = data_dir.unwrap_or(default.data_dir);
            let wallet_path = wallet.unwrap_or_else(|| config_dir.join("wallet.toml"));
            let out = out.unwrap_or_else(|| config_dir.join("config.toml"));
            if out.exists() && !force {
                anyhow::bail!(
                    "config already exists at {}; use --force to overwrite",
                    out.display()
                );
            }
            let cfg = NodeConfig {
                chain_id,
                listen_addr: listen,
                peers: peer.into_iter().map(|p| normalize_peer_url(&p)).collect(),
                mine: !no_mine,
                reject_zero_tip: false,
                block_gas_limit: default.block_gas_limit,
                miner_address: None,
                wallet_path: wallet_path.clone(),
                config_dir,
                data_dir,
            };
            cfg.ensure_dirs()?;
            if !wallet_path.exists() {
                let wallet = WalletFile::generate();
                wallet.save(&wallet_path)?;
                println!("created wallet: {}", wallet_path.display());
                println!("address: {}", wallet.address_hex);
            } else {
                println!("using existing wallet: {}", wallet_path.display());
            }
            cfg.save(&out)?;
            println!("created config: {}", out.display());
            println!(
                "run: coin --config {} run{}",
                out.display(),
                if cfg.mine { "" } else { " --no-mine" }
            );
        }
        Commands::Config { command } => {
            let path =
                config_path.unwrap_or_else(|| NodeConfig::default().config_dir.join("config.toml"));
            match command {
                ConfigCommands::Show => {
                    println!("{}", toml::to_string_pretty(&cfg)?);
                }
                ConfigCommands::AddPeer { peer } => {
                    let peer = normalize_peer_url(&peer);
                    if !cfg.peers.contains(&peer) {
                        cfg.peers.push(peer.clone());
                    }
                    cfg.save(&path)?;
                    println!("added peer: {peer}");
                }
                ConfigCommands::RemovePeer { peer } => {
                    let peer = normalize_peer_url(&peer);
                    cfg.peers.retain(|p| p != &peer);
                    cfg.save(&path)?;
                    println!("removed peer: {peer}");
                }
                ConfigCommands::SetListen { listen } => {
                    cfg.listen_addr = listen;
                    cfg.save(&path)?;
                    println!("listen_addr = {}", cfg.listen_addr);
                }
                ConfigCommands::SetMining { enabled } => {
                    cfg.mine = enabled;
                    cfg.save(&path)?;
                    println!("mine = {}", cfg.mine);
                }
            }
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
        Commands::Tui => {
            let node = start_node_background(cfg)?;
            let mut app = coin::tui::App::new(node, config_path)?;
            app.run()?;
        }
    }
    Ok(())
}

fn parse_optional_address(value: Option<String>) -> anyhow::Result<Option<Address>> {
    value.map(|v| decode_hash(&v)).transpose()
}
