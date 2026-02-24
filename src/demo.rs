use std::net::SocketAddr;
use std::path::PathBuf;
use std::time::Duration;

use alloy::node_bindings::AnvilInstance;
use alloy::primitives::U256;
use alloy::providers::{Provider, ProviderBuilder, WsConnect};
use alloy::signers::local::PrivateKeySigner;
use alloy::sol;
use common::sequencer_client::SequencerClient;
use lez_htlc_methods::{LEZ_HTLC_PROGRAM_ELF, LEZ_HTLC_PROGRAM_ID};
use nssa::{
    AccountId, PrivateKey, PublicKey, ProgramDeploymentTransaction,
    program_deployment_transaction::Message as ProgramDeploymentMessage,
};
use sequencer_core::config::{AccountInitialData, SequencerConfig};
use sha2::{Digest, Sha256};
use url::Url;

use crate::config::SwapConfig;
use crate::swap::refund::now_unix;

const BLOCK_WAIT: Duration = Duration::from_secs(4);

sol! {
    #[sol(rpc)]
    EthHTLC,
    "contracts/out/EthHTLC.sol/EthHTLC.json"
}

/// Callback for reporting demo setup progress.
pub type SetupProgressFn = Box<dyn Fn(usize, &str, &str) + Send>;

/// Self-contained demo environment: Anvil + LEZ sequencer + deployed contracts.
///
/// All services are started in-process and cleaned up on drop.
pub struct DemoEnv {
    _anvil: AnvilInstance,
    _seq_handle: sequencer_runner::SequencerHandle,
    _temp_dir: tempfile::TempDir,
    _indexer_handle: jsonrpsee::server::ServerHandle,
    pub maker_config: SwapConfig,
    pub taker_config: SwapConfig,
    pub hashlock: [u8; 32],
    pub preimage: [u8; 32],
}

fn lez_key(seed: u8) -> (PrivateKey, AccountId) {
    let key = PrivateKey::try_new([seed; 32]).unwrap();
    let pub_key = PublicKey::new_from_private_key(&key);
    let id = AccountId::from(&pub_key);
    (key, id)
}

/// Start a no-op JSON-RPC WebSocket server (indexer stub required by the sequencer).
async fn start_dummy_ws_server() -> (SocketAddr, jsonrpsee::server::ServerHandle) {
    let server = jsonrpsee::server::Server::builder()
        .build("127.0.0.1:0")
        .await
        .unwrap();
    let addr = server.local_addr().unwrap();
    let handle = server.start(jsonrpsee::RpcModule::new(()));
    (addr, handle)
}

impl DemoEnv {
    /// Spin up all infrastructure and return a ready-to-run demo environment.
    ///
    /// `on_progress` is called with `(step_number, label, detail)` for each setup phase.
    pub async fn start(on_progress: Option<SetupProgressFn>) -> Self {
        let report = |step: usize, label: &str, detail: &str| {
            if let Some(f) = &on_progress {
                f(step, label, detail);
            }
        };

        // 1. Start Anvil.
        report(1, "Starting Anvil", "");
        let anvil = alloy::node_bindings::Anvil::new()
            .block_time(1)
            .try_spawn()
            .unwrap();
        let anvil_ws = anvil.ws_endpoint();
        report(1, "Starting Anvil", &anvil_ws);

        // 2. Deploy EthHTLC contract.
        report(2, "Deploying EthHTLC contract", "");
        let maker_eth_signer: PrivateKeySigner = anvil.keys()[0].clone().into();
        let maker_eth_addr = maker_eth_signer.address();

        let deployer = ProviderBuilder::new()
            .wallet(maker_eth_signer)
            .connect_ws(WsConnect::new(&anvil_ws))
            .await
            .unwrap()
            .erased();
        let contract = EthHTLC::deploy(&deployer, U256::from(60u64))
            .await
            .unwrap();
        let eth_htlc_address = *contract.address();
        report(2, "Deploying EthHTLC contract", &format!("{eth_htlc_address}"));

        // 3. Generate LEZ accounts.
        report(3, "Generating LEZ accounts", "");
        let (maker_lez_key, maker_lez_id) = lez_key(1);
        let (taker_lez_key, taker_lez_id) = lez_key(2);
        report(3, "Generating LEZ accounts", "maker + taker");

        // 4. Start LEZ sequencer.
        report(4, "Starting LEZ sequencer", "");
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
        report(4, "Starting LEZ sequencer", &sequencer_url);

        // 5. Deploy LEZ HTLC program.
        report(5, "Deploying LEZ HTLC program", "");
        let seq_client = SequencerClient::new(Url::parse(&sequencer_url).unwrap()).unwrap();
        let msg = ProgramDeploymentMessage::new(LEZ_HTLC_PROGRAM_ELF.to_vec());
        let tx = ProgramDeploymentTransaction { message: msg };
        seq_client.send_tx_program(tx).await.unwrap();
        tokio::time::sleep(BLOCK_WAIT).await;
        report(5, "Deploying LEZ HTLC program", "deployed");

        // 6. Generate swap preimage.
        report(6, "Generating swap preimage", "");
        let preimage: [u8; 32] = rand::random();
        let hashlock: [u8; 32] = Sha256::digest(preimage).into();
        report(6, "Generating swap preimage", &hex::encode(&hashlock[..8]));

        // Build configs.
        let now = now_unix();
        let lez_timelock = now + 600; // 10 minutes
        let eth_timelock = now + 300; // 5 minutes

        let maker_config = SwapConfig {
            eth_rpc_url: anvil.ws_endpoint(),
            eth_private_key: hex::encode(anvil.keys()[0].to_bytes()),
            eth_htlc_address,
            lez_sequencer_url: sequencer_url.clone(),
            lez_signing_key: hex::encode(maker_lez_key.value()),
            lez_htlc_program_id: LEZ_HTLC_PROGRAM_ID,
            lez_amount: 1000,
            eth_amount: 1_000_000,
            lez_timelock,
            eth_timelock,
            eth_recipient_address: maker_eth_addr,
            lez_taker_account_id: taker_lez_id,
            poll_interval: Duration::from_millis(500),
        };

        let taker_config = SwapConfig {
            eth_rpc_url: anvil.ws_endpoint(),
            eth_private_key: hex::encode(anvil.keys()[1].to_bytes()),
            eth_htlc_address,
            lez_sequencer_url: sequencer_url,
            lez_signing_key: hex::encode(taker_lez_key.value()),
            lez_htlc_program_id: LEZ_HTLC_PROGRAM_ID,
            lez_amount: 1000,
            eth_amount: 1_000_000,
            lez_timelock,
            eth_timelock,
            eth_recipient_address: maker_eth_addr,
            lez_taker_account_id: taker_lez_id,
            poll_interval: Duration::from_millis(500),
        };

        Self {
            _anvil: anvil,
            _seq_handle: seq_handle,
            _temp_dir: temp_dir,
            _indexer_handle: indexer_handle,
            maker_config,
            taker_config,
            hashlock,
            preimage,
        }
    }
}
