use std::time::Duration;

use alloy::node_bindings::AnvilInstance;
use alloy::primitives::U256;
use alloy::providers::{Provider, ProviderBuilder, WsConnect};
use alloy::signers::local::PrivateKeySigner;
use alloy::sol;
use lez_htlc_methods::{LEZ_HTLC_PROGRAM_ELF, LEZ_HTLC_PROGRAM_ID};
use nssa::{
    ProgramDeploymentTransaction,
    program_deployment_transaction::Message as ProgramDeploymentMessage,
};

use crate::config::{LezAuth, SwapConfig};
use crate::error::{Result, SwapError};
use crate::scaffold;
use crate::swap::refund::now_unix;

const BLOCK_WAIT: Duration = Duration::from_secs(4);

sol! {
    #[sol(rpc)]
    EthHTLC,
    "contracts/out/EthHTLC.sol/EthHTLC.json"
}

/// Callback for reporting demo setup progress.
pub type SetupProgressFn = Box<dyn Fn(usize, &str, &str) + Send>;

/// Demo environment: Anvil (in-process) + scaffold LEZ localnet + deployed contracts.
///
/// Requires `logos-scaffold setup` to have been run. The LEZ sequencer must be
/// started externally (via `logos-scaffold localnet start` or `make infra`).
pub struct DemoEnv {
    pub anvil_stdout: Option<std::process::ChildStdout>,
    _anvil: AnvilInstance,
    pub maker_config: SwapConfig,
    pub taker_config: SwapConfig,
    pub taker_eth_private_key: String,
}

impl DemoEnv {
    /// Spin up Anvil, deploy contracts, and build configs from scaffold wallet.
    ///
    /// Assumes scaffold localnet is already running on the port configured in the
    /// wallet config (default: 3040).
    ///
    /// `on_progress` is called with `(step_number, label, detail)` for each setup phase.
    pub async fn start(on_progress: Option<SetupProgressFn>) -> Result<Self> {
        let report = |step: usize, label: &str, detail: &str| {
            if let Some(f) = &on_progress {
                f(step, label, detail);
            }
        };

        // 1. Read scaffold wallet via WalletCore.
        report(1, "Reading scaffold wallet", "");
        let wc = scaffold::wallet_core(&scaffold::wallet_home())?;
        report(1, "Reading scaffold wallet", "OK");

        // 2. Start Anvil.
        report(2, "Starting Anvil", "");
        let mut anvil = alloy::node_bindings::Anvil::new()
            .block_time(1)
            .keep_stdout()
            .try_spawn()
            .map_err(|e| SwapError::InvalidConfig(format!("failed to start Anvil: {e}")))?;
        let anvil_ws = anvil.ws_endpoint();
        let anvil_stdout = anvil.child_mut().stdout.take();
        report(2, "Starting Anvil", &anvil_ws);

        // 3. Deploy EthHTLC contract.
        report(3, "Deploying EthHTLC contract", "");
        let maker_eth_signer: PrivateKeySigner = anvil.keys()[0].clone().into();
        let maker_eth_addr = maker_eth_signer.address();

        let deployer = ProviderBuilder::new()
            .wallet(maker_eth_signer)
            .connect_ws(WsConnect::new(&anvil_ws))
            .await
            .map_err(|e| SwapError::EthRpc(format!("WS connect failed: {e}")))?
            .erased();
        let contract = EthHTLC::deploy(&deployer, U256::from(60u64))
            .await
            .map_err(|e| SwapError::EthRpc(format!("EthHTLC deploy failed: {e}")))?;
        let eth_htlc_address = *contract.address();
        report(3, "Deploying EthHTLC contract", &format!("{eth_htlc_address}"));

        // 4. Extract accounts and sequencer URL from WalletCore.
        report(4, "Reading wallet accounts", "");
        let accounts = scaffold::public_accounts(&wc)?;
        let wallet_home = scaffold::wallet_home();
        let sequencer_url = scaffold::sequencer_url_of(&wc);
        report(
            4,
            "Reading wallet accounts",
            &format!(
                "maker={} taker={}",
                &accounts[0].account_id_b58[..8],
                &accounts[1].account_id_b58[..8]
            ),
        );

        // 5. Fund accounts + deploy LEZ HTLC program.
        report(5, "Deploying LEZ HTLC program", "");
        scaffold::wallet_topup(Some(&accounts[0].account_id_b58)).await?;
        scaffold::wallet_topup(Some(&accounts[1].account_id_b58)).await?;

        let msg = ProgramDeploymentMessage::new(LEZ_HTLC_PROGRAM_ELF.to_vec());
        let tx = ProgramDeploymentTransaction { message: msg };
        wc.sequencer_client
            .send_tx_program(tx)
            .await
            .map_err(|e| SwapError::LezTransaction(format!("program deploy failed: {e}")))?;
        tokio::time::sleep(BLOCK_WAIT).await;
        report(5, "Deploying LEZ HTLC program", "deployed");

        // Build configs.
        let now = now_unix();
        let eth_timelock = now + 600; // 10 minutes (taker locks ETH first, needs longer)
        let lez_timelock = now + 300; // 5 minutes (maker locks LEZ second, shorter)

        let maker_config = SwapConfig {
            eth_rpc_url: anvil.ws_endpoint(),
            eth_private_key: hex::encode(anvil.keys()[0].to_bytes()),
            eth_htlc_address,
            lez_sequencer_url: sequencer_url.clone(),
            lez_auth: LezAuth::Wallet {
                home: wallet_home.clone(),
                account_id: accounts[0].account_id,
            },
            lez_htlc_program_id: LEZ_HTLC_PROGRAM_ID,
            lez_amount: 1000,
            eth_amount: 1_000_000,
            lez_timelock,
            eth_timelock,
            eth_recipient_address: maker_eth_addr,
            lez_taker_account_id: accounts[1].account_id,
            poll_interval: Duration::from_millis(500),
            nwaku_url: None,
        };

        let taker_config = SwapConfig {
            eth_rpc_url: anvil.ws_endpoint(),
            eth_private_key: hex::encode(anvil.keys()[1].to_bytes()),
            eth_htlc_address,
            lez_sequencer_url: sequencer_url,
            lez_auth: LezAuth::Wallet {
                home: wallet_home,
                account_id: accounts[1].account_id,
            },
            lez_htlc_program_id: LEZ_HTLC_PROGRAM_ID,
            lez_amount: 1000,
            eth_amount: 1_000_000,
            lez_timelock,
            eth_timelock,
            eth_recipient_address: maker_eth_addr,
            lez_taker_account_id: accounts[1].account_id,
            poll_interval: Duration::from_millis(500),
            nwaku_url: None,
        };

        let taker_eth_private_key = hex::encode(anvil.keys()[1].to_bytes());

        Ok(Self {
            anvil_stdout,
            _anvil: anvil,
            maker_config,
            taker_config,
            taker_eth_private_key,
        })
    }
}
