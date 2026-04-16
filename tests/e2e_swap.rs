use std::path::PathBuf;
use std::time::Duration;

use alloy::{
    node_bindings::Anvil,
    primitives::U256,
    providers::{Provider, ProviderBuilder, WsConnect},
    signers::local::PrivateKeySigner,
    sol,
};
use lez_htlc_methods::{LEZ_HTLC_PROGRAM_ELF, LEZ_HTLC_PROGRAM_ID};
use common::transaction::NSSATransaction;
use nssa::{
    AccountId,
    ProgramDeploymentTransaction,
    program_deployment_transaction::Message as ProgramDeploymentMessage,
};
use sequencer_service_rpc::RpcClient as _;
use sha2::{Digest, Sha256};
use swap_orchestrator::{
    config::{LezAuth, SwapConfig},
    eth::client::EthClient,
    lez::client::LezClient,
    scaffold,
    swap::{maker, taker, types::SwapOutcome},
};

const BLOCK_WAIT: Duration = Duration::from_secs(4);

// For deploying the EthHTLC contract (needs full ABI + bytecode).
sol! {
    #[sol(rpc)]
    EthHTLC,
    "contracts/out/EthHTLC.sol/EthHTLC.json"
}

fn now_unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

/// Shared test infrastructure: Anvil + scaffold LEZ localnet + deployed contracts.
struct TestEnv {
    anvil: alloy::node_bindings::AnvilInstance,
    deployer: alloy::providers::DynProvider,
    eth_htlc_address: alloy::primitives::Address,
    maker_eth_addr: alloy::primitives::Address,
    _taker_eth_addr: alloy::primitives::Address,
    maker_lez_id: AccountId,
    taker_lez_id: AccountId,
    sequencer_url: String,
    wallet_home: PathBuf,
}

impl TestEnv {
    /// Spin up Anvil + scaffold LEZ, deploy both contracts.
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

        // Scaffold LEZ setup via WalletCore.
        let wc = scaffold::wallet_core(&scaffold::wallet_home()).expect("scaffold wallet not found — run `make setup` first");
        let accounts = scaffold::public_accounts(&wc).unwrap();
        let maker_lez_id = accounts[0].account_id;
        let taker_lez_id = accounts[1].account_id;
        let sequencer_url = scaffold::sequencer_url_of(&wc);
        let wallet_home = scaffold::wallet_home();

        scaffold::wallet_topup(Some(&accounts[0].account_id_b58)).await.unwrap();
        scaffold::wallet_topup(Some(&accounts[1].account_id_b58)).await.unwrap();

        // Deploy LEZ HTLC program.
        let msg = ProgramDeploymentMessage::new(LEZ_HTLC_PROGRAM_ELF.to_vec());
        let tx = ProgramDeploymentTransaction { message: msg };
        wc.sequencer_client.send_transaction(NSSATransaction::ProgramDeployment(tx)).await.unwrap();
        tokio::time::sleep(BLOCK_WAIT).await;

        Self {
            anvil,
            deployer,
            eth_htlc_address,
            maker_eth_addr,
            _taker_eth_addr: taker_eth_addr,
            maker_lez_id,
            taker_lez_id,
            sequencer_url,
            wallet_home,
        }
    }

    fn swap_config(
        &self,
        eth_pk: &str,
        lez_id: AccountId,
        lez_timelock: u64,
        eth_timelock: u64,
    ) -> SwapConfig {
        SwapConfig {
            eth_rpc_url: self.anvil.ws_endpoint(),
            eth_private_key: eth_pk.to_string(),
            eth_htlc_address: self.eth_htlc_address,
            lez_sequencer_url: self.sequencer_url.clone(),
            lez_auth: LezAuth::Wallet {
                home: self.wallet_home.clone(),
                account_id: lez_id,
            },
            lez_htlc_program_id: LEZ_HTLC_PROGRAM_ID,
            lez_amount: 1000,
            eth_amount: 1_000_000,
            lez_timelock,
            eth_timelock,
            eth_recipient_address: self.maker_eth_addr,
            lez_taker_account_id: self.taker_lez_id,
            poll_interval: Duration::from_millis(500),
            messaging: None,
        }
    }

    fn maker_config(&self, lez_timelock: u64, eth_timelock: u64) -> SwapConfig {
        self.swap_config(
            &hex::encode(self.anvil.keys()[0].to_bytes()),
            self.maker_lez_id,
            lez_timelock,
            eth_timelock,
        )
    }

    fn taker_config(&self, lez_timelock: u64, eth_timelock: u64) -> SwapConfig {
        self.swap_config(
            &hex::encode(self.anvil.keys()[1].to_bytes()),
            self.taker_lez_id,
            lez_timelock,
            eth_timelock,
        )
    }

    fn lez_client(&self, config: &SwapConfig) -> LezClient {
        LezClient::new(config).unwrap()
    }
}

/// Happy path: taker locks ETH first, maker locks LEZ, taker claims LEZ, maker claims ETH.
#[tokio::test(flavor = "multi_thread")]
async fn test_atomic_swap_happy_path() {
    let env = TestEnv::setup(60).await;
    let preimage = [0xABu8; 32];
    let hashlock: [u8; 32] = Sha256::digest(preimage).into();

    let now = now_unix();
    // Taker-locks-first: ETH timelock longer, LEZ timelock shorter.
    let maker_config = env.maker_config(now + 300, now + 600);
    let taker_config = env.taker_config(now + 300, now + 600);

    // ── Capture initial balances ──
    let balance_lez = env.lez_client(&maker_config);
    let maker_lez_before = balance_lez.get_balance(&env.maker_lez_id).await.unwrap();
    let taker_lez_before = balance_lez.get_balance(&env.taker_lez_id).await.unwrap();
    let maker_eth_before = env.deployer.get_balance(env.maker_eth_addr).await.unwrap();
    let taker_eth_before = env.deployer.get_balance(env._taker_eth_addr).await.unwrap();

    // ── Run maker + taker concurrently ──
    // Maker starts first (watches for ETH lock).
    let maker_handle = tokio::spawn(async move {
        let eth = EthClient::new(&maker_config).await.unwrap();
        let lez = LezClient::new(&maker_config).unwrap();
        maker::run_maker(&maker_config, &eth, &lez, Some(hashlock), None, None).await
    });

    // Brief pause so maker's ETH watcher is ready before taker locks.
    tokio::time::sleep(Duration::from_secs(3)).await;

    // Taker generates preimage, locks ETH, waits for LEZ lock, claims LEZ.
    let taker_handle = tokio::spawn(async move {
        let eth = EthClient::new(&taker_config).await.unwrap();
        let lez = LezClient::new(&taker_config).unwrap();
        taker::run_taker(&taker_config, &eth, &lez, Some(preimage), None).await
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
    let hashlock = [0xCDu8; 32]; // arbitrary hashlock, no taker will lock

    // Set ETH timelock to expire very soon (3s). The maker will time out
    // waiting for the taker to lock ETH.
    let now = now_unix();
    let maker_config = env.maker_config(now + 300, now + 3);

    let outcome = {
        let eth = EthClient::new(&maker_config).await.unwrap();
        let lez = LezClient::new(&maker_config).unwrap();
        maker::run_maker(&maker_config, &eth, &lez, Some(hashlock), None, None).await.unwrap()
    };

    // Maker should return Refunded with no txs (never locked anything).
    assert!(
        matches!(outcome, SwapOutcome::Refunded { .. }),
        "maker should have timed out, got: {outcome:?}"
    );

    // ETH contract should be untouched (taker never locked).
    let contract_balance = env.deployer.get_balance(env.eth_htlc_address).await.unwrap();
    assert_eq!(contract_balance, U256::ZERO, "no ETH should have been locked");
}

// ── Edge case: taker times out when maker never locks LEZ ──

#[tokio::test(flavor = "multi_thread")]
async fn test_taker_refunds_on_timeout() {
    // Deploy contract with min_timelock_delta=1 so we can use short timelocks.
    let env = TestEnv::setup(1).await;
    let preimage = [0xEFu8; 32];

    let now = now_unix();
    // ETH timelock = now + 10s. Short enough to time out quickly.
    let eth_timelock = now + 10;
    let taker_config = env.taker_config(now + 300, eth_timelock);

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

    // Run taker — should lock ETH, wait for LEZ lock, time out, refund ETH.
    let outcome = {
        let eth = EthClient::new(&taker_config).await.unwrap();
        let lez = LezClient::new(&taker_config).unwrap();
        taker::run_taker(&taker_config, &eth, &lez, Some(preimage), None).await.unwrap()
    };

    assert!(
        matches!(outcome, SwapOutcome::Refunded { eth_refund_tx: Some(_), .. }),
        "taker should have refunded ETH, got: {outcome:?}"
    );

    // ETH contract should be drained (refund returned the ETH to taker).
    let contract_balance = env.deployer.get_balance(env.eth_htlc_address).await.unwrap();
    assert_eq!(contract_balance, U256::ZERO, "ETH should be refunded from contract");
}
