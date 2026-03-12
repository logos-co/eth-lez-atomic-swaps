use alloy::primitives::U256;
use lez_htlc_program::HTLCState;
use sha2::{Digest, Sha256};
use tokio::sync::mpsc;
use tracing::info;

use crate::{
    config::SwapConfig,
    error::{Result, SwapError},
    eth::client::EthClient,
    lez::client::LezClient,
    lez::watcher::{self, LezHtlcEvent},
    swap::{
        progress::{self, ProgressSender, SwapProgress},
        refund::now_unix,
        types::SwapOutcome,
    },
};

/// Run the taker side of an atomic swap (taker-locks-first).
///
/// The taker generates a secret preimage, locks ETH first (long timelock),
/// waits for the maker to lock LEZ (short timelock), then claims LEZ
/// (revealing the preimage on the LEZ chain).
///
/// If `override_preimage` is `Some`, uses it instead of generating a random one.
/// This is useful for testing/demo where determinism is needed.
pub async fn run_taker(
    config: &SwapConfig,
    eth_client: &EthClient,
    lez_client: &LezClient,
    override_preimage: Option<[u8; 32]>,
    progress: Option<ProgressSender>,
) -> Result<SwapOutcome> {
    // 1. Generate preimage and compute hashlock.
    let preimage: [u8; 32] = override_preimage.unwrap_or_else(rand::random);
    let hashlock: [u8; 32] = Sha256::digest(preimage).into();
    info!(hashlock = hex::encode(hashlock), "taker: generated preimage");
    progress::report(
        &progress,
        SwapProgress::PreimageGenerated {
            hashlock: hex::encode(hashlock),
        },
    );

    // 2. Lock ETH (long timelock).
    progress::report(&progress, SwapProgress::LockingEth);
    let swap_id = eth_client
        .lock(
            hashlock,
            config.eth_timelock,
            config.eth_recipient_address,
            U256::from(config.eth_amount),
        )
        .await?;
    info!(%swap_id, "taker: ETH locked");
    progress::report(
        &progress,
        SwapProgress::EthLocked {
            swap_id: format!("{swap_id}"),
        },
    );

    // 3. Watch for LEZ escrow lock from maker.
    progress::report(&progress, SwapProgress::WaitingForLezLock);
    let (tx, mut rx) = mpsc::channel::<LezHtlcEvent>(16);
    let watcher_lez_client = LezClient::new(config)?;
    let poll_interval = config.poll_interval;
    let watcher_handle = tokio::spawn(async move {
        let _ = watcher::watch_escrow(
            &watcher_lez_client,
            hashlock,
            poll_interval,
            tx,
        )
        .await;
    });

    loop {
        tokio::select! {
            Some(event) = rx.recv() => {
                match event {
                    LezHtlcEvent::Locked { .. } => {
                        info!("taker: LEZ escrow locked by maker");
                        progress::report(&progress, SwapProgress::LezLockDetected);
                        break;
                    }
                    LezHtlcEvent::Refunded { .. } => {
                        // Maker refunded LEZ — swap aborted.
                        watcher_handle.abort();
                        info!("taker: maker refunded LEZ, refunding ETH");
                        progress::report(&progress, SwapProgress::Refunding);
                        let eth_refund_tx = eth_client.refund(swap_id).await.ok();
                        progress::report(&progress, SwapProgress::RefundComplete);
                        return Ok(SwapOutcome::Refunded {
                            eth_refund_tx,
                            lez_refund_tx: None,
                        });
                    }
                    _ => {}
                }
            }
            _ = tokio::time::sleep(std::time::Duration::from_secs(
                config.eth_timelock.saturating_sub(now_unix())
            )) => {
                // ETH timelock expired — refund ETH.
                watcher_handle.abort();
                info!("taker: ETH timelock expired, maker never locked LEZ");
                progress::report(&progress, SwapProgress::TimelockExpired);
                progress::report(&progress, SwapProgress::Refunding);
                let eth_refund_tx = eth_client.refund(swap_id).await.ok();
                progress::report(&progress, SwapProgress::RefundComplete);
                return Ok(SwapOutcome::Refunded {
                    eth_refund_tx,
                    lez_refund_tx: None,
                });
            }
        }
    };

    watcher_handle.abort();

    // 4. Verify LEZ escrow params.
    progress::report(&progress, SwapProgress::VerifyingLezEscrow);
    let escrow = lez_client
        .get_escrow(&hashlock)
        .await?
        .ok_or_else(|| SwapError::InvalidState {
            expected: "Locked escrow".into(),
            actual: "no escrow found".into(),
        })?;

    if escrow.state != HTLCState::Locked {
        return Err(SwapError::InvalidState {
            expected: "Locked".into(),
            actual: format!("{:?}", escrow.state),
        });
    }
    if escrow.amount < config.lez_amount {
        return Err(SwapError::InvalidState {
            expected: format!("amount >= {}", config.lez_amount),
            actual: format!("amount = {}", escrow.amount),
        });
    }
    // Verify the escrow PDA actually holds funds (not a phantom account).
    let pda = lez_client.escrow_pda(&hashlock);
    let pda_balance = lez_client.get_balance(&pda).await?;
    if pda_balance < config.lez_amount {
        return Err(SwapError::InvalidState {
            expected: format!("PDA balance >= {}", config.lez_amount),
            actual: format!("PDA balance = {}", pda_balance),
        });
    }
    info!("taker: LEZ escrow verified");
    progress::report(&progress, SwapProgress::LezEscrowVerified);

    // 5. Claim LEZ (reveals preimage on the LEZ chain).
    progress::report(&progress, SwapProgress::ClaimingLez);
    let lez_claim_tx = lez_client.claim(&hashlock, &preimage).await?;
    info!(tx_hash = %lez_claim_tx, "taker: LEZ claimed");
    progress::report(
        &progress,
        SwapProgress::LezClaimed {
            tx_hash: lez_claim_tx.clone(),
        },
    );

    Ok(SwapOutcome::Completed {
        preimage,
        eth_tx: swap_id,
        lez_tx: lez_claim_tx,
    })
}
