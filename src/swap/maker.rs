use alloy::primitives::U256;
use tokio::sync::mpsc;
use tracing::info;

use crate::{
    config::SwapConfig,
    error::Result,
    eth::client::{EthClient, EthHTLC::SwapState},
    eth::watcher::{self, EthHtlcEvent},
    lez::client::LezClient,
    lez::watcher as lez_watcher,
    lez::watcher::LezHtlcEvent,
    swap::{
        progress::{self, ProgressSender, SwapProgress},
        refund::now_unix,
        types::SwapOutcome,
    },
};

/// Run the maker side of an atomic swap (taker-locks-first).
///
/// The maker optionally receives a hashlock. If `None`, the maker watches for
/// any ETH lock to its recipient address with sufficient amount and extracts
/// the hashlock from the event. This supports the UI flow where the taker
/// generates the preimage independently after discovering the maker's offer.
pub async fn run_maker(
    config: &SwapConfig,
    eth_client: &EthClient,
    lez_client: &LezClient,
    hashlock: Option<[u8; 32]>,
    progress: Option<ProgressSender>,
) -> Result<SwapOutcome> {
    // 1. Watch for ETH Locked event from the taker.
    progress::report(&progress, SwapProgress::WaitingForEthLock);
    let (tx, mut rx) = mpsc::channel::<EthHtlcEvent>(16);
    let watcher_eth_client = EthClient::new(config).await?;
    let watcher_handle = tokio::spawn(async move {
        let _ = watcher::watch_events(&watcher_eth_client, tx).await;
    });

    let (swap_id, discovered_hashlock) = loop {
        tokio::select! {
            Some(event) = rx.recv() => {
                if let EthHtlcEvent::Locked {
                    swap_id,
                    recipient,
                    amount,
                    hashlock: event_hashlock,
                    ..
                } = event
                {
                    let hashlock_matches = hashlock
                        .map_or(true, |hl| event_hashlock.0 == hl);
                    if hashlock_matches
                        && recipient == config.eth_recipient_address
                        && amount >= U256::from(config.eth_amount)
                    {
                        // Verify the HTLC is still OPEN on-chain (skip stale swaps).
                        if let Ok(htlc) = eth_client.get_htlc(swap_id).await {
                            if !matches!(htlc.state, SwapState::OPEN) {
                                continue;
                            }
                        }
                        info!(%swap_id, "maker: matched ETH Locked event");
                        progress::report(&progress, SwapProgress::EthLockDetected {
                            swap_id: format!("{swap_id}"),
                        });
                        break (swap_id, event_hashlock.0);
                    }
                }
            }
            _ = tokio::time::sleep(std::time::Duration::from_secs(
                config.eth_timelock.saturating_sub(now_unix())
            )) => {
                // ETH timelock expired — no taker showed up.
                watcher_handle.abort();
                info!("maker: ETH timelock expired, no taker locked");
                progress::report(&progress, SwapProgress::TimelockExpired);
                return Ok(SwapOutcome::Refunded {
                    eth_refund_tx: None,
                    lez_refund_tx: None,
                });
            }
        }
    };

    let hashlock = discovered_hashlock;

    watcher_handle.abort();

    // 2. Lock LEZ (short timelock).
    progress::report(&progress, SwapProgress::LezLocking);
    let lez_lock_tx = lez_client
        .lock(hashlock, config.lez_taker_account_id, config.lez_amount)
        .await?;
    info!(tx_hash = %lez_lock_tx, "maker: LEZ locked");
    progress::report(
        &progress,
        SwapProgress::LezLocked {
            tx_hash: lez_lock_tx.clone(),
        },
    );

    // 3. Watch LEZ escrow for taker's claim (reveals preimage).
    progress::report(&progress, SwapProgress::WaitingForPreimage);
    let (lez_tx, mut lez_rx) = mpsc::channel::<LezHtlcEvent>(16);
    let watcher_lez_client = LezClient::new(config)?;
    let poll_interval = config.poll_interval;
    let lez_watcher_handle = tokio::spawn(async move {
        let _ = lez_watcher::watch_escrow(
            &watcher_lez_client,
            hashlock,
            poll_interval,
            lez_tx,
        )
        .await;
    });

    let preimage = loop {
        tokio::select! {
            Some(event) = lez_rx.recv() => {
                match event {
                    LezHtlcEvent::Claimed { preimage, .. } => {
                        info!("maker: taker claimed LEZ, preimage revealed");
                        let preimage_arr: [u8; 32] = preimage.try_into().map_err(|_| {
                            crate::error::SwapError::InvalidState {
                                expected: "32-byte preimage".into(),
                                actual: "wrong length".into(),
                            }
                        })?;
                        progress::report(&progress, SwapProgress::PreimageRevealed {
                            preimage: hex::encode(preimage_arr),
                        });
                        break preimage_arr;
                    }
                    LezHtlcEvent::Refunded { .. } => {
                        // Shouldn't happen — only maker can refund LEZ.
                        // But handle gracefully.
                        lez_watcher_handle.abort();
                        return Ok(SwapOutcome::Refunded {
                            eth_refund_tx: None,
                            lez_refund_tx: Some(lez_lock_tx),
                        });
                    }
                    _ => {}
                }
            }
            _ = tokio::time::sleep(std::time::Duration::from_secs(
                config.lez_timelock.saturating_sub(now_unix())
            )) => {
                // LEZ timelock expired — taker didn't claim. Refund LEZ.
                lez_watcher_handle.abort();
                info!("maker: LEZ timelock expired, taker didn't claim");
                progress::report(&progress, SwapProgress::TimelockExpired);
                progress::report(&progress, SwapProgress::Refunding);
                let lez_refund_tx = lez_client.refund(&hashlock).await.ok();
                progress::report(&progress, SwapProgress::RefundComplete);
                return Ok(SwapOutcome::Refunded {
                    eth_refund_tx: None,
                    lez_refund_tx,
                });
            }
        }
    };

    lez_watcher_handle.abort();

    // 4. Claim ETH using the revealed preimage.
    progress::report(&progress, SwapProgress::ClaimingEth);
    let eth_claim_tx = eth_client.claim(swap_id, preimage).await?;
    info!(%eth_claim_tx, "maker: ETH claimed");
    progress::report(
        &progress,
        SwapProgress::EthClaimed {
            tx_hash: format!("{eth_claim_tx}"),
        },
    );

    Ok(SwapOutcome::Completed {
        preimage,
        eth_tx: eth_claim_tx,
        lez_tx: lez_lock_tx,
    })
}
