use axum::http::StatusCode;
use coin::chain::{contract_address, ChainCore};
use coin::crypto::{address_from_public_key, hex_hash};
use coin::node::{router, NodeServer};
use coin::types::{Account, Transaction};
use coin::wallet::{sign_tx, WalletFile};
use coin::{
    encode_contract_blob, encode_contract_call, ContractBlob, ContractCallKind,
    ContractCallPayload, Metadata, MethodMeta, Opcode, Value,
};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};
use tower::ServiceExt;

fn test_dir(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "coin-client-{}-{}",
        name,
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ))
}

fn config(name: &str) -> coin::config::NodeConfig {
    let data_dir = test_dir(name);
    coin::config::NodeConfig {
        data_dir: data_dir.clone(),
        config_dir: data_dir.join("config"),
        wallet_path: data_dir.join("config").join("wallet.toml"),
        mine: false,
        ..coin::config::NodeConfig::default()
    }
}

fn node(name: &str) -> Arc<Mutex<NodeServer>> {
    let cfg = config(name);
    cfg.ensure_dirs().unwrap();
    let core = ChainCore::open(cfg).unwrap();
    Arc::new(Mutex::new(NodeServer::new(core)))
}

async fn json_body(resp: axum::response::Response) -> String {
    String::from_utf8(
        axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap()
}

#[tokio::test]
async fn wallet_round_trip_and_signing_match_address() {
    let wallet = WalletFile::generate();
    let path = test_dir("wallet-round-trip").join("wallet.toml");
    wallet.save(&path).unwrap();
    let loaded = WalletFile::load(&path).unwrap();
    assert_eq!(wallet.address().unwrap(), loaded.address().unwrap());
    assert_eq!(
        address_from_public_key(
            loaded
                .signing_key()
                .unwrap()
                .verifying_key()
                .to_encoded_point(true)
                .as_bytes()
        ),
        loaded.address().unwrap()
    );
}

#[tokio::test]
async fn http_health_height_account_and_mempool_work_like_a_client() {
    let node = node("http-smoke");
    let app = router(node.clone());
    let wallet = WalletFile::generate();
    let addr = wallet.address().unwrap();

    {
        let guard = node.lock().unwrap();
        let mut diffs = Vec::new();
        guard
            .core
            .store
            .put_account(
                &addr,
                &Account {
                    balance: 123,
                    account_index: 7,
                    code_hash: None,
                },
                &mut diffs,
            )
            .unwrap();
    }

    let health = app
        .clone()
        .oneshot(
            axum::http::Request::builder()
                .uri("/health")
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(health.status(), StatusCode::OK);

    let account = app
        .clone()
        .oneshot(
            axum::http::Request::builder()
                .uri(format!("/account/{}", hex_hash(&addr)))
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(account.status(), StatusCode::OK);

    let height = app
        .oneshot(
            axum::http::Request::builder()
                .uri("/height")
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(height.status(), StatusCode::OK);
}

#[tokio::test]
async fn http_submit_tx_and_block_round_trip_like_a_client() {
    let node = node("http-submit");
    let app = router(node.clone());
    let wallet = WalletFile::generate();
    let sender = wallet.address().unwrap();
    {
        let guard = node.lock().unwrap();
        let mut diffs = Vec::new();
        guard
            .core
            .store
            .put_account(
                &sender,
                &Account {
                    balance: 1_000_000_000,
                    account_index: 0,
                    code_hash: None,
                },
                &mut diffs,
            )
            .unwrap();
    }

    let tx = sign_tx(
        Transaction {
            from: [0; 32],
            to: Some([0x44; 32]),
            value: 99,
            gas_limit: 50_000,
            max_gas_price: 10_000,
            mining_tip: 7,
            expiration_height: None,
            payload: Vec::new(),
            account_index: 0,
            nonce: 1,
            public_key: Vec::new(),
            signature: Vec::new(),
        },
        1,
        &wallet,
    )
    .unwrap();

    let res = app
        .clone()
        .oneshot(
            axum::http::Request::builder()
                .method("POST")
                .uri("/tx")
                .header("content-type", "application/json")
                .body(axum::body::Body::from(serde_json::to_vec(&tx).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
}

#[tokio::test]
async fn client_contract_call_flow_uses_lcall1_and_persists_receipts() {
    let node = node("http-contract");
    let app = router(node.clone());
    let wallet = WalletFile::generate();
    let sender = wallet.address().unwrap();
    {
        let guard = node.lock().unwrap();
        let mut diffs = Vec::new();
        guard
            .core
            .store
            .put_account(
                &sender,
                &Account {
                    balance: 3_000_000_000,
                    account_index: 0,
                    code_hash: None,
                },
                &mut diffs,
            )
            .unwrap();
    }

    let mut metadata = Metadata::default();
    metadata.methods.insert(1, MethodMeta { args: 1, rets: 0 });
    metadata.jump_table.insert(1, 0);
    let code = encode_contract_blob(&ContractBlob {
        metadata,
        code: vec![Opcode::SetState as u8, 0, Opcode::Stop as u8],
    })
    .unwrap();
    let contract = contract_address(&sender, 0);
    let deploy = sign_tx(
        Transaction {
            from: [0; 32],
            to: None,
            value: 0,
            gas_limit: 200_000,
            max_gas_price: 10_000,
            mining_tip: 11,
            expiration_height: None,
            payload: code.clone(),
            account_index: 0,
            nonce: 1,
            public_key: Vec::new(),
            signature: Vec::new(),
        },
        1,
        &wallet,
    )
    .unwrap();
    let deploy_res = app
        .clone()
        .oneshot(
            axum::http::Request::builder()
                .method("POST")
                .uri("/tx")
                .header("content-type", "application/json")
                .body(axum::body::Body::from(serde_json::to_vec(&deploy).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    let deploy_status = deploy_res.status();
    assert_eq!(
        deploy_status,
        StatusCode::OK,
        "{}",
        json_body(deploy_res).await
    );

    {
        let mut guard = node.lock().unwrap();
        guard.core.mine_next_block([0x77; 32]).unwrap();
    }

    let account_res = app
        .clone()
        .oneshot(
            axum::http::Request::builder()
                .uri(format!("/account/{}", hex_hash(&sender)))
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let account_body = json_body(account_res).await;
    let account_json: serde_json::Value = serde_json::from_str(&account_body).unwrap();
    let account_index = account_json["account_index"].as_u64().unwrap();

    let call = sign_tx(
        Transaction {
            from: [0; 32],
            to: Some(contract),
            value: 0,
            gas_limit: 200_000,
            max_gas_price: 10_000,
            mining_tip: 11,
            expiration_height: None,
            payload: encode_contract_call(&ContractCallPayload {
                kind: ContractCallKind::Method,
                method_idx: 1,
                args: vec![Value::Address([0x88; 32])],
            })
            .unwrap(),
            account_index,
            nonce: 2,
            public_key: Vec::new(),
            signature: Vec::new(),
        },
        1,
        &wallet,
    )
    .unwrap();

    let call_res = app
        .clone()
        .oneshot(
            axum::http::Request::builder()
                .method("POST")
                .uri("/tx")
                .header("content-type", "application/json")
                .body(axum::body::Body::from(serde_json::to_vec(&call).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    let call_status = call_res.status();
    assert_eq!(call_status, StatusCode::OK, "{}", json_body(call_res).await);

    {
        let mut guard = node.lock().unwrap();
        guard.core.mine_next_block([0x77; 32]).unwrap();
    }

    let mut guard = node.lock().unwrap();
    let block = guard.core.mine_next_block([0x77; 32]).unwrap();
    let receipts = guard
        .core
        .store
        .get_receipts(&block.hash().unwrap())
        .unwrap();
    assert!(receipts.is_some());
    assert_eq!(
        guard.core.store.get_vm_state_value(&contract, 0),
        Value::Address([0x88; 32])
    );
}
