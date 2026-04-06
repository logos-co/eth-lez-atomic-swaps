use std::path::PathBuf;
use std::time::Duration;

use lez_htlc_methods::{LEZ_HTLC_PROGRAM_ELF, LEZ_HTLC_PROGRAM_ID};
use lez_htlc_program::HTLCState;
use common::transaction::NSSATransaction;
use nssa::{AccountId, ProgramDeploymentTransaction, program_deployment_transaction::Message as ProgramDeploymentMessage};
use nssa_core::program::ProgramId;
use sequencer_service_rpc::RpcClient as _;
use sha2::{Digest, Sha256};
use swap_orchestrator::{
    config::{LezAuth, SwapConfig},
    lez::{client::LezClient, watcher},
    scaffold,
};
use tokio::sync::mpsc;

const BLOCK_WAIT: Duration = Duration::from_secs(4);

fn make_preimage_and_hashlock(seed: u8) -> ([u8; 32], [u8; 32]) {
    let preimage = [seed; 32];
    let hashlock: [u8; 32] = Sha256::digest(preimage).into();
    (preimage, hashlock)
}

struct TestEnv {
    program_id: ProgramId,
    maker_id: AccountId,
    taker_id: AccountId,
    sequencer_url: String,
    wallet_home: PathBuf,
}

impl TestEnv {
    fn lez_client_for(&self, account_id: AccountId, counterparty_lez: AccountId) -> LezClient {
        let config = SwapConfig {
            lez_auth: LezAuth::Wallet {
                home: self.wallet_home.clone(),
                account_id,
            },
            lez_sequencer_url: self.sequencer_url.clone(),
            lez_htlc_program_id: self.program_id,
            lez_taker_account_id: counterparty_lez,
            // Unused by LezClient::new:
            eth_rpc_url: String::new(),
            eth_private_key: String::new(),
            eth_htlc_address: alloy::primitives::Address::ZERO,
            lez_amount: 0,
            eth_amount: 0,
            lez_timelock: 0,
            eth_timelock: 0,
            eth_recipient_address: alloy::primitives::Address::ZERO,
            poll_interval: Duration::from_millis(500),
            nwaku_url: None,
        };
        LezClient::new(&config).unwrap()
    }

    fn maker_client(&self) -> LezClient {
        self.lez_client_for(self.maker_id, self.taker_id)
    }

    fn taker_client(&self) -> LezClient {
        self.lez_client_for(self.taker_id, self.maker_id)
    }
}

async fn setup() -> TestEnv {
    // Read scaffold wallet via WalletCore.
    let wc = scaffold::wallet_core(&scaffold::wallet_home()).expect("scaffold wallet not found — run `make setup` first");
    let accounts = scaffold::public_accounts(&wc).unwrap();
    let maker_id = accounts[0].account_id;
    let taker_id = accounts[1].account_id;
    let sequencer_url = scaffold::sequencer_url_of(&wc);
    let wallet_home = scaffold::wallet_home();

    // Fund accounts.
    scaffold::wallet_topup(Some(&accounts[0].account_id_b58)).await.unwrap();
    scaffold::wallet_topup(Some(&accounts[1].account_id_b58)).await.unwrap();

    // Deploy LEZ HTLC program.
    let msg = ProgramDeploymentMessage::new(LEZ_HTLC_PROGRAM_ELF.to_vec());
    let tx = ProgramDeploymentTransaction { message: msg };
    wc.sequencer_client.send_transaction(NSSATransaction::ProgramDeployment(tx)).await.unwrap();

    // Wait for deployment block.
    tokio::time::sleep(BLOCK_WAIT).await;

    TestEnv {
        program_id: LEZ_HTLC_PROGRAM_ID,
        maker_id,
        taker_id,
        sequencer_url,
        wallet_home,
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
    let (_, hashlock) = make_preimage_and_hashlock(0x01);

    // timelock=0 → already expired; not testing timelock enforcement here.
    maker.lock(hashlock, env.taker_id, 1000, 0).await.unwrap();
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
    let (preimage, hashlock) = make_preimage_and_hashlock(0x02);

    maker.lock(hashlock, env.taker_id, 1000, 0).await.unwrap();
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
    let (_, hashlock) = make_preimage_and_hashlock(0x03);

    let maker_before = maker.get_balance(&env.maker_id).await.unwrap();
    // timelock=0 → already expired; not testing timelock enforcement here.
    maker.lock(hashlock, env.taker_id, 1000, 0).await.unwrap();
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
    let (_, hashlock) = make_preimage_and_hashlock(0x04);

    maker.lock(hashlock, env.taker_id, 1000, 0).await.unwrap();
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
    let (preimage, hashlock) = make_preimage_and_hashlock(0x05);

    let (tx, mut rx) = mpsc::channel(16);
    let watcher_client = env.lez_client_for(env.maker_id, env.taker_id);
    let watcher_handle = tokio::spawn(async move {
        watcher::watch_escrow(&watcher_client, hashlock, Duration::from_millis(500), tx).await
    });

    // Lock LEZ — watcher should emit Locked.
    maker.lock(hashlock, env.taker_id, 1000, 0).await.unwrap();

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

/// Validates on-chain timelock enforcement via the LEZ runtime's timestamp
/// validity window. The guest program attaches `with_timestamp_validity_window(timelock..)`
/// to the refund output, so the runtime must reject refund transactions whose
/// block timestamp falls before the timelock.
///
/// Sequence:
///   1. Lock LEZ with a far-future timelock (1 hour from now).
///   2. Attempt refund immediately → the runtime should reject the transaction
///      because the block timestamp is before the validity window.
///   3. Lock LEZ again with an already-expired timelock (in the past).
///   4. Refund → should succeed because the block timestamp satisfies the window.
///
/// This is the primary regression test for on-chain timelock enforcement in the
/// atomic swap flow. The off-chain guard in `src/swap/refund.rs` is bypassed
/// here intentionally — we call `LezClient::refund` directly to exercise the
/// runtime's ValidityWindow check.
#[tokio::test]
async fn test_refund_rejected_before_timelock_accepted_after() {
    use std::time::{SystemTime, UNIX_EPOCH};

    let env = setup().await;
    let maker = env.maker_client();

    let now_secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs();

    // ── Phase 1: early refund must be rejected ──────────────────────
    // Lock with a timelock 1 hour in the future.
    let (_, hashlock_future) = make_preimage_and_hashlock(0x10);
    let future_timelock_secs = now_secs + 3600;
    maker
        .lock(hashlock_future, env.taker_id, 1000, future_timelock_secs)
        .await
        .unwrap();
    tokio::time::sleep(BLOCK_WAIT * 2).await;

    // Refund: the transaction is submitted to the sequencer, but the runtime
    // should reject it because the current block timestamp is before the
    // validity window start (timelock).
    let _ = maker.refund(&hashlock_future).await;
    tokio::time::sleep(BLOCK_WAIT * 2).await;

    // Escrow must still be Locked — the early refund had no on-chain effect.
    let escrow = maker
        .get_escrow(&hashlock_future)
        .await
        .unwrap()
        .expect("escrow should still exist");
    assert_eq!(
        escrow.state,
        HTLCState::Locked,
        "runtime should reject refund before timelock expiry"
    );

    // ── Phase 2: refund after timelock must succeed ─────────────────
    // Lock with an already-expired timelock (1 second in the past).
    let (_, hashlock_past) = make_preimage_and_hashlock(0x11);
    let past_timelock_secs = now_secs.saturating_sub(1);
    let maker_before = maker.get_balance(&env.maker_id).await.unwrap();

    maker
        .lock(hashlock_past, env.taker_id, 1000, past_timelock_secs)
        .await
        .unwrap();
    tokio::time::sleep(BLOCK_WAIT * 2).await;

    maker.refund(&hashlock_past).await.unwrap();
    tokio::time::sleep(BLOCK_WAIT).await;

    let escrow = maker
        .get_escrow(&hashlock_past)
        .await
        .unwrap()
        .expect("escrow should exist after refund");
    assert_eq!(
        escrow.state,
        HTLCState::Refunded,
        "refund should succeed after timelock expiry"
    );

    // Balance restored — maker got the locked amount back.
    let maker_after = maker.get_balance(&env.maker_id).await.unwrap();
    assert_eq!(maker_after, maker_before);
}
