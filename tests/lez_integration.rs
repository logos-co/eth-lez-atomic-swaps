use std::{net::SocketAddr, path::PathBuf, time::Duration};

use common::sequencer_client::SequencerClient;
use lez_htlc_methods::{LEZ_HTLC_PROGRAM_ELF, LEZ_HTLC_PROGRAM_ID};
use lez_htlc_program::HTLCState;
use nssa::{
    AccountId, PrivateKey, PublicKey,
    program_deployment_transaction::Message as ProgramDeploymentMessage,
    ProgramDeploymentTransaction,
};
use nssa_core::program::ProgramId;
use sequencer_core::config::{AccountInitialData, SequencerConfig};
use sha2::{Digest, Sha256};
use swap_orchestrator::{
    config::SwapConfig,
    lez::{client::LezClient, watcher},
};
use tempfile::TempDir;
use tokio::sync::mpsc;
use url::Url;

const BLOCK_WAIT: Duration = Duration::from_secs(4);

/// Start a no-op JSON-RPC WebSocket server so the sequencer's indexer client
/// can connect at startup (it panics if the WS handshake fails).
async fn start_dummy_ws_server() -> (SocketAddr, jsonrpsee::server::ServerHandle) {
    let server = jsonrpsee::server::Server::builder()
        .build("127.0.0.1:0")
        .await
        .unwrap();
    let addr = server.local_addr().unwrap();
    let handle = server.start(jsonrpsee::RpcModule::new(()));
    (addr, handle)
}

fn test_key(seed: u8) -> (PrivateKey, AccountId) {
    let key = PrivateKey::try_new([seed; 32]).unwrap();
    let pub_key = PublicKey::new_from_private_key(&key);
    let id = AccountId::from(&pub_key);
    (key, id)
}

fn make_preimage_and_hashlock() -> ([u8; 32], [u8; 32]) {
    let preimage = [0xABu8; 32];
    let hashlock: [u8; 32] = Sha256::digest(preimage).into();
    (preimage, hashlock)
}

struct TestEnv {
    _handle: sequencer_runner::SequencerHandle,
    _temp_dir: TempDir,
    _indexer_handle: jsonrpsee::server::ServerHandle,
    program_id: ProgramId,
    maker_key: PrivateKey,
    maker_id: AccountId,
    taker_key: PrivateKey,
    taker_id: AccountId,
    sequencer_url: String,
}

impl TestEnv {
    fn lez_client_for(&self, key: &PrivateKey, counterparty_lez: AccountId) -> LezClient {
        let config = SwapConfig {
            lez_signing_key: hex::encode(key.value()),
            lez_sequencer_url: self.sequencer_url.clone(),
            lez_htlc_program_id: self.program_id,
            lez_account_id: AccountId::new([0; 32]),
            counterparty_lez_account_id: counterparty_lez,
            // Unused by LezClient::new:
            eth_rpc_url: String::new(),
            eth_private_key: String::new(),
            eth_htlc_address: alloy::primitives::Address::ZERO,
            lez_amount: 0,
            eth_amount: 0,
            lez_timelock: 0,
            eth_timelock: 0,
            counterparty_eth_address: alloy::primitives::Address::ZERO,
            poll_interval: Duration::from_millis(500),
        };
        LezClient::new(&config).unwrap()
    }

    fn maker_client(&self) -> LezClient {
        self.lez_client_for(&self.maker_key, self.taker_id)
    }

    fn taker_client(&self) -> LezClient {
        self.lez_client_for(&self.taker_key, self.maker_id)
    }
}

async fn setup() -> TestEnv {
    let (maker_key, maker_id) = test_key(1);
    let (taker_key, taker_id) = test_key(2);

    // Start dummy WS server for the sequencer's indexer client.
    let (indexer_addr, indexer_handle) = start_dummy_ws_server().await;

    let config_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("configs/test_sequencer.json");
    let mut config = SequencerConfig::from_path(&config_path).unwrap();

    let temp_dir = tempfile::tempdir().unwrap();
    config.home = temp_dir.path().to_owned();
    config.port = 0;
    config.indexer_rpc_url = format!("ws://127.0.0.1:{}", indexer_addr.port())
        .parse()
        .unwrap();
    config.initial_accounts = vec![
        AccountInitialData {
            account_id: maker_id.to_string(),
            balance: 1_000_000,
        },
        AccountInitialData {
            account_id: taker_id.to_string(),
            balance: 1_000_000,
        },
    ];

    let (handle, addr) = sequencer_runner::startup_sequencer(config).await.unwrap();
    let addr = SocketAddr::new("127.0.0.1".parse().unwrap(), addr.port());
    let sequencer_url = format!("http://{addr}");

    // Deploy LEZ HTLC program.
    let client = SequencerClient::new(Url::parse(&sequencer_url).unwrap()).unwrap();
    let msg = ProgramDeploymentMessage::new(LEZ_HTLC_PROGRAM_ELF.to_vec());
    let tx = ProgramDeploymentTransaction { message: msg };
    client.send_tx_program(tx).await.unwrap();

    // Wait for deployment block.
    tokio::time::sleep(BLOCK_WAIT).await;

    TestEnv {
        _handle: handle,
        _temp_dir: temp_dir,
        _indexer_handle: indexer_handle,
        program_id: LEZ_HTLC_PROGRAM_ID,
        maker_key,
        maker_id,
        taker_key,
        taker_id,
        sequencer_url,
    }
}

// ---------- Tests ----------

#[tokio::test]
async fn test_transfer_and_read_balance() {
    let env = setup().await;
    let maker = env.maker_client();

    let before = maker.get_balance(&env.taker_id).await.unwrap();
    maker.transfer(env.taker_id, 500).await.unwrap();
    tokio::time::sleep(BLOCK_WAIT).await;

    let after = maker.get_balance(&env.taker_id).await.unwrap();
    assert_eq!(after, before + 500);
}

#[tokio::test]
async fn test_lock_creates_escrow() {
    let env = setup().await;
    let maker = env.maker_client();
    let (_, hashlock) = make_preimage_and_hashlock();

    maker.lock(hashlock, env.taker_id, 1000).await.unwrap();
    tokio::time::sleep(BLOCK_WAIT * 2).await;

    let escrow = maker.get_escrow(&hashlock).await.unwrap().expect("escrow should exist");
    assert_eq!(escrow.state, HTLCState::Locked);
    assert_eq!(escrow.amount, 1000);
    assert_eq!(escrow.taker_id, env.taker_id);
}

#[tokio::test]
async fn test_lock_then_claim() {
    let env = setup().await;
    let maker = env.maker_client();
    let taker = env.taker_client();
    let (preimage, hashlock) = make_preimage_and_hashlock();

    maker.lock(hashlock, env.taker_id, 1000).await.unwrap();
    tokio::time::sleep(BLOCK_WAIT * 2).await;

    let taker_before = taker.get_balance(&env.taker_id).await.unwrap();
    taker.claim(&hashlock, &preimage).await.unwrap();
    tokio::time::sleep(BLOCK_WAIT).await;

    let escrow = maker.get_escrow(&hashlock).await.unwrap().unwrap();
    assert_eq!(escrow.state, HTLCState::Claimed);

    let taker_after = taker.get_balance(&env.taker_id).await.unwrap();
    assert_eq!(taker_after, taker_before + 1000);
}

#[tokio::test]
async fn test_lock_then_refund() {
    let env = setup().await;
    let maker = env.maker_client();
    let (_, hashlock) = make_preimage_and_hashlock();

    let maker_before = maker.get_balance(&env.maker_id).await.unwrap();
    maker.lock(hashlock, env.taker_id, 1000).await.unwrap();
    tokio::time::sleep(BLOCK_WAIT * 2).await;

    maker.refund(&hashlock).await.unwrap();
    tokio::time::sleep(BLOCK_WAIT).await;

    let escrow = maker.get_escrow(&hashlock).await.unwrap().unwrap();
    assert_eq!(escrow.state, HTLCState::Refunded);

    let maker_after = maker.get_balance(&env.maker_id).await.unwrap();
    assert_eq!(maker_after, maker_before);
}

#[tokio::test]
async fn test_claim_wrong_preimage_fails() {
    let env = setup().await;
    let maker = env.maker_client();
    let taker = env.taker_client();
    let (_, hashlock) = make_preimage_and_hashlock();

    maker.lock(hashlock, env.taker_id, 1000).await.unwrap();
    tokio::time::sleep(BLOCK_WAIT * 2).await;

    let wrong_preimage = [0xFFu8; 32];
    // Transaction is accepted to the mempool but fails during block execution.
    let _ = taker.claim(&hashlock, &wrong_preimage).await;
    tokio::time::sleep(BLOCK_WAIT).await;

    // Escrow should still be Locked — the wrong preimage claim had no effect.
    let escrow = maker.get_escrow(&hashlock).await.unwrap().unwrap();
    assert_eq!(escrow.state, HTLCState::Locked);
}

#[tokio::test]
async fn test_watcher_detects_lock_and_claim() {
    let env = setup().await;
    let maker = env.maker_client();
    let taker = env.taker_client();
    let (preimage, hashlock) = make_preimage_and_hashlock();

    let (tx, mut rx) = mpsc::channel(16);
    let watcher_client = env.lez_client_for(&env.maker_key, env.taker_id);
    let watcher_handle = tokio::spawn(async move {
        watcher::watch_escrow(&watcher_client, hashlock, Duration::from_millis(500), tx).await
    });

    // Lock LEZ — watcher should emit Locked.
    maker.lock(hashlock, env.taker_id, 1000).await.unwrap();

    let event = tokio::time::timeout(Duration::from_secs(15), rx.recv())
        .await
        .expect("timed out waiting for Locked event")
        .expect("channel closed");
    assert!(matches!(event, watcher::LezHtlcEvent::Locked { .. }));

    // Wait for lock block confirmation before claiming.
    tokio::time::sleep(BLOCK_WAIT).await;

    // Claim LEZ — watcher should emit Claimed.
    taker.claim(&hashlock, &preimage).await.unwrap();

    let event = tokio::time::timeout(Duration::from_secs(15), rx.recv())
        .await
        .expect("timed out waiting for Claimed event")
        .expect("channel closed");
    assert!(matches!(event, watcher::LezHtlcEvent::Claimed { .. }));

    watcher_handle.abort();
}
