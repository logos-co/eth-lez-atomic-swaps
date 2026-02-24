use alloy::primitives::U256;
use lez_htlc_program::HTLCState;
use tokio::sync::mpsc;
use tracing::info;

use crate::{
    config::SwapConfig,
    error::{Result, SwapError},
    eth::client::EthClient,
    eth::watcher::{self, EthHtlcEvent},
    lez::client::LezClient,
    swap::{
        progress::{self, ProgressSender, SwapProgress},
        refund::now_unix,
        types::SwapOutcome,
    },
};

/// Run the taker side of an atomic swap.
///
/// The taker verifies the maker's LEZ lock, locks ETH, waits for the
/// maker to claim ETH (which reveals the preimage), then claims LEZ.
pub async fn run_taker(
    config: &SwapConfig,
    eth_client: &EthClient,
    lez_client: &LezClient,
    hashlock: [u8; 32],
    progress: Option<ProgressSender>,
) -> Result<SwapOutcome> {
    // 1. Verify the LEZ escrow is locked with the expected params.
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

    info!("taker: LEZ escrow verified");
    progress::report(&progress, SwapProgress::LezEscrowVerified);

    // 2. Lock ETH.
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

    // 3. Watch for ETH Claimed event (maker reveals preimage).
    progress::report(&progress, SwapProgress::WaitingForPreimage);
    let (tx, mut rx) = mpsc::channel::<EthHtlcEvent>(16);
    let watcher_eth_client = EthClient::new(config).await?;
    let watcher_handle = tokio::spawn(async move {
        let _ = watcher::watch_events(&watcher_eth_client, tx).await;
    });

    let preimage = loop {
        tokio::select! {
            Some(event) = rx.recv() => {
                match event {
                    EthHtlcEvent::Claimed {
                        swap_id: claimed_id,
                        preimage,
                    } if claimed_id == swap_id => {
                        info!("taker: maker claimed ETH, preimage revealed");
                        progress::report(&progress, SwapProgress::PreimageRevealed {
                            preimage: hex::encode(preimage),
                        });
                        break preimage;
                    }
                    EthHtlcEvent::Refunded {
                        swap_id: refunded_id,
                    } if refunded_id == swap_id => {
                        // Maker refunded ETH — this shouldn't happen in the
                        // normal flow but handle it gracefully.
                        watcher_handle.abort();
                        return Ok(SwapOutcome::Refunded {
                            eth_refund_tx: None,
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
                info!("taker: ETH timelock expired, refunding");
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

    // 4. Claim LEZ using the revealed preimage.
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
