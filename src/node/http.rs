use crate::chain::ChainCore;
use crate::crypto::{decode_hash, hex_hash};
use crate::types::{Block, Transaction};
use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::net::{Ipv4Addr, SocketAddr, UdpSocket};
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tower_http::cors::CorsLayer;

pub type SharedNode = Arc<Mutex<NodeServer>>;

pub struct NodeServer {
    pub core: ChainCore,
    pub discovery_id: String,
    pub peers: HashSet<String>,
    pub discovered_peers: HashSet<String>,
    pub peer_status: HashMap<String, PeerStatus>,
    pub orphan_blocks: HashMap<String, Block>,
    pub discovery_logs: Vec<String>,
    pub seen_txs: HashSet<String>,
    pub seen_blocks: HashSet<String>,
    pub bad_peers: HashMap<String, Instant>,
    pub mining: MiningStatus,
}

#[derive(Clone, Debug)]
pub struct PeerStatus {
    pub ok: bool,
    pub height: Option<u64>,
    pub head: Option<String>,
    pub genesis: Option<String>,
    pub last_seen: Option<Instant>,
    pub last_error: Option<String>,
}

#[derive(Clone, Debug)]
pub struct MiningStatus {
    pub enabled: bool,
    pub in_progress: bool,
    pub last_height: u64,
    pub last_hash: Option<String>,
    pub last_error: Option<String>,
    pub mined_blocks: u64,
    pub logs: Vec<String>,
}

impl MiningStatus {
    fn new(enabled: bool, height: u64) -> Self {
        Self {
            enabled,
            in_progress: false,
            last_height: height,
            last_hash: None,
            last_error: None,
            mined_blocks: 0,
            logs: vec![format!("miner initialized at height {height}")],
        }
    }

    pub fn push_log(&mut self, msg: impl Into<String>) {
        self.logs.push(msg.into());
        if self.logs.len() > 12 {
            self.logs.remove(0);
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct HealthResponse {
    pub ok: bool,
    #[serde(default)]
    pub node_id: Option<String>,
    pub chain_id: u64,
    pub height: u64,
    pub head: String,
    pub genesis: String,
    pub peers: usize,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct PeerAnnouncement {
    pub url: String,
    pub height: u64,
    #[serde(default)]
    pub node_id: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct HeaderAnnouncement {
    pub from: String,
    pub block_hash: String,
    pub height: u64,
}

impl NodeServer {
    pub fn new(core: ChainCore) -> Self {
        let peers = core.cfg.peers.iter().cloned().collect();
        let height = core.store.height().unwrap_or(0);
        let mining = MiningStatus::new(core.cfg.mine, height);
        Self {
            core,
            discovery_id: new_discovery_id(),
            peers,
            discovered_peers: HashSet::new(),
            peer_status: HashMap::new(),
            orphan_blocks: HashMap::new(),
            discovery_logs: vec!["LAN discovery ready".to_string()],
            seen_txs: HashSet::new(),
            seen_blocks: HashSet::new(),
            bad_peers: HashMap::new(),
            mining,
        }
    }

    pub fn is_peer_allowed(&mut self, peer: &str) -> bool {
        self.bad_peers.retain(|_, until| *until > Instant::now());
        !self.bad_peers.contains_key(peer)
    }

    pub fn punish_peer(&mut self, peer: String) {
        self.bad_peers
            .insert(peer, Instant::now() + Duration::from_secs(3600));
    }

    pub fn push_discovery_log(&mut self, msg: impl Into<String>) {
        self.discovery_logs.push(msg.into());
        if self.discovery_logs.len() > 12 {
            self.discovery_logs.remove(0);
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct LanAnnouncement {
    magic: String,
    node_id: String,
    chain_id: u64,
    port: u16,
    height: u64,
}

const LAN_DISCOVERY_PORT: u16 = 12368;
const LAN_DISCOVERY_MAGIC: &str = "COINLAN1";

pub async fn run_lan_discovery(node: SharedNode) {
    let bind_addr = format!("0.0.0.0:{LAN_DISCOVERY_PORT}");
    let socket = match tokio::net::UdpSocket::bind(&bind_addr).await {
        Ok(socket) => socket,
        Err(err) => {
            node.lock()
                .unwrap()
                .push_discovery_log(format!("LAN discovery disabled: {err}"));
            return;
        }
    };
    if let Err(err) = socket.set_broadcast(true) {
        node.lock()
            .unwrap()
            .push_discovery_log(format!("LAN broadcast disabled: {err}"));
    }

    let mut buf = [0u8; 512];
    let mut tick = tokio::time::interval(Duration::from_secs(5));
    loop {
        tokio::select! {
            _ = tick.tick() => announce_lan(&socket, &node).await,
            received = socket.recv_from(&mut buf) => {
                if let Ok((len, from)) = received {
                    handle_lan_packet(&node, &buf[..len], from);
                }
            }
        }
    }
}

async fn announce_lan(socket: &tokio::net::UdpSocket, node: &SharedNode) {
    let msg = {
        let node = node.lock().unwrap();
        let Some(port) = listen_port(&node.core.cfg.listen_addr) else {
            return;
        };
        LanAnnouncement {
            magic: LAN_DISCOVERY_MAGIC.to_string(),
            node_id: node.discovery_id.clone(),
            chain_id: node.core.cfg.chain_id,
            port,
            height: node.core.store.height().unwrap_or(0),
        }
    };
    let Ok(bytes) = serde_json::to_vec(&msg) else {
        return;
    };
    let target = format!("255.255.255.255:{LAN_DISCOVERY_PORT}");
    let _ = socket.send_to(&bytes, target).await;
}

fn handle_lan_packet(node: &SharedNode, bytes: &[u8], from: SocketAddr) {
    let Ok(msg) = serde_json::from_slice::<LanAnnouncement>(bytes) else {
        return;
    };
    if msg.magic != LAN_DISCOVERY_MAGIC || from.ip().is_loopback() {
        return;
    }
    let peer = normalize_peer_url(&format!("{}:{}", from.ip(), msg.port));
    let mut node = node.lock().unwrap();
    if msg.node_id == node.discovery_id || msg.chain_id != node.core.cfg.chain_id {
        return;
    }
    let is_new = node.peers.insert(peer.clone());
    node.discovered_peers.insert(peer.clone());
    if is_new {
        node.push_discovery_log(format!("discovered {peer} at height {}", msg.height));
    }
}

fn new_discovery_id() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or_default();
    format!("{}-{nanos}", std::process::id())
}

fn listen_port(listen_addr: &str) -> Option<u16> {
    listen_addr.rsplit_once(':')?.1.parse::<u16>().ok()
}

pub fn normalize_peer_url(peer: &str) -> String {
    let peer = if peer.starts_with("http://") || peer.starts_with("https://") {
        peer.trim_end_matches('/').to_string()
    } else {
        format!("http://{}", peer.trim_end_matches('/'))
    };
    let Some((scheme, rest)) = peer.split_once("://") else {
        return peer;
    };
    let host_port = rest.split('/').next().unwrap_or(rest);
    if host_port
        .rsplit_once(':')
        .is_some_and(|(_, port)| port.parse::<u16>().is_ok())
    {
        peer
    } else {
        format!("{scheme}://{rest}:12367")
    }
}

pub fn advertised_peer_url(listen_addr: &str) -> String {
    let advertised = if listen_addr.starts_with("0.0.0.0:") {
        match local_ipv4() {
            Some(ip) => listen_addr.replacen("0.0.0.0", &ip.to_string(), 1),
            None => listen_addr.replacen("0.0.0.0", "127.0.0.1", 1),
        }
    } else {
        listen_addr.to_string()
    };
    normalize_peer_url(&advertised)
}

pub async fn run_peer_sync(node: SharedNode) {
    let client = match reqwest::Client::builder()
        .timeout(Duration::from_secs(3))
        .build()
    {
        Ok(client) => client,
        Err(err) => {
            node.lock()
                .unwrap()
                .push_discovery_log(format!("peer sync disabled: {err}"));
            return;
        }
    };
    let mut tick = tokio::time::interval(Duration::from_secs(4));
    let mut scan_tick = tokio::time::interval(Duration::from_secs(20));
    loop {
        tokio::select! {
            _ = tick.tick() => {}
            _ = scan_tick.tick() => scan_lan_for_peers(&node, &client).await,
        }
        let (peers, self_url) = {
            let node = node.lock().unwrap();
            let self_url = advertised_peer_url(&node.core.cfg.listen_addr);
            (node.peers.iter().cloned().collect::<Vec<_>>(), self_url)
        };
        for peer in peers {
            sync_peer(&node, &client, peer, self_url.clone()).await;
        }
        retry_orphans(&node);
    }
}

async fn scan_lan_for_peers(node: &SharedNode, client: &reqwest::Client) {
    let (chain_id, self_id, self_port, self_genesis, known) = {
        let node = node.lock().unwrap();
        let self_genesis = genesis_hash(&node).unwrap_or_default();
        (
            node.core.cfg.chain_id,
            node.discovery_id.clone(),
            listen_port(&node.core.cfg.listen_addr).unwrap_or(12367),
            self_genesis,
            node.peers.clone(),
        )
    };
    let mut candidates = arp_ipv4_candidates();
    let local_ip = local_ipv4();
    if let Some(local) = local_ip {
        let [a, b, c, _] = local.octets();
        candidates.extend((1..=254).map(|d| Ipv4Addr::new(a, b, c, d)));
    }
    candidates.sort();
    candidates.dedup();

    let mut spawned = 0usize;
    for ip in candidates {
        if ip.is_loopback() || ip.is_unspecified() || local_ip == Some(ip) {
            continue;
        }
        let peer = normalize_peer_url(&format!("{ip}:{self_port}"));
        if known.contains(&peer) {
            continue;
        }
        spawned += 1;
        let node = node.clone();
        let client = client.clone();
        let self_genesis = self_genesis.clone();
        let self_id = self_id.clone();
        tokio::spawn(async move {
            let base = peer.trim_end_matches('/');
            let Ok(res) = client.get(format!("{base}/health")).send().await else {
                return;
            };
            if !res.status().is_success() {
                return;
            }
            let Ok(health) = res.json::<HealthResponse>().await else {
                node.lock()
                    .unwrap()
                    .push_discovery_log(format!("LAN scan saw {peer} but /health was invalid"));
                return;
            };
            if !health.ok {
                return;
            }
            if health.node_id.as_deref() == Some(self_id.as_str()) {
                node.lock()
                    .unwrap()
                    .push_discovery_log(format!("LAN scan ignored self {peer}"));
                return;
            }
            if health.chain_id != chain_id {
                node.lock().unwrap().push_discovery_log(format!(
                    "LAN scan rejected {peer}: wrong chain id {}",
                    health.chain_id
                ));
                return;
            }
            if health.genesis != self_genesis {
                mark_peer_error(
                    &node,
                    &peer,
                    "genesis mismatch; reset one node's data dir".to_string(),
                );
                return;
            }
            let mut node = node.lock().unwrap();
            let is_new = node.peers.insert(peer.clone());
            node.discovered_peers.insert(peer.clone());
            if is_new {
                node.push_discovery_log(format!("LAN scan found {peer}"));
            }
        });
    }
    if spawned > 0 {
        node.lock()
            .unwrap()
            .push_discovery_log(format!("LAN scan probing {spawned} candidates"));
    } else {
        node.lock()
            .unwrap()
            .push_discovery_log("LAN scan had no new candidates".to_string());
    }
}

async fn sync_peer(node: &SharedNode, client: &reqwest::Client, peer: String, self_url: String) {
    let base = peer.trim_end_matches('/');
    let health = match client.get(format!("{base}/health")).send().await {
        Ok(res) => match res.json::<HealthResponse>().await {
            Ok(health) if health.ok => health,
            Ok(_) => {
                mark_peer_error(node, &peer, "health check failed".to_string());
                return;
            }
            Err(err) => {
                mark_peer_error(node, &peer, format!("health decode failed: {err}"));
                return;
            }
        },
        Err(err) => {
            mark_peer_error(node, &peer, format!("connect failed: {err}"));
            return;
        }
    };
    if health.node_id.as_deref() == Some(node.lock().unwrap().discovery_id.as_str()) {
        let mut node = node.lock().unwrap();
        node.peers.remove(&peer);
        node.discovered_peers.remove(&peer);
        node.peer_status.remove(&peer);
        node.push_discovery_log(format!("ignored self peer {peer}"));
        return;
    }
    if health.chain_id != node.lock().unwrap().core.cfg.chain_id {
        mark_peer_error(node, &peer, format!("wrong chain id {}", health.chain_id));
        return;
    }
    let local_genesis = node
        .lock()
        .unwrap()
        .core
        .store
        .get_block_by_height(0)
        .ok()
        .flatten()
        .and_then(|block| block.hash().ok())
        .map(|hash| hex_hash(&hash))
        .unwrap_or_default();
    if health.genesis != local_genesis {
        mark_peer_error(
            node,
            &peer,
            "genesis mismatch; reset one node's data dir".to_string(),
        );
        return;
    }
    mark_peer_ok(node, &peer, &health);

    sync_blocks(node, client, &peer, base, &health).await;

    sync_mempool(node, client, &peer, base).await;

    if let Ok(res) = client.get(format!("{base}/peers")).send().await {
        if let Ok(remote_peers) = res.json::<Vec<String>>().await {
            let mut node = node.lock().unwrap();
            for remote_peer in remote_peers {
                let remote_peer = normalize_peer_url(&remote_peer);
                if remote_peer != self_url {
                    node.peers.insert(remote_peer);
                }
            }
        }
    }

    let (height, node_id) = {
        let node = node.lock().unwrap();
        (
            node.core.store.height().unwrap_or(0),
            node.discovery_id.clone(),
        )
    };
    let _ = client
        .post(format!("{base}/peers"))
        .json(&PeerAnnouncement {
            url: self_url,
            height,
            node_id: Some(node_id),
        })
        .send()
        .await;
}

async fn sync_blocks(
    node: &SharedNode,
    client: &reqwest::Client,
    peer: &str,
    base: &str,
    health: &HealthResponse,
) {
    let (local_height, local_head) = {
        let node = node.lock().unwrap();
        (
            node.core.store.height().unwrap_or(0),
            node.core
                .head()
                .ok()
                .and_then(|block| block.hash().ok())
                .map(|hash| hex_hash(&hash))
                .unwrap_or_default(),
        )
    };
    if health.height < local_height || (health.height == local_height && health.head == local_head)
    {
        return;
    }

    let mut common_height = None;
    let mut height = health.height.min(local_height);
    loop {
        let remote_hash = match fetch_block_by_height(client, base, height).await {
            Some(block) => block.hash().ok().map(|hash| (hex_hash(&hash), block)),
            None => None,
        };
        let Some((remote_hash, remote_block)) = remote_hash else {
            break;
        };
        let local_matches = {
            let node = node.lock().unwrap();
            node.core
                .store
                .get_block_by_height(height)
                .ok()
                .flatten()
                .and_then(|block| block.hash().ok())
                .is_some_and(|hash| hex_hash(&hash) == remote_hash)
        };
        if local_matches {
            common_height = Some(height);
            break;
        }
        if height == health.height {
            let _ = accept_synced_block(node, peer, height, remote_block);
        }
        if height == 0 {
            break;
        }
        height -= 1;
    }

    let start = common_height.map(|h| h + 1).unwrap_or(local_height + 1);
    for height in start..=health.height {
        let Some(block) = fetch_block_by_height(client, base, height).await else {
            break;
        };
        if !accept_synced_block(node, peer, height, block) {
            break;
        }
    }
}

async fn fetch_block_by_height(client: &reqwest::Client, base: &str, height: u64) -> Option<Block> {
    let res = client
        .get(format!("{base}/block/height/{height}"))
        .send()
        .await
        .ok()?;
    if !res.status().is_success() {
        return None;
    }
    res.json::<Block>().await.ok()
}

fn accept_synced_block(node: &SharedNode, peer: &str, height: u64, block: Block) -> bool {
    let hash = block.hash().ok().map(|h| hex_hash(&h)).unwrap_or_default();
    let parent_known = {
        let node = node.lock().unwrap();
        block.header.height == 0
            || node
                .core
                .store
                .get_block_by_hash(&block.header.prev_block_hash)
                .ok()
                .flatten()
                .is_some()
    };
    if !parent_known {
        let mut node = node.lock().unwrap();
        node.orphan_blocks.insert(hash.clone(), block);
        node.push_discovery_log(format!("stored orphan block {height} {hash} from {peer}"));
        return false;
    }
    let mut node = node.lock().unwrap();
    match node.core.accept_block(block) {
        Ok(()) => {
            node.seen_blocks.insert(hash.clone());
            node.push_discovery_log(format!("synced block {height} {hash} from {peer}"));
            true
        }
        Err(err) => {
            node.push_discovery_log(format!(
                "sync block {height} {hash} from {peer} failed: {err}"
            ));
            false
        }
    }
}

fn retry_orphans(node: &SharedNode) {
    let pending = {
        let node = node.lock().unwrap();
        node.orphan_blocks.values().cloned().collect::<Vec<_>>()
    };
    for block in pending {
        let hash = block.hash().ok().map(|h| hex_hash(&h)).unwrap_or_default();
        let mut node = node.lock().unwrap();
        if node.core.accept_block(block).is_ok() {
            node.orphan_blocks.remove(&hash);
            node.seen_blocks.insert(hash.clone());
            node.push_discovery_log(format!("accepted orphan block {hash}"));
        }
    }
}

async fn sync_mempool(node: &SharedNode, client: &reqwest::Client, peer: &str, base: &str) {
    let Ok(res) = client.get(format!("{base}/mempool")).send().await else {
        return;
    };
    if !res.status().is_success() {
        return;
    }
    let Ok(remote_txs) = res.json::<Vec<Transaction>>().await else {
        return;
    };
    if remote_txs.is_empty() {
        return;
    }

    let mut accepted = Vec::new();
    {
        let mut node = node.lock().unwrap();
        let peer_count = node.peers.len();
        for tx in remote_txs {
            let tx_hash = tx.hash().ok().map(|hash| hex_hash(&hash));
            if tx_hash
                .as_ref()
                .is_some_and(|hash| node.seen_txs.contains(hash))
            {
                continue;
            }
            let result = node.core.submit_tx(tx.clone(), peer_count);
            if result.accepted {
                if let Some(hash) = tx_hash {
                    node.seen_txs.insert(hash);
                }
                accepted.push(tx);
            }
        }
        if !accepted.is_empty() {
            node.push_discovery_log(format!("synced {} mempool txs from {peer}", accepted.len()));
        }
    }

    for tx in accepted {
        let peers = node
            .lock()
            .unwrap()
            .peers
            .iter()
            .cloned()
            .collect::<Vec<_>>();
        tokio::spawn(gossip_tx(peers, tx));
    }
}

fn mark_peer_ok(node: &SharedNode, peer: &str, health: &HealthResponse) {
    let mut node = node.lock().unwrap();
    node.peer_status.insert(
        peer.to_string(),
        PeerStatus {
            ok: true,
            height: Some(health.height),
            head: Some(health.head.clone()),
            genesis: Some(health.genesis.clone()),
            last_seen: Some(Instant::now()),
            last_error: None,
        },
    );
}

fn mark_peer_error(node: &SharedNode, peer: &str, err: String) {
    let mut node = node.lock().unwrap();
    node.peer_status.insert(
        peer.to_string(),
        PeerStatus {
            ok: false,
            height: None,
            head: None,
            genesis: None,
            last_seen: None,
            last_error: Some(err.clone()),
        },
    );
    node.push_discovery_log(format!("peer {peer}: {err}"));
}

fn genesis_hash(node: &NodeServer) -> anyhow::Result<String> {
    let block = node
        .core
        .store
        .get_block_by_height(0)?
        .ok_or_else(|| anyhow::anyhow!("missing genesis block"))?;
    Ok(hex_hash(&block.hash()?))
}

fn local_ipv4() -> Option<Ipv4Addr> {
    let socket = UdpSocket::bind("0.0.0.0:0").ok()?;
    socket.connect("8.8.8.8:80").ok()?;
    match socket.local_addr().ok()?.ip() {
        std::net::IpAddr::V4(ip) if !ip.is_loopback() && !ip.is_unspecified() => Some(ip),
        _ => None,
    }
}

fn arp_ipv4_candidates() -> Vec<Ipv4Addr> {
    let Ok(output) = Command::new("arp").arg("-a").output() else {
        return Vec::new();
    };
    if !output.status.success() {
        return Vec::new();
    }
    String::from_utf8_lossy(&output.stdout)
        .split_whitespace()
        .filter_map(|part| part.parse::<Ipv4Addr>().ok())
        .filter(|ip| !ip.is_loopback() && !ip.is_unspecified())
        .collect()
}

pub async fn serve(node: SharedNode) -> anyhow::Result<()> {
    let addr = node.lock().unwrap().core.cfg.listen_addr.clone();
    let app = router(node);
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    axum::serve(listener, app).await?;
    Ok(())
}

pub fn router(node: SharedNode) -> Router {
    Router::new()
        .route("/health", get(health))
        .route("/height", get(height))
        .route("/peers", get(peers).post(announce_peer))
        .route("/tx", post(submit_tx_json))
        .route("/tx/bin", post(submit_tx_bin))
        .route("/block/header", post(block_header_announce))
        .route("/block", post(submit_block_json))
        .route("/block/bin", post(submit_block_bin))
        .route("/block/hash/:hash", get(block_by_hash))
        .route("/block/height/:height", get(block_by_height))
        .route("/account/:address", get(account_by_address))
        .route("/mempool", get(mempool))
        .layer(CorsLayer::permissive())
        .with_state(node)
}

async fn health(State(node): State<SharedNode>) -> Json<HealthResponse> {
    let node = node.lock().unwrap();
    let head = node.core.head().and_then(|b| b.hash()).unwrap_or([0; 32]);
    let genesis = genesis_hash(&node).unwrap_or_else(|_| "unknown".to_string());
    Json(HealthResponse {
        ok: true,
        node_id: Some(node.discovery_id.clone()),
        chain_id: node.core.cfg.chain_id,
        height: node.core.store.height().unwrap_or(0),
        head: hex_hash(&head),
        genesis,
        peers: node.peers.len(),
    })
}

async fn height(State(node): State<SharedNode>) -> Json<serde_json::Value> {
    Json(serde_json::json!({ "height": node.lock().unwrap().core.store.height().unwrap_or(0) }))
}

async fn peers(State(node): State<SharedNode>) -> Json<Vec<String>> {
    Json(node.lock().unwrap().peers.iter().cloned().collect())
}

async fn announce_peer(
    State(node): State<SharedNode>,
    Json(msg): Json<PeerAnnouncement>,
) -> Json<serde_json::Value> {
    let mut node = node.lock().unwrap();
    if msg.node_id.as_deref() != Some(node.discovery_id.as_str()) {
        let peer = normalize_peer_url(&msg.url);
        if peer != advertised_peer_url(&node.core.cfg.listen_addr) {
            node.peers.insert(peer);
        }
    }
    Json(serde_json::json!({ "accepted": true }))
}

async fn submit_tx_json(
    State(node): State<SharedNode>,
    Json(tx): Json<Transaction>,
) -> (StatusCode, Json<serde_json::Value>) {
    let (result, peers, tx_for_gossip) = {
        let mut node = node.lock().unwrap();
        let tx_hash = tx.hash().ok().map(|hash| hex_hash(&hash));
        if tx_hash
            .as_ref()
            .is_some_and(|hash| node.seen_txs.contains(hash))
        {
            return (
                StatusCode::OK,
                Json(serde_json::json!({ "accepted": true, "warning": "already seen" })),
            );
        }
        let peers = node.peers.iter().cloned().collect::<Vec<_>>();
        let result = node.core.submit_tx(tx.clone(), peers.len());
        if result.accepted {
            if let Some(hash) = tx_hash {
                node.seen_txs.insert(hash);
            }
        }
        let tx_for_gossip = result.accepted.then_some(tx);
        (result, peers, tx_for_gossip)
    };
    if let Some(tx) = tx_for_gossip {
        tokio::spawn(gossip_tx(peers, tx));
    }
    (
        if result.accepted {
            StatusCode::OK
        } else {
            StatusCode::BAD_REQUEST
        },
        Json(serde_json::to_value(result).unwrap()),
    )
}

async fn submit_tx_bin(
    State(node): State<SharedNode>,
    body: Bytes,
) -> (StatusCode, Json<serde_json::Value>) {
    match bincode::deserialize::<Transaction>(&body) {
        Ok(tx) => submit_tx_json(State(node), Json(tx)).await,
        Err(err) => (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "accepted": false, "error": err.to_string() })),
        ),
    }
}

async fn submit_block_json(
    State(node): State<SharedNode>,
    Json(block): Json<Block>,
) -> (StatusCode, Json<serde_json::Value>) {
    let hash = block.hash().unwrap_or([0; 32]);
    let hash_hex = hex_hash(&hash);
    let block_for_gossip = block.clone();
    let accepted = {
        let mut node = node.lock().unwrap();
        if node.seen_blocks.contains(&hash_hex)
            || node
                .core
                .store
                .get_block_by_hash(&hash)
                .ok()
                .flatten()
                .is_some()
        {
            return (
                StatusCode::OK,
                Json(
                    serde_json::json!({ "accepted": true, "hash": hash_hex, "warning": "already seen" }),
                ),
            );
        }
        match node.core.accept_block(block) {
            Ok(()) => {
                node.seen_blocks.insert(hash_hex.clone());
                Some((
                    node.peers.iter().cloned().collect::<Vec<_>>(),
                    advertised_peer_url(&node.core.cfg.listen_addr),
                ))
            }
            Err(err) => {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(serde_json::json!({ "accepted": false, "error": err.to_string() })),
                )
            }
        }
    };
    if let Some((peers, from)) = accepted {
        tokio::spawn(async move { gossip_block(peers, from, block_for_gossip).await });
    }
    (
        StatusCode::OK,
        Json(serde_json::json!({ "accepted": true, "hash": hash_hex })),
    )
}

async fn submit_block_bin(
    State(node): State<SharedNode>,
    body: Bytes,
) -> (StatusCode, Json<serde_json::Value>) {
    match bincode::deserialize::<Block>(&body) {
        Ok(block) => submit_block_json(State(node), Json(block)).await,
        Err(err) => (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "accepted": false, "error": err.to_string() })),
        ),
    }
}

async fn block_header_announce(
    State(node): State<SharedNode>,
    Json(msg): Json<HeaderAnnouncement>,
) -> Json<serde_json::Value> {
    let (seen, height, should_fetch, from, hash) = {
        let mut node = node.lock().unwrap();
        if node.is_peer_allowed(&msg.from) {
            node.peers.insert(normalize_peer_url(&msg.from));
        }
        let seen = node.seen_blocks.contains(&msg.block_hash)
            || decode_hash(&msg.block_hash)
                .ok()
                .and_then(|hash| node.core.store.get_block_by_hash(&hash).ok().flatten())
                .is_some();
        (
            seen,
            node.core.store.height().unwrap_or(0),
            !seen && msg.height > node.core.store.height().unwrap_or(0),
            normalize_peer_url(&msg.from),
            msg.block_hash,
        )
    };
    if should_fetch {
        let node = node.clone();
        tokio::spawn(async move { fetch_announced_block(node, from, hash).await });
    }
    Json(serde_json::json!({ "seen": seen, "height": height }))
}

async fn fetch_announced_block(node: SharedNode, from: String, hash: String) {
    let client = match reqwest::Client::builder()
        .timeout(Duration::from_secs(3))
        .build()
    {
        Ok(client) => client,
        Err(_) => return,
    };
    let base = from.trim_end_matches('/');
    let Ok(res) = client.get(format!("{base}/block/hash/{hash}")).send().await else {
        return;
    };
    if !res.status().is_success() {
        return;
    }
    let Ok(block) = res.json::<Block>().await else {
        return;
    };
    let parent_hash = block.header.prev_block_hash;
    let parent_known = {
        let node = node.lock().unwrap();
        block.header.height == 0
            || node
                .core
                .store
                .get_block_by_hash(&parent_hash)
                .ok()
                .flatten()
                .is_some()
    };
    if !parent_known {
        fetch_block_by_hash_once(&node, &client, base, parent_hash).await;
    }
    let hash = block
        .hash()
        .ok()
        .map(|hash| hex_hash(&hash))
        .unwrap_or_default();
    let block_for_gossip = block.clone();
    let accepted = {
        let mut node = node.lock().unwrap();
        match node.core.accept_block(block) {
            Ok(()) => {
                node.seen_blocks.insert(hash.clone());
                node.push_discovery_log(format!("accepted announced block {hash} from {from}"));
                Some((
                    node.peers.iter().cloned().collect::<Vec<_>>(),
                    advertised_peer_url(&node.core.cfg.listen_addr),
                ))
            }
            Err(err) => {
                node.push_discovery_log(format!(
                    "announced block {hash} from {from} failed: {err}"
                ));
                None
            }
        }
    };
    retry_orphans(&node);
    if let Some((peers, from)) = accepted {
        tokio::spawn(async move { gossip_block(peers, from, block_for_gossip).await });
    }
}

async fn fetch_block_by_hash_once(
    node: &SharedNode,
    client: &reqwest::Client,
    base: &str,
    hash: [u8; 32],
) {
    let hash_hex = hex_hash(&hash);
    let Ok(res) = client
        .get(format!("{base}/block/hash/{hash_hex}"))
        .send()
        .await
    else {
        return;
    };
    if !res.status().is_success() {
        return;
    }
    let Ok(block) = res.json::<Block>().await else {
        return;
    };
    let accepted = {
        let mut node = node.lock().unwrap();
        if node.core.accept_block(block).is_ok() {
            node.seen_blocks.insert(hash_hex.clone());
            node.push_discovery_log(format!("fetched missing parent {hash_hex}"));
            true
        } else {
            false
        }
    };
    if accepted {
        retry_orphans(node);
    }
}

async fn block_by_hash(
    State(node): State<SharedNode>,
    Path(hash): Path<String>,
) -> (StatusCode, Json<serde_json::Value>) {
    match decode_hash(&hash).and_then(|h| node.lock().unwrap().core.store.get_block_by_hash(&h)) {
        Ok(Some(block)) => (StatusCode::OK, Json(serde_json::to_value(block).unwrap())),
        Ok(None) => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "error": "not found" })),
        ),
        Err(err) => (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": err.to_string() })),
        ),
    }
}

async fn block_by_height(
    State(node): State<SharedNode>,
    Path(height): Path<u64>,
) -> (StatusCode, Json<serde_json::Value>) {
    match node.lock().unwrap().core.store.get_block_by_height(height) {
        Ok(Some(block)) => (StatusCode::OK, Json(serde_json::to_value(block).unwrap())),
        Ok(None) => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "error": "not found" })),
        ),
        Err(err) => (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": err.to_string() })),
        ),
    }
}

async fn account_by_address(
    State(node): State<SharedNode>,
    Path(address): Path<String>,
) -> (StatusCode, Json<serde_json::Value>) {
    match decode_hash(&address) {
        Ok(addr) => {
            let account = node
                .lock()
                .unwrap()
                .core
                .store
                .get_account(&addr)
                .unwrap_or_default();
            (StatusCode::OK, Json(serde_json::to_value(account).unwrap()))
        }
        Err(err) => (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": err.to_string() })),
        ),
    }
}

async fn mempool(State(node): State<SharedNode>) -> Json<Vec<Transaction>> {
    Json(node.lock().unwrap().core.mempool.all())
}

pub async fn gossip_tx(peers: Vec<String>, tx: Transaction) {
    let client = match reqwest::Client::builder()
        .timeout(Duration::from_secs(3))
        .build()
    {
        Ok(client) => client,
        Err(_) => return,
    };
    for peer in peers {
        let _ = client
            .post(format!("{}/tx", peer.trim_end_matches('/')))
            .json(&tx)
            .send()
            .await;
    }
}

pub async fn gossip_block(peers: Vec<String>, from: String, block: Block) {
    let client = match reqwest::Client::builder()
        .timeout(Duration::from_secs(3))
        .build()
    {
        Ok(client) => client,
        Err(_) => return,
    };
    for peer in &peers {
        let _ = client
            .post(format!("{}/block", peer.trim_end_matches('/')))
            .json(&block)
            .send()
            .await;
    }
    gossip_block_header(peers, from, &block).await;
}

pub async fn gossip_block_header(peers: Vec<String>, from: String, block: &Block) {
    let Ok(hash) = block.hash() else {
        return;
    };
    let msg = HeaderAnnouncement {
        from,
        block_hash: hex_hash(&hash),
        height: block.header.height,
    };
    let client = match reqwest::Client::builder()
        .timeout(Duration::from_secs(3))
        .build()
    {
        Ok(client) => client,
        Err(_) => return,
    };
    for peer in peers {
        let _ = client
            .post(format!("{}/block/header", peer.trim_end_matches('/')))
            .json(&msg)
            .send()
            .await;
    }
}
