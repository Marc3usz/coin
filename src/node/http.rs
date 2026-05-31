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
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tower_http::cors::CorsLayer;

pub type SharedNode = Arc<Mutex<NodeServer>>;

pub struct NodeServer {
    pub core: ChainCore,
    pub peers: HashSet<String>,
    pub seen_txs: HashSet<String>,
    pub seen_blocks: HashSet<String>,
    pub bad_peers: HashMap<String, Instant>,
    pub mining: MiningStatus,
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

#[derive(Debug, Serialize)]
pub struct HealthResponse {
    pub ok: bool,
    pub height: u64,
    pub head: String,
    pub peers: usize,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct PeerAnnouncement {
    pub url: String,
    pub height: u64,
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
            peers,
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
    Json(HealthResponse {
        ok: true,
        height: node.core.store.height().unwrap_or(0),
        head: hex_hash(&head),
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
    node.lock().unwrap().peers.insert(msg.url);
    Json(serde_json::json!({ "accepted": true }))
}

async fn submit_tx_json(
    State(node): State<SharedNode>,
    Json(tx): Json<Transaction>,
) -> (StatusCode, Json<serde_json::Value>) {
    let mut node = node.lock().unwrap();
    let peers = node.peers.len();
    let result = node.core.submit_tx(tx, peers);
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
    let mut node = node.lock().unwrap();
    let hash = block.hash().unwrap_or([0; 32]);
    match node.core.accept_block(block) {
        Ok(()) => {
            node.seen_blocks.insert(hex_hash(&hash));
            (
                StatusCode::OK,
                Json(serde_json::json!({ "accepted": true, "hash": hex_hash(&hash) })),
            )
        }
        Err(err) => (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "accepted": false, "error": err.to_string() })),
        ),
    }
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
    let mut node = node.lock().unwrap();
    if node.is_peer_allowed(&msg.from) {
        node.peers.insert(msg.from);
    }
    Json(
        serde_json::json!({ "seen": node.seen_blocks.contains(&msg.block_hash), "height": node.core.store.height().unwrap_or(0) }),
    )
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
    let client = reqwest::Client::new();
    for peer in peers {
        let _ = client
            .post(format!("{}/tx", peer.trim_end_matches('/')))
            .json(&tx)
            .send()
            .await;
    }
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
    let client = reqwest::Client::new();
    for peer in peers {
        let _ = client
            .post(format!("{}/block/header", peer.trim_end_matches('/')))
            .json(&msg)
            .send()
            .await;
    }
}
