use std::sync::atomic::{AtomicBool, Ordering};

use alloy::primitives::U256;
use tokio::sync::mpsc;
use tracing::info;

use crate::{
    config::SwapConfig,
    error::{Result, SwapError},
    eth::client::{EthClient, EthHTLC::SwapState},
    eth::watcher::{self, EthHtlcEvent},
    lez::client::{LezClient, RefundOutcome},
    lez::watcher as lez_watcher,
    lez::watcher::LezHtlcEvent,
    swap::{
        progress::{self, ProgressSender, SwapProgress},
        refund::now_unix,
        types::SwapOutcome,
    },
};

/// Durable crash-recovery journal for the standing liquidity bot.
///
/// Implemented by `cli::bot::StateStore`. The swap flow records an in-flight
/// swap *durably* (fsync'd) **before** locking LEZ, and clears it only once the
/// swap reaches a confirmed terminal state (ETH claimed, or LEZ refund
/// confirmed on-chain). Defined here so the swap layer can drive the journal
/// without depending on the CLI layer.
pub trait SwapJournal: Send + Sync {
    /// Durably record an in-flight swap. Must return `Err` (not silently drop)
    /// if the write cannot be made durable, so the caller can refuse to lock.
    fn record(&self, hashlock_hex: &str, swap_id: &str) -> Result<()>;
    /// Clear a swap after a confirmed terminal state. Best-effort: a failed
    /// clear only costs a redundant (idempotent) reconcile on the next restart.
    fn clear(&self, hashlock_hex: &str);
}

/// Extra guards/plumbing that are active only in `--loop` (liquidity-bot) mode.
pub struct LoopGuards<'a> {
    /// Minimum seconds by which the taker's on-chain ETH timelock must exceed
    /// the maker's fresh LEZ timelock before the maker locks LEZ. Guards against
    /// a hostile taker locking ETH at the contract minimum while the maker locks
    /// LEZ for far longer (the taker could then claim LEZ *and* refund ETH).
    pub timelock_margin_secs: u64,
    /// Durable in-flight journal for crash recovery.
    pub journal: &'a dyn SwapJournal,
}

/// The maker-safety timelock invariant, evaluated against the *actual* on-chain
/// values: the taker's ETH escrow must expire at least `margin_secs` after the
/// maker's LEZ escrow, so the maker always has time to observe the preimage and
/// claim ETH after the LEZ claim window closes.
pub fn eth_timelock_covers_lez(
    eth_expiry_secs: u64,
    lez_expiry_secs: u64,
    margin_secs: u64,
) -> bool {
    eth_expiry_secs >= lez_expiry_secs.saturating_add(margin_secs)
}

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
    guards: Option<&LoopGuards<'_>>,
) -> Result<SwapOutcome> {
    // 1. Watch for ETH Locked event from the taker.
    progress::report(&progress, SwapProgress::WaitingForEthLock);
    let (tx, mut rx) = mpsc::channel::<EthHtlcEvent>(16);
    let watcher_eth_client = EthClient::new(config).await?;
    let watcher_handle = tokio::spawn(async move {
        let _ = watcher::watch_events(&watcher_eth_client, tx).await;
    });

    let (swap_id, discovered_hashlock, eth_timelock_secs) = loop {
        tokio::select! {
            Some(event) = rx.recv() => {
                if let EthHtlcEvent::Locked {
                    swap_id,
                    recipient,
                    amount,
                    hashlock: event_hashlock,
                    timelock: event_timelock,
                    ..
                } = event
                {
                    let hashlock_matches = hashlock.is_none_or(|hl| event_hashlock.0 == hl);
                    if hashlock_matches
                        && recipient == config.eth_recipient_address
                        && amount >= U256::from(config.eth_amount)
                    {
                        // Verify the HTLC is still OPEN on-chain (skip stale swaps).
                        if let Ok(htlc) = eth_client.get_htlc(swap_id).await
                            && !matches!(htlc.state, SwapState::OPEN)
                        {
                            continue;
                        }
                        info!(%swap_id, "maker: matched ETH Locked event");
                        progress::report(&progress, SwapProgress::EthLockDetected {
                            swap_id: format!("{swap_id}"),
                            hashlock: hex::encode(event_hashlock.0),
                        });
                        // The contract stores the timelock as an absolute unix
                        // second value; saturate a hostile huge value (that only
                        // gives the maker *more* headroom) into u64.
                        break (swap_id, event_hashlock.0, event_timelock.saturating_to::<u64>());
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
    let hashlock_hex = hex::encode(hashlock);

    watcher_handle.abort();

    // Loop-mode safety gates, evaluated against the *matched* on-chain lock —
    // not just the maker's locally-advertised durations — before any LEZ moves.
    if let Some(guards) = guards {
        // P1-1: the taker's actual ETH timelock must clear the maker's fresh LEZ
        // timelock by the configured margin. A hostile taker that locked ETH at
        // the contract minimum (while the maker would lock LEZ for far longer)
        // is rejected here, before the maker commits any LEZ.
        if !eth_timelock_covers_lez(
            eth_timelock_secs,
            config.lez_timelock,
            guards.timelock_margin_secs,
        ) {
            return Err(SwapError::InvalidState {
                expected: format!(
                    "ETH timelock >= LEZ timelock ({}) + margin ({}s) = {}s",
                    config.lez_timelock,
                    guards.timelock_margin_secs,
                    config.lez_timelock.saturating_add(guards.timelock_margin_secs)
                ),
                actual: format!("matched ETH timelock {eth_timelock_secs}s"),
            });
        }
        // P1-4: durably journal this swap BEFORE locking LEZ. If the write is not
        // durable we refuse to lock — a crash after locking with no journal entry
        // would strand the LEZ (the escrow PDA cannot be enumerated by owner).
        guards
            .journal
            .record(&hashlock_hex, &format!("{swap_id}"))?;
    }

    // 2. Lock LEZ (short timelock).
    progress::report(&progress, SwapProgress::LezLocking);
    let lez_lock_tx = lez_client
        .lock(
            hashlock,
            config.lez_taker_account_id,
            config.lez_amount,
            config.lez_timelock,
        )
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
        let _ =
            lez_watcher::watch_escrow(&watcher_lez_client, hashlock, poll_interval, lez_tx).await;
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
                        // But handle gracefully: the escrow is terminal.
                        lez_watcher_handle.abort();
                        if let Some(guards) = guards {
                            guards.journal.clear(&hashlock_hex);
                        }
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
                // LEZ timelock expired — taker didn't claim. Refund LEZ, but wait
                // for a CONFIRMED terminal state before clearing the journal: a
                // last-moment taker claim can win the race, in which case the
                // maker must claim ETH instead (P1-5 / P1-7).
                lez_watcher_handle.abort();
                info!("maker: LEZ timelock expired, taker didn't claim");
                progress::report(&progress, SwapProgress::TimelockExpired);
                progress::report(&progress, SwapProgress::Refunding);
                match lez_client.refund_confirmed(&hashlock).await {
                    Ok(RefundOutcome::Refunded(tx)) => {
                        progress::report(&progress, SwapProgress::RefundComplete);
                        if let Some(guards) = guards {
                            guards.journal.clear(&hashlock_hex);
                        }
                        return Ok(SwapOutcome::Refunded {
                            eth_refund_tx: None,
                            lez_refund_tx: (!tx.is_empty()).then_some(tx),
                        });
                    }
                    Ok(RefundOutcome::ClaimedByTaker(preimage)) => {
                        info!("maker: taker claimed LEZ during refund race, claiming ETH");
                        progress::report(&progress, SwapProgress::PreimageRevealed {
                            preimage: hex::encode(preimage),
                        });
                        progress::report(&progress, SwapProgress::ClaimingEth);
                        // Err propagates → journal retained for reconcile retry.
                        let eth_claim_tx = eth_client.claim(swap_id, preimage).await?;
                        progress::report(&progress, SwapProgress::EthClaimed {
                            tx_hash: format!("{eth_claim_tx}"),
                        });
                        if let Some(guards) = guards {
                            guards.journal.clear(&hashlock_hex);
                        }
                        return Ok(SwapOutcome::Completed {
                            preimage,
                            eth_tx: eth_claim_tx,
                            lez_tx: lez_lock_tx,
                        });
                    }
                    Err(e) => {
                        // Refund not confirmed — keep the journal entry so the
                        // next reconcile retries; surface the error.
                        return Err(e);
                    }
                }
            }
        }
    };

    lez_watcher_handle.abort();

    // 4. Claim ETH using the revealed preimage. On failure the error propagates
    // WITHOUT clearing the journal: the taker already has the LEZ, so reconcile
    // must retry this ETH claim on the next restart (P1-5).
    progress::report(&progress, SwapProgress::ClaimingEth);
    let eth_claim_tx = eth_client.claim(swap_id, preimage).await?;
    info!(%eth_claim_tx, "maker: ETH claimed");
    progress::report(
        &progress,
        SwapProgress::EthClaimed {
            tx_hash: format!("{eth_claim_tx}"),
        },
    );

    if let Some(guards) = guards {
        guards.journal.clear(&hashlock_hex);
    }

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
/// Each iteration gets fresh timelocks, checks balance, and runs a single maker swap. On failure,
/// the error is logged and the loop continues (R1 resilience).
pub async fn run_maker_loop(
    base_config: &SwapConfig,
    auto_config: &AutoAcceptConfig,
    cancel: &AtomicBool,
    progress: Option<ProgressSender>,
    journal: &dyn SwapJournal,
    timelock_margin_secs: u64,
) -> AutoAcceptResult {
    let mut completed: u32 = 0;
    let mut failed: u32 = 0;
    let mut iteration: u32 = 0;

    let guards = LoopGuards {
        timelock_margin_secs,
        journal,
    };

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
                tokio::time::sleep(base_config.poll_interval).await;
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
                tokio::time::sleep(base_config.poll_interval).await;
                continue;
            }
            _ => {} // balance sufficient
        }

        progress::report(&progress, SwapProgress::AutoAcceptIteration { iteration });

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
                tokio::time::sleep(base_config.poll_interval).await;
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
            Some(&guards),
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

#[cfg(test)]
mod tests {
    use super::*;

    // P1-1: the loop must reject a matched ETH lock whose on-chain timelock does
    // not clear the maker's fresh LEZ timelock by the configured margin — even
    // though the maker's own advertised durations are internally consistent.
    #[test]
    fn timelock_gate_uses_actual_eth_expiry_not_advertised() {
        let lez_expiry = 1_000_000u64;
        let margin = 300u64; // 5 min

        // Honest taker: ETH expiry well beyond LEZ + margin.
        assert!(eth_timelock_covers_lez(lez_expiry + 1200, lez_expiry, margin));
        // Exactly on the margin boundary is acceptable.
        assert!(eth_timelock_covers_lez(lez_expiry + margin, lez_expiry, margin));

        // Hostile taker: locked ETH at the contract 5-min minimum while the
        // maker would lock LEZ for 20 min — ETH expires BEFORE the LEZ window
        // even opens for the maker's claim. Must be rejected.
        assert!(!eth_timelock_covers_lez(lez_expiry - 900, lez_expiry, margin));
        // Just short of the margin is rejected.
        assert!(!eth_timelock_covers_lez(
            lez_expiry + margin - 1,
            lez_expiry,
            margin
        ));
    }

    #[test]
    fn timelock_gate_saturates_and_does_not_panic_on_huge_values() {
        // A hostile huge ETH timelock only gives the maker more headroom.
        assert!(eth_timelock_covers_lez(u64::MAX, 1_000, 300));
        // Saturating add on the LEZ side must not overflow-panic.
        assert!(!eth_timelock_covers_lez(10, u64::MAX, 300));
    }
}
