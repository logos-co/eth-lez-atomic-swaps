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
    error::SwapError,
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

fn now_unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

/// Shared test infrastructure: Anvil + LEZ sequencer + deployed contracts.
struct TestEnv {
    anvil: alloy::node_bindings::AnvilInstance,
    deployer: alloy::providers::DynProvider,
    eth_htlc_address: alloy::primitives::Address,
    maker_eth_addr: alloy::primitives::Address,
    _taker_eth_addr: alloy::primitives::Address,
    maker_lez_key: PrivateKey,
    maker_lez_id: AccountId,
    taker_lez_key: PrivateKey,
    taker_lez_id: AccountId,
    sequencer_url: String,
    _seq_handle: sequencer_runner::SequencerHandle,
    _temp_dir: tempfile::TempDir,
    _indexer_handle: jsonrpsee::server::ServerHandle,
}

impl TestEnv {
    /// Spin up Anvil + LEZ sequencer, deploy both contracts.
    /// `min_timelock_delta`: passed to the EthHTLC constructor.
    async fn setup(min_timelock_delta: u64) -> Self {
        let anvil = Anvil::new().block_time(1).try_spawn().unwrap();
        let maker_eth_signer: PrivateKeySigner = anvil.keys()[0].clone().into();
        let taker_eth_signer: PrivateKeySigner = anvil.keys()[1].clone().into();
        let maker_eth_addr = maker_eth_signer.address();
        let taker_eth_addr = taker_eth_signer.address();

        let deployer = ProviderBuilder::new()
            .wallet(maker_eth_signer.clone())
            .connect_ws(WsConnect::new(anvil.ws_endpoint()))
            .await
            .unwrap()
            .erased();
        let contract = EthHTLC::deploy(&deployer, U256::from(min_timelock_delta))
            .await
            .unwrap();
        let eth_htlc_address = *contract.address();

        let (maker_lez_key, maker_lez_id) = test_lez_key(1);
        let (taker_lez_key, taker_lez_id) = test_lez_key(2);

        let (indexer_addr, indexer_handle) = start_dummy_ws_server().await;

        let config_path =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("configs/test_sequencer.json");
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

        let (seq_handle, seq_addr) =
            sequencer_runner::startup_sequencer(seq_config).await.unwrap();
        let seq_addr = SocketAddr::new("127.0.0.1".parse().unwrap(), seq_addr.port());
        let sequencer_url = format!("http://{seq_addr}");

        let seq_client = SequencerClient::new(Url::parse(&sequencer_url).unwrap()).unwrap();
        let msg = ProgramDeploymentMessage::new(LEZ_HTLC_PROGRAM_ELF.to_vec());
        let tx = ProgramDeploymentTransaction { message: msg };
        seq_client.send_tx_program(tx).await.unwrap();
        tokio::time::sleep(BLOCK_WAIT).await;

        Self {
            anvil,
            deployer,
            eth_htlc_address,
            maker_eth_addr,
            _taker_eth_addr: taker_eth_addr,
            maker_lez_key,
            maker_lez_id,
            taker_lez_key,
            taker_lez_id,
            sequencer_url,
            _seq_handle: seq_handle,
            _temp_dir: temp_dir,
            _indexer_handle: indexer_handle,
        }
    }

    fn swap_config(
        &self,
        eth_pk: &str,
        lez_key: &PrivateKey,
        lez_id: AccountId,
        lez_timelock: u64,
        eth_timelock: u64,
    ) -> SwapConfig {
        SwapConfig {
            eth_rpc_url: self.anvil.ws_endpoint(),
            eth_private_key: eth_pk.to_string(),
            eth_htlc_address: self.eth_htlc_address,
            lez_sequencer_url: self.sequencer_url.clone(),
            lez_signing_key: hex::encode(lez_key.value()),
            lez_htlc_program_id: LEZ_HTLC_PROGRAM_ID,
            lez_amount: 1000,
            eth_amount: 1_000_000,
            lez_timelock,
            eth_timelock,
            eth_recipient_address: self.maker_eth_addr,
            lez_taker_account_id: self.taker_lez_id,
            poll_interval: Duration::from_millis(500),
        }
    }

    fn maker_config(&self, lez_timelock: u64, eth_timelock: u64) -> SwapConfig {
        self.swap_config(
            &hex::encode(self.anvil.keys()[0].to_bytes()),
            &self.maker_lez_key,
            self.maker_lez_id,
            lez_timelock,
            eth_timelock,
        )
    }

    fn taker_config(&self, lez_timelock: u64, eth_timelock: u64) -> SwapConfig {
        self.swap_config(
            &hex::encode(self.anvil.keys()[1].to_bytes()),
            &self.taker_lez_key,
            self.taker_lez_id,
            lez_timelock,
            eth_timelock,
        )
    }

    fn lez_client(&self, config: &SwapConfig) -> LezClient {
        LezClient::new(config).unwrap()
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn test_atomic_swap_happy_path() {
    let env = TestEnv::setup(60).await;
    let preimage = [0xABu8; 32];
    let hashlock: [u8; 32] = Sha256::digest(preimage).into();

    let now = now_unix();
    let maker_config = env.maker_config(now + 600, now + 300);
    let taker_config = env.taker_config(now + 600, now + 300);

    // ── Capture initial balances ──
    let balance_lez = env.lez_client(&maker_config);
    let maker_lez_before = balance_lez.get_balance(&env.maker_lez_id).await.unwrap();
    let taker_lez_before = balance_lez.get_balance(&env.taker_lez_id).await.unwrap();
    let maker_eth_before = env.deployer.get_balance(env.maker_eth_addr).await.unwrap();
    let taker_eth_before = env.deployer.get_balance(env._taker_eth_addr).await.unwrap();

    // ── Run maker + taker ──
    let maker_handle = tokio::spawn(async move {
        let eth = EthClient::new(&maker_config).await.unwrap();
        let lez = LezClient::new(&maker_config).unwrap();
        maker::run_maker(&maker_config, &eth, &lez, Some(preimage), None).await
    });
    tokio::time::sleep(Duration::from_secs(10)).await;

    let taker_handle = tokio::spawn(async move {
        let eth = EthClient::new(&taker_config).await.unwrap();
        let lez = LezClient::new(&taker_config).unwrap();
        taker::run_taker(&taker_config, &eth, &lez, hashlock, None).await
    });

    let (maker_result, taker_result) = tokio::join!(maker_handle, taker_handle);
    let maker_outcome = maker_result.unwrap().unwrap();
    let taker_outcome = taker_result.unwrap().unwrap();

    assert!(matches!(maker_outcome, SwapOutcome::Completed { .. }));
    assert!(matches!(taker_outcome, SwapOutcome::Completed { .. }));

    // ── Verify balances ──
    tokio::time::sleep(BLOCK_WAIT).await;

    // LEZ: exact (no gas).
    let maker_lez_after = balance_lez.get_balance(&env.maker_lez_id).await.unwrap();
    let taker_lez_after = balance_lez.get_balance(&env.taker_lez_id).await.unwrap();
    assert_eq!(maker_lez_before - maker_lez_after, 1000, "maker should have spent 1000 LEZ");
    assert_eq!(taker_lez_after - taker_lez_before, 1000, "taker should have gained 1000 LEZ");

    // ETH: contract drained, taker lost >= eth_amount, maker only spent gas.
    let contract_balance = env.deployer.get_balance(env.eth_htlc_address).await.unwrap();
    assert_eq!(contract_balance, U256::ZERO, "HTLC contract should be empty");

    let taker_eth_after = env.deployer.get_balance(env._taker_eth_addr).await.unwrap();
    assert!(
        taker_eth_before - taker_eth_after >= U256::from(1_000_000u64),
        "taker should have spent at least 1M wei"
    );

    let maker_eth_after = env.deployer.get_balance(env.maker_eth_addr).await.unwrap();
    let maker_eth_spent = maker_eth_before - maker_eth_after;
    assert!(
        maker_eth_spent < U256::from(500_000_000_000_000u64),
        "maker should have only spent gas (spent={maker_eth_spent})"
    );
}

// ── Edge case: maker times out when taker never locks ETH ──

#[tokio::test(flavor = "multi_thread")]
async fn test_maker_refunds_on_timeout() {
    let env = TestEnv::setup(60).await;
    let preimage = [0xCDu8; 32];

    // Set LEZ timelock to expire very soon (3s). The maker will lock LEZ, then
    // time out waiting for the taker to lock ETH, and refund.
    let now = now_unix();
    let maker_config = env.maker_config(now + 3, now + 300);

    let balance_lez = env.lez_client(&maker_config);
    let maker_lez_before = balance_lez.get_balance(&env.maker_lez_id).await.unwrap();

    let outcome = {
        let eth = EthClient::new(&maker_config).await.unwrap();
        let lez = LezClient::new(&maker_config).unwrap();
        maker::run_maker(&maker_config, &eth, &lez, Some(preimage), None).await.unwrap()
    };

    assert!(
        matches!(outcome, SwapOutcome::Refunded { lez_refund_tx: Some(_), .. }),
        "maker should have refunded LEZ, got: {outcome:?}"
    );

    // Wait for LEZ refund block.
    tokio::time::sleep(BLOCK_WAIT).await;

    // Maker's LEZ balance should be restored.
    let maker_lez_after = balance_lez.get_balance(&env.maker_lez_id).await.unwrap();
    assert_eq!(
        maker_lez_after, maker_lez_before,
        "maker LEZ should be fully restored after refund"
    );

    // ETH contract should be untouched (taker never locked).
    let contract_balance = env.deployer.get_balance(env.eth_htlc_address).await.unwrap();
    assert_eq!(contract_balance, U256::ZERO, "no ETH should have been locked");
}

// ── Edge case: taker times out when maker never claims ETH ──

#[tokio::test(flavor = "multi_thread")]
async fn test_taker_refunds_on_timeout() {
    // Deploy contract with min_timelock_delta=1 so we can use short timelocks.
    let env = TestEnv::setup(1).await;
    let preimage = [0xEFu8; 32];
    let hashlock: [u8; 32] = Sha256::digest(preimage).into();

    let now = now_unix();
    // ETH timelock = now + 10s. Enough for the lock tx to be mined (needs
    // block.timestamp + minTimelockDelta < timelock), but short enough that
    // the wall-clock timeout fires quickly. We then fast-forward Anvil so
    // the on-chain refund succeeds.
    let eth_timelock = now + 10;
    let maker_config = env.maker_config(now + 600, eth_timelock);
    let taker_config = env.taker_config(now + 600, eth_timelock);

    // Maker locks LEZ manually (not using run_maker, which would also claim ETH).
    let maker_lez = LezClient::new(&maker_config).unwrap();
    maker_lez
        .lock(hashlock, env.taker_lez_id, 1000)
        .await
        .unwrap();
    tokio::time::sleep(BLOCK_WAIT).await;

    // Spawn a background task that fast-forwards Anvil time after taker locks ETH.
    let ff_provider = ProviderBuilder::new()
        .connect_ws(WsConnect::new(env.anvil.ws_endpoint()))
        .await
        .unwrap()
        .erased();
    tokio::spawn(async move {
        // Wait for the taker to lock ETH (a few seconds).
        tokio::time::sleep(Duration::from_secs(4)).await;
        // Fast-forward Anvil past the ETH timelock.
        let _: serde_json::Value = ff_provider
            .raw_request("evm_increaseTime".into(), &[U256::from(300)])
            .await
            .unwrap();
        let _: serde_json::Value = ff_provider
            .raw_request("evm_mine".into(), &())
            .await
            .unwrap();
    });

    // Run taker — should lock ETH, wait, time out, refund ETH.
    let outcome = {
        let eth = EthClient::new(&taker_config).await.unwrap();
        let lez = LezClient::new(&taker_config).unwrap();
        taker::run_taker(&taker_config, &eth, &lez, hashlock, None).await.unwrap()
    };

    assert!(
        matches!(outcome, SwapOutcome::Refunded { eth_refund_tx: Some(_), .. }),
        "taker should have refunded ETH, got: {outcome:?}"
    );

    // ETH contract should be drained (refund returned the ETH to taker).
    let contract_balance = env.deployer.get_balance(env.eth_htlc_address).await.unwrap();
    assert_eq!(contract_balance, U256::ZERO, "ETH should be refunded from contract");
}

// ── Edge case: taker rejects missing escrow ──

#[tokio::test(flavor = "multi_thread")]
async fn test_taker_rejects_missing_escrow() {
    let env = TestEnv::setup(60).await;
    let hashlock = [0xFFu8; 32]; // no escrow exists for this hashlock

    let now = now_unix();
    let taker_config = env.taker_config(now + 600, now + 300);

    let eth = EthClient::new(&taker_config).await.unwrap();
    let lez = LezClient::new(&taker_config).unwrap();
    let result = taker::run_taker(&taker_config, &eth, &lez, hashlock, None).await;

    assert!(result.is_err(), "taker should reject missing escrow");
    let err = result.unwrap_err();
    assert!(
        matches!(err, SwapError::InvalidState { .. }),
        "should be InvalidState error, got: {err}"
    );
}

// ── Edge case: taker rejects escrow with insufficient amount ──

#[tokio::test(flavor = "multi_thread")]
async fn test_taker_rejects_insufficient_escrow_amount() {
    let env = TestEnv::setup(60).await;
    let preimage = [0xBBu8; 32];
    let hashlock: [u8; 32] = Sha256::digest(preimage).into();

    let now = now_unix();
    let maker_config = env.maker_config(now + 600, now + 300);
    let taker_config = env.taker_config(now + 600, now + 300);

    // Maker locks only 500 LEZ — less than the 1000 the taker expects.
    let maker_lez = LezClient::new(&maker_config).unwrap();
    maker_lez
        .lock(hashlock, env.taker_lez_id, 500)
        .await
        .unwrap();
    tokio::time::sleep(BLOCK_WAIT).await;

    let eth = EthClient::new(&taker_config).await.unwrap();
    let lez = LezClient::new(&taker_config).unwrap();
    let result = taker::run_taker(&taker_config, &eth, &lez, hashlock, None).await;

    assert!(result.is_err(), "taker should reject insufficient amount");
    let err = result.unwrap_err();
    assert!(
        matches!(err, SwapError::InvalidState { .. }),
        "should be InvalidState error, got: {err}"
    );
}
