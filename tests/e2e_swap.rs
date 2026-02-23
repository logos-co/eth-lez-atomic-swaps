use std::{net::SocketAddr, path::PathBuf, time::Duration};

use alloy::{
    node_bindings::Anvil,
    primitives::U256,
    providers::{Provider, ProviderBuilder, WsConnect},
    signers::local::PrivateKeySigner,
    sol,
};
use common::sequencer_client::SequencerClient;
use lez_htlc_methods::{LEZ_HTLC_PROGRAM_ELF, LEZ_HTLC_PROGRAM_ID};
use nssa::{
    AccountId, PrivateKey, PublicKey,
    program_deployment_transaction::Message as ProgramDeploymentMessage,
    ProgramDeploymentTransaction,
};
use sequencer_core::config::{AccountInitialData, SequencerConfig};
use sha2::{Digest, Sha256};
use swap_orchestrator::{
    config::SwapConfig,
    eth::client::EthClient,
    lez::client::LezClient,
    swap::{maker, taker, types::SwapOutcome},
};
use url::Url;

const BLOCK_WAIT: Duration = Duration::from_secs(4);

/// Start a no-op JSON-RPC WebSocket server for the sequencer's indexer client.
async fn start_dummy_ws_server() -> (SocketAddr, jsonrpsee::server::ServerHandle) {
    let server = jsonrpsee::server::Server::builder()
        .build("127.0.0.1:0")
        .await
        .unwrap();
    let addr = server.local_addr().unwrap();
    let handle = server.start(jsonrpsee::RpcModule::new(()));
    (addr, handle)
}

// For deploying the EthHTLC contract (needs full ABI + bytecode).
sol! {
    #[sol(rpc)]
    EthHTLC,
    "contracts/out/EthHTLC.sol/EthHTLC.json"
}

fn test_lez_key(seed: u8) -> (PrivateKey, AccountId) {
    let key = PrivateKey::try_new([seed; 32]).unwrap();
    let pub_key = PublicKey::new_from_private_key(&key);
    let id = AccountId::from(&pub_key);
    (key, id)
}

#[tokio::test(flavor = "multi_thread")]
async fn test_atomic_swap_happy_path() {
    // ── Known preimage (so taker can derive hashlock) ──
    let preimage = [0xABu8; 32];
    let hashlock: [u8; 32] = Sha256::digest(preimage).into();

    // ── Spin up Anvil (ETH) ──
    let anvil = Anvil::new().block_time(1).try_spawn().unwrap();
    let maker_eth_signer: PrivateKeySigner = anvil.keys()[0].clone().into();
    let _taker_eth_signer: PrivateKeySigner = anvil.keys()[1].clone().into();
    let maker_eth_addr = maker_eth_signer.address();

    // Deploy EthHTLC contract.
    let deployer = ProviderBuilder::new()
        .wallet(maker_eth_signer.clone())
        .connect_ws(WsConnect::new(anvil.ws_endpoint()))
        .await
        .unwrap()
        .erased();
    let contract = EthHTLC::deploy(&deployer, U256::from(60)).await.unwrap();
    let eth_htlc_address = *contract.address();

    // ── Spin up LEZ sequencer ──
    let (maker_lez_key, maker_lez_id) = test_lez_key(1);
    let (taker_lez_key, taker_lez_id) = test_lez_key(2);

    let (indexer_addr, _indexer_handle) = start_dummy_ws_server().await;

    let config_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("configs/test_sequencer.json");
    let mut seq_config = SequencerConfig::from_path(&config_path).unwrap();
    let temp_dir = tempfile::tempdir().unwrap();
    seq_config.home = temp_dir.path().to_owned();
    seq_config.port = 0;
    seq_config.indexer_rpc_url = format!("ws://127.0.0.1:{}", indexer_addr.port())
        .parse()
        .unwrap();
    seq_config.initial_accounts = vec![
        AccountInitialData {
            account_id: maker_lez_id.to_string(),
            balance: 1_000_000,
        },
        AccountInitialData {
            account_id: taker_lez_id.to_string(),
            balance: 1_000_000,
        },
    ];

    let (_seq_handle, seq_addr) = sequencer_runner::startup_sequencer(seq_config).await.unwrap();
    let seq_addr = SocketAddr::new("127.0.0.1".parse().unwrap(), seq_addr.port());
    let sequencer_url = format!("http://{seq_addr}");

    // Deploy LEZ HTLC program.
    let seq_client = SequencerClient::new(Url::parse(&sequencer_url).unwrap()).unwrap();
    let msg = ProgramDeploymentMessage::new(LEZ_HTLC_PROGRAM_ELF.to_vec());
    let tx = ProgramDeploymentTransaction { message: msg };
    seq_client.send_tx_program(tx).await.unwrap();
    tokio::time::sleep(BLOCK_WAIT).await;

    // ── Timelocks ──
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let lez_timelock = now + 600; // 10 min
    let eth_timelock = now + 300; // 5 min

    // ── Build SwapConfigs ──
    // Both configs share the same addresses:
    //   eth_recipient_address = maker's ETH address (ETH HTLC recipient)
    //   lez_taker_account_id  = taker's LEZ account (LEZ HTLC taker)
    let base_config = |eth_pk: &str, lez_key: &PrivateKey, lez_id: AccountId| -> SwapConfig {
        SwapConfig {
            eth_rpc_url: anvil.ws_endpoint(),
            eth_private_key: eth_pk.to_string(),
            eth_htlc_address,
            lez_sequencer_url: sequencer_url.clone(),
            lez_signing_key: hex::encode(lez_key.value()),
            lez_account_id: lez_id,
            lez_htlc_program_id: LEZ_HTLC_PROGRAM_ID,
            lez_amount: 1000,
            eth_amount: 1_000_000, // wei
            lez_timelock,
            eth_timelock,
            eth_recipient_address: maker_eth_addr,
            lez_taker_account_id: taker_lez_id,
            poll_interval: Duration::from_millis(500),
        }
    };

    let maker_config = base_config(
        &hex::encode(anvil.keys()[0].to_bytes()),
        &maker_lez_key,
        maker_lez_id,
    );
    let taker_config = base_config(
        &hex::encode(anvil.keys()[1].to_bytes()),
        &taker_lez_key,
        taker_lez_id,
    );

    // ── Run maker (locks LEZ, waits for ETH, claims ETH) ──
    let maker_handle = tokio::spawn(async move {
        let eth = EthClient::new(&maker_config).await.unwrap();
        let lez = LezClient::new(&maker_config).unwrap();
        maker::run_maker(&maker_config, &eth, &lez, Some(preimage)).await
    });

    // Wait for maker to lock LEZ and start ETH watcher.
    tokio::time::sleep(Duration::from_secs(10)).await;

    // ── Run taker (verifies LEZ escrow, locks ETH, waits for claim, claims LEZ) ──
    let taker_handle = tokio::spawn(async move {
        let eth = EthClient::new(&taker_config).await.unwrap();
        let lez = LezClient::new(&taker_config).unwrap();
        taker::run_taker(&taker_config, &eth, &lez, hashlock).await
    });

    // Both should complete within a reasonable time.
    let (maker_result, taker_result) = tokio::join!(maker_handle, taker_handle);
    let maker_outcome = maker_result.unwrap().unwrap();
    let taker_outcome = taker_result.unwrap().unwrap();

    assert!(
        matches!(maker_outcome, SwapOutcome::Completed { .. }),
        "maker should complete"
    );
    assert!(
        matches!(taker_outcome, SwapOutcome::Completed { .. }),
        "taker should complete"
    );
}
