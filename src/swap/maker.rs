use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, Ordering};

use alloy::primitives::{FixedBytes, U256};
use tokio::sync::mpsc;
use tracing::{info, warn};

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
    /// Durably record an in-flight swap in the `PreLock` stage, BEFORE locking
    /// LEZ (P2-2). Must return `Err` (not silently drop) if the write cannot be
    /// made durable, so the caller can refuse to lock. If the lock then fails
    /// pre-commit, reconciliation retires this `PreLock` entry (nothing was
    /// locked) instead of wedging startup forever.
    fn record(&self, hashlock_hex: &str, swap_id: &str) -> Result<()>;
    /// Upgrade a recorded entry from `PreLock` to `Funded` after the LEZ lock is
    /// confirmed on-chain (P2-2). Best-effort: a lost upgrade self-heals via
    /// reconciliation (still-`PreLock` entry + existing escrow ⇒ re-upgrade).
    fn mark_funded(&self, hashlock_hex: &str);
    /// Clear a swap after a confirmed terminal state. Best-effort: a failed
    /// clear only costs a redundant (idempotent) reconcile on the next restart.
    fn clear(&self, hashlock_hex: &str);
    /// Whether a hashlock is already journaled as in-flight. The maker loop uses
    /// this to SKIP matching an ETH lock whose hashlock is already being handled
    /// by startup reconciliation / a resume watcher, so a replayed still-OPEN
    /// lock is never re-locked and double-funded (P1-A belt).
    fn contains(&self, hashlock_hex: &str) -> bool;
    /// Whether the journal currently holds ANY in-flight (non-quarantined) entry.
    /// The loop calls this after each `run_maker` return to decide — from the
    /// JOURNAL state, not the error variant — whether funds are unresolved and
    /// the loop must STOP for a supervised restart → reconciliation (P1-3).
    fn has_in_flight(&self) -> bool;
}

/// Extra guards/plumbing that are active only in `--loop` (liquidity-bot) mode.
pub struct LoopGuards<'a> {
    /// Minimum seconds by which the taker's on-chain ETH timelock must exceed
    /// the maker's fresh LEZ timelock before the maker locks LEZ. Guards against
    /// a hostile taker locking ETH at the contract minimum while the maker locks
    /// LEZ for far longer (the taker could then claim LEZ *and* refund ETH).
    pub timelock_margin_secs: u64,
    /// The maker's LEZ timelock *duration* in seconds (not an absolute expiry).
    /// A FRESH absolute LEZ expiry is computed from this at the moment an ETH
    /// lock matches — the per-iteration `config.lez_timelock` is fixed at
    /// iteration start and goes stale while the maker waits for a taker, so a
    /// late-arriving minimum-duration ETH lock would otherwise pass the margin
    /// check against an already-past LEZ expiry (P1-1).
    pub lez_timelock_secs: u64,
    /// Durable in-flight journal for crash recovery.
    pub journal: &'a dyn SwapJournal,
}

/// Compute the absolute LEZ expiry (unix seconds) to lock with, evaluated FRESH
/// at match time: `now` + the configured LEZ timelock duration. Using a value
/// computed at iteration start would go stale while the maker waits for an ETH
/// lock, letting a late minimum-duration lock pass the margin gate against a
/// near-past expiry and then be locked with that stale timelock (P1-1).
pub(crate) fn fresh_lez_expiry_secs(now_secs: u64, lez_timelock_secs: u64) -> u64 {
    now_secs.saturating_add(lez_timelock_secs)
}

/// Whether the maker loop must STOP after an iteration. The decider is the
/// JOURNAL state, not the `run_maker` outcome variant: `run_maker` durably
/// records an entry before locking LEZ and clears it only on a confirmed
/// terminal state, and startup guarantees the journal is empty before the loop
/// begins (reconcile drains all prior entries). So any entry present after an
/// iteration belongs to the swap that just ran and means funds are
/// in-flight/unresolved — STOP for a supervised restart → reconciliation rather
/// than spin a fresh watcher that could rematch the still-OPEN ETH lock (P1-3).
pub(crate) fn loop_must_stop_after_iteration(journal_has_in_flight: bool) -> bool {
    journal_has_in_flight
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

/// What to do with a matched ETH-lock candidate in the maker event loop.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CandidateDisposition {
    /// Already rejected this run, or already journaled as in-flight — ignore it
    /// and keep waiting for other locks (P1-A belt / P1-E rejected-set).
    Skip,
    /// The taker's ETH timelock is too tight; record it as rejected and keep
    /// waiting — NEVER surface an error (that restarts the outer loop and the
    /// 256-block replay re-delivers this same lock, a hot-loop DoS) (P1-E).
    Reject,
    /// Safe to proceed to lock LEZ against.
    Accept,
}

/// Pure decision for one matched candidate. `margin` is `None` for the
/// single-shot maker (no loop-mode timelock gate). Kept side-effect-free so the
/// P1-A journal-skip belt and the P1-E rejected-set / timelock gate are
/// unit-testable without a live watcher or sequencer.
///
/// The rejected-set is keyed by the ETH **swap id** (the escrow identifier),
/// NOT the hashlock: a taker who locks too-tight and then re-locks correctly
/// under a NEW swap id (same secret) must not be suppressed by a stale
/// hashlock-keyed rejection (P2-5). The journal-skip belt (`in_journal`) stays
/// hashlock-scoped — a hashlock already in flight must never be re-locked
/// regardless of swap id (P1-A).
pub(crate) fn classify_candidate(
    swap_id: &[u8; 32],
    rejected: &HashSet<[u8; 32]>,
    in_journal: bool,
    eth_timelock_secs: u64,
    lez_timelock_secs: u64,
    margin: Option<u64>,
) -> CandidateDisposition {
    if rejected.contains(swap_id) || in_journal {
        return CandidateDisposition::Skip;
    }
    if let Some(margin_secs) = margin
        && !eth_timelock_covers_lez(eth_timelock_secs, lez_timelock_secs, margin_secs)
    {
        return CandidateDisposition::Reject;
    }
    CandidateDisposition::Accept
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

/// Claim the taker's ETH after they revealed the preimage, retrying with
/// bounded exponential backoff.
///
/// At this point the taker already holds the maker's LEZ, so this is the leg
/// that realizes the maker's proceeds — a transient RPC/WS failure must not be
/// allowed to silently drop it. We retry hard; if the claim keeps failing we
/// surface [`SwapError::EthClaimUnresolved`] so the caller STOPS the loop and
/// lets the supervisor restart into startup reconciliation (which retries the
/// claim before the taker's ETH refund deadline). A revert because the claim
/// already landed (state `CLAIMED`) is treated as success (P1-B).
async fn claim_eth_after_reveal(
    eth_client: &EthClient,
    swap_id: FixedBytes<32>,
    preimage: [u8; 32],
) -> Result<FixedBytes<32>> {
    const MAX_ATTEMPTS: u32 = 6;
    let mut delay = std::time::Duration::from_secs(2);
    let mut last_err: Option<String> = None;

    for attempt in 1..=MAX_ATTEMPTS {
        match eth_client.claim(swap_id, preimage).await {
            Ok(tx) => return Ok(tx),
            Err(e) => {
                // The submit may have reverted because the claim already landed
                // — re-check on-chain before counting it a failure.
                if let Ok(htlc) = eth_client.get_htlc(swap_id).await
                    && matches!(htlc.state, SwapState::CLAIMED)
                {
                    info!(%swap_id, "maker: ETH already claimed on re-check");
                    return Ok(FixedBytes::ZERO);
                }
                warn!(%swap_id, "maker: ETH claim attempt {attempt}/{MAX_ATTEMPTS} failed: {e}");
                last_err = Some(e.to_string());
                if attempt < MAX_ATTEMPTS {
                    tokio::time::sleep(delay).await;
                    delay = (delay * 2).min(std::time::Duration::from_secs(30));
                }
            }
        }
    }

    Err(SwapError::EthClaimUnresolved(
        last_err.unwrap_or_else(|| "unknown error".into()),
    ))
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

    // Bounded set of ETH swap ids this run has already rejected (e.g. a lock
    // whose ETH timelock does not clear the maker's LEZ timelock by the margin).
    // The 256-block replay watcher re-delivers historical locks on every
    // restart; rejecting *within* this event loop and continuing to wait —
    // instead of returning Err and letting the outer loop restart over the same
    // lock — prevents a permanent hot-loop / DoS on one hostile candidate
    // (P1-E). Keyed by swap id (not hashlock) so a corrected same-secret re-lock
    // under a fresh escrow is still acceptable (P2-5).
    const MAX_REJECTED: usize = 1024;
    let mut rejected: HashSet<[u8; 32]> = HashSet::new();

    let (swap_id, discovered_hashlock, lez_expiry_secs) = loop {
        tokio::select! {
            Some(event) = rx.recv() => {
                if let EthHtlcEvent::Locked {
                    swap_id,
                    recipient,
                    amount,
                    hashlock: event_hashlock,
                    timelock: event_timelock,
                    tx_hash: event_tx_hash,
                    ..
                } = event
                {
                    let hl = event_hashlock.0;
                    let hashlock_matches = hashlock.is_none_or(|want| hl == want);
                    if hashlock_matches
                        && recipient == config.eth_recipient_address
                        && amount >= U256::from(config.eth_amount)
                    {
                        // The contract stores the timelock as an absolute unix
                        // second value; saturate a hostile huge value (that only
                        // gives the maker *more* headroom) into u64.
                        let eth_timelock_secs = event_timelock.saturating_to::<u64>();

                        // P1-1: compute the LEZ expiry we will ACTUALLY lock with
                        // FRESH, right now at match time — not the stale
                        // `config.lez_timelock` fixed at iteration start. The
                        // maker may have waited a long time for this lock; a stale
                        // (near-past) expiry would let a late minimum-duration ETH
                        // lock pass the margin gate and then be locked against an
                        // already-expired LEZ timelock. Single-shot maker (no
                        // guards) keeps the config's absolute value.
                        let lez_expiry_secs = guards
                            .map(|g| fresh_lez_expiry_secs(now_unix(), g.lez_timelock_secs))
                            .unwrap_or(config.lez_timelock);

                        // Combined decision (pure, unit-tested): already
                        // rejected/journaled ⇒ Skip; ETH timelock too tight vs
                        // the FRESH LEZ expiry + margin ⇒ Reject (record + keep
                        // waiting, never Err — that would restart the outer loop
                        // and replay this same lock forever, P1-E); otherwise
                        // Accept. The journal check is the P1-A belt against
                        // re-locking an in-flight hashlock. `None` guards ⇒
                        // single-shot maker (no gate). Rejection is keyed by swap
                        // id (P2-5).
                        let in_journal = guards
                            .map(|g| g.journal.contains(&hex::encode(hl)))
                            .unwrap_or(false);
                        match classify_candidate(
                            &swap_id.0,
                            &rejected,
                            in_journal,
                            eth_timelock_secs,
                            lez_expiry_secs,
                            guards.map(|g| g.timelock_margin_secs),
                        ) {
                            CandidateDisposition::Skip => {
                                info!(%swap_id, "maker: skipping ETH lock — already in-flight/handled");
                                continue;
                            }
                            CandidateDisposition::Reject => {
                                warn!(
                                    %swap_id,
                                    eth_timelock_secs,
                                    lez_expiry_secs,
                                    "maker: rejecting ETH lock — timelock too tight vs fresh LEZ \
                                     expiry + margin; waiting for other locks"
                                );
                                // Surface the rejection to progress consumers
                                // (CLI / GUI): without this the maker loop
                                // silently stays at WaitingForEthLock and the
                                // operator has no signal that a candidate was
                                // seen and turned away, or why. Additive only —
                                // the P1-E record-and-keep-waiting semantics
                                // are unchanged.
                                progress::report(&progress, SwapProgress::EthLockRejected {
                                    swap_id: format!("{swap_id}"),
                                    eth_expiry_secs: eth_timelock_secs,
                                    required_expiry_secs: lez_expiry_secs.saturating_add(
                                        guards.map(|g| g.timelock_margin_secs).unwrap_or(0),
                                    ),
                                });
                                if rejected.len() < MAX_REJECTED {
                                    rejected.insert(swap_id.0);
                                }
                                continue;
                            }
                            CandidateDisposition::Accept => {}
                        }

                        // Verify the HTLC is still OPEN on-chain (skip stale swaps).
                        if let Ok(htlc) = eth_client.get_htlc(swap_id).await
                            && !matches!(htlc.state, SwapState::OPEN)
                        {
                            continue;
                        }

                        info!(%swap_id, tx_hash = %event_tx_hash, "maker: matched ETH Locked event");
                        progress::report(&progress, SwapProgress::EthLockDetected {
                            swap_id: format!("{swap_id}"),
                            hashlock: hex::encode(hl),
                            tx_hash: format!("{event_tx_hash}"),
                            chain_id: eth_client.chain_id(),
                        });
                        break (swap_id, hl, lez_expiry_secs);
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

    // The taker's ETH timelock has already been checked against the maker's
    // FRESH LEZ expiry + margin inside the event loop above (a too-tight lock is
    // rejected-and-skipped there, never reaching this point) — so all that
    // remains before moving any LEZ is to durably journal the swap.
    // `lez_expiry_secs` is the fresh absolute expiry computed at match time; it
    // is the value we validated the margin against AND the value we lock with,
    // so the two can never disagree (P1-1).
    if let Some(guards) = guards {
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
            lez_expiry_secs,
        )
        .await?;
    info!(tx_hash = %lez_lock_tx, "maker: LEZ locked");
    // P2-2: the lock is confirmed and an on-chain escrow now exists — upgrade the
    // journal entry from PreLock to Funded so a later crash-recovery pass treats
    // a phantom/short escrow read as ambiguous (retain) rather than as a
    // never-locked entry to retire.
    if let Some(guards) = guards {
        guards.journal.mark_funded(&hashlock_hex);
    }
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
                lez_expiry_secs.saturating_sub(now_unix())
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
                        // Retry in-process with bounded backoff; on persistent
                        // failure this returns EthClaimUnresolved (Err propagates
                        // → journal retained) so the loop STOPS for a supervised
                        // restart rather than racing past an unresolved fund-
                        // bearing entry (P1-B).
                        let eth_claim_tx = claim_eth_after_reveal(eth_client, swap_id, preimage).await?;
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

    // 4. Claim ETH using the revealed preimage. Retry in-process with bounded
    // backoff (P1-B): the taker already holds the LEZ, so a transient RPC blip
    // must not drop the maker's ETH leg. On persistent failure the error
    // propagates WITHOUT clearing the journal and the caller STOPS the loop for
    // a supervised restart, where reconcile retries the claim (P1-5 / P1-B).
    progress::report(&progress, SwapProgress::ClaimingEth);
    let eth_claim_tx = claim_eth_after_reveal(eth_client, swap_id, preimage).await?;
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
        lez_timelock_secs: auto_config.lez_timelock_minutes * 60,
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
                // Whether the loop may continue is decided BELOW by the journal
                // state, not this error variant (P1-3).
            }
        }

        // P1-3 (principle ii): the JOURNAL — not the outcome variant — decides
        // whether the loop may continue. run_maker durably records an entry
        // before locking LEZ and clears it only on a confirmed terminal state;
        // startup guarantees the journal is empty before the loop begins, so any
        // entry present here belongs to the swap that just ran and means funds
        // are in-flight/unresolved. STOP for a supervised restart →
        // reconciliation (which retries the claim/refund and, for an unfundable
        // partial lock, quarantines it) rather than spin a fresh watcher that
        // could rematch the still-OPEN ETH lock. This subsumes the old
        // EthClaimUnresolved special-case AND catches EscrowStateUnknown / LEZ
        // funding-confirmation failures that previously fell through to another
        // iteration with a retained entry.
        if loop_must_stop_after_iteration(journal.has_in_flight()) {
            progress::report(
                &progress,
                SwapProgress::AutoAcceptSwapFailed {
                    iteration,
                    error: "in-flight journal entry unresolved after swap — stopping loop for \
                            supervised restart"
                        .into(),
                },
            );
            break;
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
        assert!(eth_timelock_covers_lez(
            lez_expiry + 1200,
            lez_expiry,
            margin
        ));
        // Exactly on the margin boundary is acceptable.
        assert!(eth_timelock_covers_lez(
            lez_expiry + margin,
            lez_expiry,
            margin
        ));

        // Hostile taker: locked ETH at the contract 5-min minimum while the
        // maker would lock LEZ for 20 min — ETH expires BEFORE the LEZ window
        // even opens for the maker's claim. Must be rejected.
        assert!(!eth_timelock_covers_lez(
            lez_expiry - 900,
            lez_expiry,
            margin
        ));
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

    // P1-E: a too-tight ETH timelock yields Reject (record + keep waiting),
    // NOT an error that restarts the loop over the replayed lock. Once
    // rejected, the same hashlock yields Skip on every subsequent replay — so
    // the candidate can never re-trigger processing (no hot-loop / DoS).
    #[test]
    fn tight_timelock_is_rejected_then_skipped_not_restarted() {
        let hl = [0x11u8; 32];
        let lez = 1_000_000u64;
        let margin = 300u64;
        let mut rejected: HashSet<[u8; 32]> = HashSet::new();

        // First encounter: too tight → Reject (caller records it, keeps waiting).
        assert_eq!(
            classify_candidate(&hl, &rejected, false, lez, lez, Some(margin)),
            CandidateDisposition::Reject
        );
        rejected.insert(hl);
        // Every subsequent replay of the same lock → Skip (never re-processed).
        assert_eq!(
            classify_candidate(&hl, &rejected, false, lez, lez, Some(margin)),
            CandidateDisposition::Skip
        );
    }

    // P1-A belt: a hashlock already in the crash-recovery journal (being handled
    // by reconcile / a resume watcher) is Skipped even if its timelock is fine —
    // re-locking it would create a second escrow fund and strand LEZ.
    #[test]
    fn journaled_hashlock_is_skipped_even_when_timelock_ok() {
        let hl = [0x22u8; 32];
        let lez = 1_000u64;
        let rejected: HashSet<[u8; 32]> = HashSet::new();
        // Timelock is comfortably safe, but it is already in the journal.
        assert_eq!(
            classify_candidate(&hl, &rejected, true, lez + 10_000, lez, Some(300)),
            CandidateDisposition::Skip
        );
        // Not in the journal and timelock safe → Accept.
        assert_eq!(
            classify_candidate(&hl, &rejected, false, lez + 10_000, lez, Some(300)),
            CandidateDisposition::Accept
        );
    }

    // The single-shot maker (no guards ⇒ margin None) applies no loop-mode gate:
    // any matched, non-journaled, non-rejected candidate is Accepted.
    #[test]
    fn single_shot_maker_has_no_timelock_gate() {
        let hl = [0x33u8; 32];
        let rejected: HashSet<[u8; 32]> = HashSet::new();
        // Even a "tight" timelock is Accepted when margin is None.
        assert_eq!(
            classify_candidate(&hl, &rejected, false, 1, 1_000_000, None),
            CandidateDisposition::Accept
        );
    }

    // P1-1: the LEZ expiry the margin is checked against must be computed FRESH
    // at match time. A `config.lez_timelock` fixed at iteration start goes stale
    // while the maker waits for a taker; a late minimum-duration ETH lock then
    // clears the margin against the near-past stale expiry but NOT against the
    // fresh expiry it would actually be locked with.
    #[test]
    fn stale_expiry_rejected_at_match_time() {
        let margin = 300u64; // 5 min
        let lez_duration = 1_200u64; // 20 min
        let iteration_start = 1_000_000u64;
        // Maker waited 25 min: the iteration-start expiry is now 5 min in the past.
        let now = iteration_start + 1_500;
        let stale_lez_expiry = iteration_start + lez_duration;
        // Taker locks ETH at the contract 5-min minimum.
        let eth_lock = now + 300;

        // Against the STALE expiry the tight lock wrongly passes the margin gate.
        assert!(eth_timelock_covers_lez(eth_lock, stale_lez_expiry, margin));

        // Against the FRESH expiry computed at match time it is correctly
        // rejected — and that same fresh value is what the LEZ lock uses, so the
        // gate and the lock can never disagree.
        let fresh = fresh_lez_expiry_secs(now, lez_duration);
        assert!(!eth_timelock_covers_lez(eth_lock, fresh, margin));
    }

    // P2-5: the rejected-set is keyed by ETH swap id, not hashlock. A taker who
    // locks too-tight (rejected) and then re-locks CORRECTLY under a fresh escrow
    // id with the SAME secret must NOT be suppressed by the stale rejection.
    #[test]
    fn rejected_set_is_keyed_by_swap_id_not_hashlock() {
        let swap_a = [0xA1u8; 32]; // first (too-tight) escrow id
        let swap_b = [0xB2u8; 32]; // corrected re-lock, same secret, NEW escrow id
        let lez = 1_000_000u64;
        let margin = 300u64;
        let mut rejected: HashSet<[u8; 32]> = HashSet::new();

        // First escrow too tight → Reject; the caller records the swap id.
        assert_eq!(
            classify_candidate(&swap_a, &rejected, false, lez, lez, Some(margin)),
            CandidateDisposition::Reject
        );
        rejected.insert(swap_a);
        // Replay of the SAME escrow id → Skip.
        assert_eq!(
            classify_candidate(&swap_a, &rejected, false, lez + 10_000, lez, Some(margin)),
            CandidateDisposition::Skip
        );
        // A corrected re-lock under a NEW escrow id (same secret) with a safe
        // timelock is a fresh, acceptable candidate — not suppressed.
        assert_eq!(
            classify_candidate(&swap_b, &rejected, false, lez + 10_000, lez, Some(margin)),
            CandidateDisposition::Accept
        );
    }

    // A minimal in-memory journal to exercise the P1-3 stop decision without a
    // live watcher/sequencer: `record`/`clear` mutate the set; the loop consults
    // `has_in_flight` after each iteration.
    #[derive(Default)]
    struct MockJournal {
        entries: std::sync::Mutex<HashSet<String>>,
    }
    impl SwapJournal for MockJournal {
        fn record(&self, hashlock_hex: &str, _swap_id: &str) -> Result<()> {
            self.entries
                .lock()
                .unwrap()
                .insert(hashlock_hex.to_string());
            Ok(())
        }
        fn mark_funded(&self, _hashlock_hex: &str) {}
        fn clear(&self, hashlock_hex: &str) {
            self.entries.lock().unwrap().remove(hashlock_hex);
        }
        fn contains(&self, hashlock_hex: &str) -> bool {
            self.entries.lock().unwrap().contains(hashlock_hex)
        }
        fn has_in_flight(&self) -> bool {
            !self.entries.lock().unwrap().is_empty()
        }
    }

    // P1-3 (principle ii): the loop stops iff the journal still holds an
    // in-flight entry after run_maker returns — regardless of the error variant.
    #[test]
    fn loop_stops_when_journal_nonempty_on_any_error() {
        let journal = MockJournal::default();

        // Clean journal (e.g. a pre-lock transient failure that stranded nothing)
        // ⇒ the loop may continue.
        assert!(!loop_must_stop_after_iteration(journal.has_in_flight()));

        // run_maker recorded an entry before locking LEZ and returned an error
        // WITHOUT clearing it (EthClaimUnresolved / EscrowStateUnknown / a LEZ
        // funding-confirmation failure — the variant is irrelevant). The entry is
        // retained ⇒ the loop MUST stop for a supervised restart.
        journal.record(&"ab".repeat(32), "0xdead").unwrap();
        assert!(loop_must_stop_after_iteration(journal.has_in_flight()));

        // A confirmed terminal state clears the entry ⇒ the loop may continue.
        journal.clear(&"ab".repeat(32));
        assert!(!loop_must_stop_after_iteration(journal.has_in_flight()));
    }
}
