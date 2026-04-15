use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use alloy::primitives::U256;
use tokio::sync::mpsc;
use tracing::info;

use crate::{
    config::{account_id_to_base58, SwapConfig},
    error::{Result, SwapError},
    eth::client::{EthClient, EthHTLC::SwapState},
    eth::watcher::{self, EthHtlcEvent},
    lez::client::LezClient,
    lez::watcher as lez_watcher,
    lez::watcher::LezHtlcEvent,
    messaging::sender::MessagingSender,
    messaging::types::{SwapOffer, OFFERS_TOPIC},
    swap::{
        progress::{self, ProgressSender, SwapProgress},
        refund::now_unix,
        types::SwapOutcome,
    },
};

/// Wait until the cancel flag is set. Returns immediately if the flag is already set.
/// If `cancel` is `None`, pends forever (no cancellation configured).
async fn cancel_wait(cancel: &Option<&AtomicBool>) {
    match cancel {
        Some(flag) => loop {
            if flag.load(Ordering::Relaxed) {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        },
        None => std::future::pending().await,
    }
}

/// Run the maker side of an atomic swap (taker-locks-first).
///
/// The maker optionally receives a hashlock. If `None`, the maker watches for
/// any ETH lock to its recipient address with sufficient amount and extracts
/// the hashlock from the event. This supports the UI flow where the taker
/// generates the preimage independently after discovering the maker's offer.
///
/// If `cancel` is `Some`, the flag is checked during the ETH lock wait phase.
/// Setting the flag causes the function to return `Err(SwapError::Cancelled)`.
pub async fn run_maker(
    config: &SwapConfig,
    eth_client: &EthClient,
    lez_client: &LezClient,
    hashlock: Option<[u8; 32]>,
    cancel: Option<&AtomicBool>,
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
            _ = cancel_wait(&cancel) => {
                watcher_handle.abort();
                return Err(SwapError::Cancelled);
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

/// Configuration for the auto-accept maker loop.
pub struct AutoAcceptConfig {
    pub lez_timelock_minutes: u64,
    pub eth_timelock_minutes: u64,
}

/// Result of a completed auto-accept loop run.
pub struct AutoAcceptResult {
    pub total_completed: u32,
    pub total_failed: u32,
}

/// Run the maker in a loop, auto-accepting swaps until cancelled or out of funds.
///
/// Each iteration gets fresh timelocks, checks balance, publishes an offer
/// (if messaging is configured), and runs a single maker swap. On failure,
/// the error is logged and the loop continues (R1 resilience).
pub async fn run_maker_loop(
    base_config: &SwapConfig,
    auto_config: &AutoAcceptConfig,
    cancel: &AtomicBool,
    progress: Option<ProgressSender>,
    messaging: Option<&MessagingSender>,
) -> AutoAcceptResult {
    let mut completed: u32 = 0;
    let mut failed: u32 = 0;
    let mut iteration: u32 = 0;

    progress::report(&progress, SwapProgress::AutoAcceptStarted);

    loop {
        // Check cancel flag between iterations.
        if cancel.load(Ordering::Relaxed) {
            progress::report(&progress, SwapProgress::AutoAcceptCancelled);
            break;
        }

        iteration += 1;

        // Fresh timelocks for this iteration.
        let fresh_config = base_config.with_fresh_timelocks(
            auto_config.lez_timelock_minutes,
            auto_config.eth_timelock_minutes,
        );

        // Check LEZ balance before proceeding.
        let lez_client = match LezClient::new(&fresh_config) {
            Ok(c) => c,
            Err(e) => {
                failed += 1;
                progress::report(
                    &progress,
                    SwapProgress::AutoAcceptSwapFailed {
                        iteration,
                        error: format!("LEZ client init failed: {e}"),
                    },
                );
                tokio::time::sleep(Duration::from_secs(5)).await;
                continue;
            }
        };

        match lez_client.get_balance(&lez_client.account_id()).await {
            Ok(balance) if balance < fresh_config.lez_amount => {
                progress::report(
                    &progress,
                    SwapProgress::AutoAcceptInsufficientFunds {
                        lez_balance: balance.to_string(),
                        lez_required: fresh_config.lez_amount.to_string(),
                    },
                );
                break;
            }
            Err(e) => {
                failed += 1;
                progress::report(
                    &progress,
                    SwapProgress::AutoAcceptSwapFailed {
                        iteration,
                        error: format!("balance check failed: {e}"),
                    },
                );
                tokio::time::sleep(Duration::from_secs(5)).await;
                continue;
            }
            _ => {} // balance sufficient
        }

        // Publish offer via delivery module (if messaging callback provided).
        if let Some(sender) = &messaging {
            let offer = SwapOffer {
                hashlock: String::new(),
                lez_amount: fresh_config.lez_amount,
                eth_amount: fresh_config.eth_amount,
                maker_eth_address: format!("{}", fresh_config.eth_recipient_address),
                maker_lez_account: account_id_to_base58(&lez_client.account_id()),
                lez_timelock: fresh_config.lez_timelock,
                eth_timelock: fresh_config.eth_timelock,
                lez_htlc_program_id: hex::encode(
                    fresh_config
                        .lez_htlc_program_id
                        .iter()
                        .flat_map(|w| w.to_le_bytes())
                        .collect::<Vec<u8>>(),
                ),
                eth_htlc_address: format!("{}", fresh_config.eth_htlc_address),
            };
            sender.publish(OFFERS_TOPIC, &offer);
            info!(iteration, "maker: offer published via delivery module");
        }

        progress::report(
            &progress,
            SwapProgress::AutoAcceptIteration { iteration },
        );

        // Create ETH client for this iteration.
        let eth_client = match EthClient::new(&fresh_config).await {
            Ok(c) => c,
            Err(e) => {
                failed += 1;
                progress::report(
                    &progress,
                    SwapProgress::AutoAcceptSwapFailed {
                        iteration,
                        error: format!("ETH client init failed: {e}"),
                    },
                );
                tokio::time::sleep(Duration::from_secs(5)).await;
                continue;
            }
        };

        // Run a single maker swap with cancel support.
        match run_maker(
            &fresh_config,
            &eth_client,
            &lez_client,
            None,
            Some(cancel),
            progress.clone(),
        )
        .await
        {
            Ok(SwapOutcome::Completed { .. }) => {
                completed += 1;
                progress::report(
                    &progress,
                    SwapProgress::AutoAcceptSwapCompleted {
                        iteration,
                        status: "completed".into(),
                    },
                );
            }
            Ok(SwapOutcome::Refunded { .. }) => {
                failed += 1;
                progress::report(
                    &progress,
                    SwapProgress::AutoAcceptSwapFailed {
                        iteration,
                        error: "swap refunded (taker timed out)".into(),
                    },
                );
            }
            Err(SwapError::Cancelled) => {
                progress::report(&progress, SwapProgress::AutoAcceptCancelled);
                break;
            }
            Err(e) => {
                failed += 1;
                progress::report(
                    &progress,
                    SwapProgress::AutoAcceptSwapFailed {
                        iteration,
                        error: e.to_string(),
                    },
                );
                // R1: log error and continue to next iteration
            }
        }
    }

    progress::report(
        &progress,
        SwapProgress::AutoAcceptStopped {
            total_completed: completed,
            total_failed: failed,
        },
    );

    AutoAcceptResult {
        total_completed: completed,
        total_failed: failed,
    }
}
