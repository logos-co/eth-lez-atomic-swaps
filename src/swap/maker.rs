use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use alloy::primitives::{FixedBytes, U256};
use lee_core::account::AccountId;
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
    ops::OpsRecorder,
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
    /// Durable, privacy-safe operations ledger (issue #98) — records
    /// `accepted`/`completed`/`refunded`/`failed` counts for public-trial
    /// reporting. Written strictly AFTER `journal` on every path, so an
    /// ops-ledger failure can never gate or corrupt a swap decision (see
    /// `crate::ops::OpsLedger`'s doc).
    pub ops: &'a dyn OpsRecorder,
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

/// Why a matched candidate was turned away. Every variant is decided in the
/// PURE path below, i.e. strictly before the journal write and before any LEZ
/// instruction is built.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RejectReason {
    /// The taker's ETH timelock does not clear the maker's fresh LEZ expiry by
    /// the safety margin.
    TimelockTooTight,
    /// The `takerLezAccount` published in the ETH lock is one the maker can
    /// never lock to: all-zero, or the maker's OWN account. The LEZ guest
    /// asserts `maker != taker` and rejects a zero/unknown taker, so building a
    /// Lock instruction for such a value is guaranteed to fail — see
    /// [`taker_lez_account_is_usable`] for why that must never happen after the
    /// journal write.
    UnusableTakerLezAccount,
    /// A designated-counterparty maker (`lez_taker_account_id` configured) saw a
    /// lock naming a different taker. Not hostile — just not ours.
    NotDesignatedTaker,
}

/// What to do with a matched ETH-lock candidate in the maker event loop.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CandidateDisposition {
    /// Already rejected this run, or already journaled as in-flight — ignore it
    /// and keep waiting for other locks (P1-A belt / P1-E rejected-set).
    Skip,
    /// Turn this candidate away: record it as rejected and keep waiting — NEVER
    /// surface an error (that restarts the outer loop and the 256-block replay
    /// re-delivers this same lock, a hot-loop DoS) (P1-E).
    Reject(RejectReason),
    /// Safe to proceed to lock LEZ against.
    Accept,
}

/// Whether a `takerLezAccount` read off the `Locked` event is one the maker can
/// actually lock to.
///
/// **This is the check that must run before the journal write.** The LEZ HTLC
/// guest asserts `maker_id != taker_id`, so a hostile taker who locks one cheap
/// ETH HTLC naming the MAKER'S OWN LEZ account makes the maker's Lock
/// instruction fail. If that were discovered after `journal.record`, the maker
/// would return `Err` with a non-empty journal — and a non-empty journal is
/// exactly what `loop_must_stop_after_iteration` treats as "funds unresolved,
/// stop for a supervised restart". One Sepolia transaction would then kill the
/// bot on every restart: a free, permanent DoS. Deciding it here, in the pure
/// path, downgrades that attack to a logged rejection that costs the attacker
/// gas and costs the maker nothing.
///
/// A `bytes32` is always exactly 32 bytes, so length can never be wrong on this
/// path; the contract additionally rejects `bytes32(0)` at lock time. Both are
/// re-checked anyway — this function is the maker's own belt, and it must not
/// depend on the contract it is talking to being the one we think it is.
pub fn taker_lez_account_is_usable(
    taker_lez_account: &[u8; 32],
    maker_lez_account: &[u8; 32],
) -> bool {
    taker_lez_account != &[0u8; 32] && taker_lez_account != maker_lez_account
}

/// The LEZ account the maker will name as the SOLE claimant of its escrow,
/// derived from the taker's own on-chain lock. `None` for a value the maker can
/// never lock to.
///
/// This is the seam between "what the taker published on Ethereum" and "what
/// goes into the LEZ `Lock` instruction", exposed so an integration test can
/// assert the whole chain (ETH lock → watcher decode → LEZ taker_id) without a
/// sequencer. It is evaluated at MATCH time, inside the pure path, so its
/// failure case can only ever produce a rejection — never a post-journal error.
pub fn designated_lez_claimant(
    taker_lez_account: &[u8; 32],
    maker_lez_account: &[u8; 32],
) -> Option<AccountId> {
    taker_lez_account_is_usable(taker_lez_account, maker_lez_account)
        .then(|| AccountId::new(*taker_lez_account))
}

/// The matched ETH lock under consideration, as read off the `Locked` event.
pub struct Candidate<'a> {
    /// The HASHLOCK — the only sound dedupe key on this path, and deliberately
    /// the only identifier the pure decision sees (see [`classify_candidate`]).
    pub hashlock: &'a [u8; 32],
    /// LEZ account the taker published as the designated claimant.
    pub taker_lez_account: &'a [u8; 32],
    /// Absolute on-chain ETH expiry, unix seconds.
    pub eth_timelock_secs: u64,
}

/// The maker's side of the decision: who it is, who (if anyone) it is willing
/// to serve, and the LEZ expiry/margin it would lock with.
pub struct MakerPolicy<'a> {
    /// The maker's OWN LEZ account — a candidate naming it is unusable.
    pub maker_lez_account: &'a [u8; 32],
    /// `Some` only for a designated-counterparty maker (`lez_taker_account_id`
    /// configured): serve just that taker. `None` means serve anyone, which is
    /// the point of binding the taker account on-chain.
    pub designated_taker: Option<&'a [u8; 32]>,
    /// The FRESH absolute LEZ expiry the maker would lock with (P1-1).
    pub lez_expiry_secs: u64,
    /// The margin the loop-mode maker configured (`TIMELOCK_MARGIN_MINUTES`).
    /// `None` for the single-shot maker — which now falls back to
    /// [`DEFAULT_TIMELOCK_MARGIN_SECS`] rather than skipping the gate. The
    /// ETH-covers-LEZ ordering check (P0-3) is UNCONDITIONAL: a single-shot
    /// maker used to accept a taker's `eth_timelock < lez_timelock`, letting a
    /// malicious taker refund ETH after its expiry and still claim LEZ.
    pub margin: Option<u64>,
}

/// Default ordering margin (seconds) applied when no loop-mode margin is
/// configured (the single-shot maker). Matches the loop's own
/// `TIMELOCK_MARGIN_MINUTES` default of 5 minutes (see `src/cli/maker.rs`).
pub const DEFAULT_TIMELOCK_MARGIN_SECS: u64 = 5 * 60;

/// Pure decision for one matched candidate. Kept side-effect-free so the P1-A
/// journal-skip belt, the P1-E rejected-set / timelock gate AND the
/// taker-account binding checks are unit-testable without a live watcher or
/// sequencer — and, critically, so every rejection happens before `journal
/// .record` and before any LEZ instruction is built.
///
/// # Both dedupe keys are the HASHLOCK, and that is load-bearing
///
/// The rejected-set used to be keyed by ETH swap id, so that a taker who locked
/// too-tight could re-lock correctly under a fresh escrow with the same secret
/// (P2-5). Binding `takerLezAccount` into the `swapId` preimage KILLED that
/// scheme: one sender can now mint UNLIMITED distinct swap ids for a single
/// hashlock just by varying that field, so a swap-id-keyed rejected set is
/// bypassable for free — every hostile re-lock looks like a brand-new candidate
/// and re-enters processing. The ETH and LEZ sides also no longer agree on
/// cardinality: LEZ derives ONE escrow PDA per hashlock and refuses any
/// pre-existing escrow, so at most one ETH lock per hashlock can ever be
/// served.
///
/// The invariant is therefore **one in-flight/decided swap per hashlock**, and
/// both gates key on it: the in-run rejected-set and the durable journal
/// (`in_journal`). The deliberate cost is that a rejection now burns the SECRET,
/// not just the escrow — a taker whose lock is turned away must retry with a
/// fresh preimage. That is one cheap client-side regeneration for them, versus
/// a free unlimited re-trigger for an attacker, so it is the right trade.
pub fn classify_candidate(
    candidate: &Candidate<'_>,
    policy: &MakerPolicy<'_>,
    rejected: &HashSet<[u8; 32]>,
    in_journal: bool,
) -> CandidateDisposition {
    if rejected.contains(candidate.hashlock) || in_journal {
        return CandidateDisposition::Skip;
    }
    // Structural first, and cheapest: a taker account we could never lock to.
    if !taker_lez_account_is_usable(candidate.taker_lez_account, policy.maker_lez_account) {
        return CandidateDisposition::Reject(RejectReason::UnusableTakerLezAccount);
    }
    if let Some(designated) = policy.designated_taker
        && candidate.taker_lez_account != designated
    {
        return CandidateDisposition::Reject(RejectReason::NotDesignatedTaker);
    }
    // P0-3: the ETH-covers-LEZ ordering gate is UNCONDITIONAL. A single-shot
    // maker (`margin: None`) used to skip it entirely, so a malicious taker
    // could lock ETH with `eth_timelock < lez_timelock`, refund it after its
    // (earlier) expiry, and still claim the maker's LEZ. Fall back to the same
    // default margin the loop uses instead of leaving the gate open.
    let margin_secs = policy.margin.unwrap_or(DEFAULT_TIMELOCK_MARGIN_SECS);
    if !eth_timelock_covers_lez(candidate.eth_timelock_secs, policy.lez_expiry_secs, margin_secs) {
        return CandidateDisposition::Reject(RejectReason::TimelockTooTight);
    }
    CandidateDisposition::Accept
}

/// How long the auto-accept loop waits before re-checking the balance after
/// an `AutoAcceptInsufficientFunds` iteration (issue #93 point 3). Distinct
/// from `poll_interval` (2s default): re-checking that fast would hammer the
/// sequencer for no reason while waiting on a background top-up that runs on
/// a multi-minute cadence (`--fund-check-secs`, default 300s).
const INSUFFICIENT_FUNDS_RETRY_DELAY: Duration = Duration::from_secs(30);

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
    // (P1-E). Keyed by HASHLOCK, not swap id: `takerLezAccount` is part of the
    // swapId preimage, so a hostile taker can mint unlimited distinct swap ids
    // for one hashlock and walk straight past a swap-id-keyed set (see
    // `rejected_set_keyed_by_hashlock_survives_swap_id_malleability`).
    const MAX_REJECTED: usize = 1024;
    let mut rejected: HashSet<[u8; 32]> = HashSet::new();

    // The maker's own LEZ account: a candidate naming it is unusable (the guest
    // asserts maker != taker), and that must be caught in the pure path below.
    let maker_lez_account: [u8; 32] = *lez_client.account_id().value();
    // `Some` only when the operator pinned a counterparty. Unset (the new
    // default) means the maker serves whoever locks ETH, binding each swap to
    // the LEZ account carried on that taker's own lock.
    let designated_taker: Option<[u8; 32]> =
        config.lez_taker_account_id.map(|id| *id.value());

    let (swap_id, discovered_hashlock, lez_expiry_secs, lez_claimant) = loop {
        tokio::select! {
            Some(event) = rx.recv() => {
                if let EthHtlcEvent::Locked {
                    swap_id,
                    recipient,
                    amount,
                    hashlock: event_hashlock,
                    timelock: event_timelock,
                    taker_lez_account,
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
                        // NOTE ordering: this whole decision — including the
                        // taker-LEZ-account binding checks — is PURE and runs
                        // before `journal.record` below. A candidate the maker
                        // cannot lock to must never leave a journal entry
                        // behind, because a non-empty journal stops the loop
                        // for a supervised restart (see
                        // `taker_lez_account_is_usable`).
                        let disposition = classify_candidate(
                            &Candidate {
                                hashlock: &hl,
                                taker_lez_account: &taker_lez_account.0,
                                eth_timelock_secs,
                            },
                            &MakerPolicy {
                                maker_lez_account: &maker_lez_account,
                                designated_taker: designated_taker.as_ref(),
                                lez_expiry_secs,
                                margin: guards.map(|g| g.timelock_margin_secs),
                            },
                            &rejected,
                            in_journal,
                        );
                        match disposition {
                            CandidateDisposition::Skip => {
                                info!(%swap_id, "maker: skipping ETH lock — already in-flight/handled");
                                continue;
                            }
                            CandidateDisposition::Reject(reason) => {
                                // Surface the rejection to progress consumers
                                // (CLI / GUI): without this the maker loop
                                // silently stays at WaitingForEthLock and the
                                // operator has no signal that a candidate was
                                // seen and turned away, or why. Additive only —
                                // the P1-E record-and-keep-waiting semantics
                                // are unchanged.
                                match reason {
                                    RejectReason::TimelockTooTight => {
                                        warn!(
                                            %swap_id,
                                            eth_timelock_secs,
                                            lez_expiry_secs,
                                            "maker: rejecting ETH lock — timelock too tight vs fresh LEZ \
                                             expiry + margin; waiting for other locks"
                                        );
                                        progress::report(&progress, SwapProgress::EthLockRejected {
                                            swap_id: format!("{swap_id}"),
                                            eth_expiry_secs: eth_timelock_secs,
                                            required_expiry_secs: lez_expiry_secs.saturating_add(
                                                guards
                                                    .map(|g| g.timelock_margin_secs)
                                                    .unwrap_or(DEFAULT_TIMELOCK_MARGIN_SECS),
                                            ),
                                        });
                                    }
                                    RejectReason::UnusableTakerLezAccount
                                    | RejectReason::NotDesignatedTaker => {
                                        warn!(
                                            %swap_id,
                                            taker_lez_account = %hex::encode(taker_lez_account.0),
                                            ?reason,
                                            "maker: rejecting ETH lock — its takerLezAccount is not \
                                             one this maker can lock to; waiting for other locks"
                                        );
                                        progress::report(&progress, SwapProgress::EthLockRejectedTakerAccount {
                                            swap_id: format!("{swap_id}"),
                                            taker_lez_account: hex::encode(taker_lez_account.0),
                                            reason: match reason {
                                                RejectReason::UnusableTakerLezAccount =>
                                                    "unusable takerLezAccount (zero, or the maker's own \
                                                     account — the LEZ HTLC requires maker != taker)".into(),
                                                _ => "not the designated counterparty this maker serves \
                                                      (LEZ_TAKER_ACCOUNT_ID)".to_string(),
                                            },
                                        });
                                    }
                                }
                                // Keyed by HASHLOCK, not swap id: since
                                // `takerLezAccount` entered the swapId preimage
                                // one sender can mint unlimited swap ids for a
                                // single hashlock, so a swap-id-keyed set is a
                                // free bypass (see `classify_candidate`).
                                if rejected.len() < MAX_REJECTED {
                                    rejected.insert(hl);
                                }
                                continue;
                            }
                            CandidateDisposition::Accept => {}
                        }

                        // Resolve the LEZ claimant HERE, at match time and
                        // still inside the pure/pre-journal region, so the
                        // conversion can never fail later: a `None` after
                        // `journal.record` would return Err with a non-empty
                        // journal and stop the loop (a free restart-DoS). The
                        // classifier already guarantees Some; this is the
                        // suspenders, and it keeps the fallible step before the
                        // durable write.
                        let Some(lez_claimant) = designated_lez_claimant(
                            &taker_lez_account.0,
                            &maker_lez_account,
                        ) else {
                            warn!(%swap_id, "maker: unusable takerLezAccount slipped past classification — skipping");
                            if rejected.len() < MAX_REJECTED {
                                rejected.insert(hl);
                            }
                            continue;
                        };

                        // Verify the HTLC is still OPEN on-chain (skip stale swaps).
                        if let Ok(htlc) = eth_client.get_htlc(swap_id).await
                            && !matches!(htlc.state, SwapState::OPEN)
                        {
                            continue;
                        }

                        info!(
                            %swap_id,
                            tx_hash = %event_tx_hash,
                            taker_lez_account = %hex::encode(taker_lez_account.0),
                            "maker: matched ETH Locked event"
                        );
                        progress::report(&progress, SwapProgress::EthLockDetected {
                            swap_id: format!("{swap_id}"),
                            hashlock: hex::encode(hl),
                            taker_lez_account: hex::encode(taker_lez_account.0),
                            tx_hash: format!("{event_tx_hash}"),
                            chain_id: eth_client.chain_id(),
                        });
                        break (swap_id, hl, lez_expiry_secs, lez_claimant);
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
        // issue #98: record the durable, privacy-safe "accepted" ops event
        // AFTER the fund-safety journal write succeeded, and only in loop
        // mode (a single-shot interactive run is not public-trial
        // operations). Idempotent on hashlock — never double-counts a
        // restart-resumed swap, which never re-reaches this call site.
        guards.ops.accepted(&hashlock_hex, &format!("{swap_id}"));
    }

    // 2. Lock LEZ (short timelock), naming the taker's OWN LEZ account as read
    // off their ETH lock — never a statically configured one. The LEZ guest
    // gates Claim on `signer == escrow.taker_id`, so this binding is what lets
    // an arbitrary taker (not just a pre-agreed counterparty) collect. The
    // value was validated in the pure path above, before the journal write.
    progress::report(&progress, SwapProgress::LezLocking);
    let lez_lock_tx = lez_client
        .lock(hashlock, lez_claimant, config.lez_amount, lez_expiry_secs)
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
                            guards.ops.refunded(&hashlock_hex);
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
                            guards.ops.refunded(&hashlock_hex);
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
                        // → journal retained, ops NOT recorded terminal yet) so
                        // the loop STOPS for a supervised restart rather than
                        // racing past an unresolved fund-bearing entry (P1-B).
                        let eth_claim_tx = claim_eth_after_reveal(eth_client, swap_id, preimage).await?;
                        progress::report(&progress, SwapProgress::EthClaimed {
                            tx_hash: format!("{eth_claim_tx}"),
                        });
                        if let Some(guards) = guards {
                            guards.journal.clear(&hashlock_hex);
                            guards.ops.completed(&hashlock_hex);
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
        guards.ops.completed(&hashlock_hex);
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
    /// Transient sequencer/RPC errors on hot-path reads, tracked separately
    /// from `total_failed` (see [`SwapProgress::AutoAcceptTransientError`]).
    pub total_transient_errors: u32,
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
    ops: &dyn OpsRecorder,
) -> AutoAcceptResult {
    let mut completed: u32 = 0;
    let mut failed: u32 = 0;
    let mut transient: u32 = 0;
    let mut iteration: u32 = 0;

    let guards = LoopGuards {
        timelock_margin_secs,
        lez_timelock_secs: auto_config.lez_timelock_minutes * 60,
        ops,
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

        // Bounded-retried (issue #93 follow-up — the deployed maker's
        // per-iteration balance check had no retry of its own, so a transient
        // sequencer timeout burned an entire loop iteration and inflated
        // `failed` even though nothing about the swap itself went wrong). A
        // blip that recovers within the retry budget never reaches the `Err`
        // arm below at all; one that doesn't is reported as
        // `AutoAcceptTransientError`, NOT `AutoAcceptSwapFailed`.
        let balance_read = lez_client
            .get_balance_with_retry(&lez_client.account_id(), |e| {
                transient += 1;
                progress::report(
                    &progress,
                    SwapProgress::AutoAcceptTransientError {
                        iteration,
                        error: format!("balance check: {e}"),
                    },
                );
            })
            .await;
        match balance_read {
            Ok(balance) if balance < fresh_config.lez_amount => {
                progress::report(
                    &progress,
                    SwapProgress::AutoAcceptInsufficientFunds {
                        lez_balance: balance.to_string(),
                        lez_required: fresh_config.lez_amount.to_string(),
                    },
                );
                // Do NOT break (issue #93 point 3): under `restart:
                // unless-stopped`, exiting here turned "ran out of LEZ
                // inventory" into a silent crash loop that never refilled —
                // the process would exit, restart, immediately hit the same
                // shortfall, and exit again, with no offers on the board the
                // whole time. The CLI layer runs an independent background
                // top-up (`cli::bot::spawn_fund_topper`) that claims from the
                // pinata faucet whenever the balance drops below a low-water
                // mark; this loop just waits for that to land and re-checks.
                warn!(
                    balance = %balance,
                    required = %fresh_config.lez_amount,
                    "maker-loop: insufficient LEZ inventory — waiting for the background \
                     fund-topper instead of exiting"
                );
                tokio::time::sleep(INSUFFICIENT_FUNDS_RETRY_DELAY).await;
                continue;
            }
            Err(e) => {
                // Already counted + reported as a transient error by the
                // `on_transient` closure above (every attempt inside
                // `get_balance_with_retry` failed) — this is the sequencer
                // being unreachable, not a swap failure, so `failed` is
                // deliberately NOT incremented here.
                warn!(
                    "maker-loop: balance check exhausted retries — waiting before next \
                     iteration: {e}"
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
            total_transient_errors: transient,
        },
    );

    AutoAcceptResult {
        total_completed: completed,
        total_failed: failed,
        total_transient_errors: transient,
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

    // ── candidate classification ────────────────────────────────────────
    //
    // Test fixtures. MAKER is the maker's own LEZ account; TAKER an honest
    // stranger's. `candidate`/`policy` keep each test to the field it is about.
    const MAKER: [u8; 32] = [0xC0u8; 32];
    const TAKER: [u8; 32] = [0x7Eu8; 32];

    fn candidate<'a>(
        hashlock: &'a [u8; 32],
        taker: &'a [u8; 32],
        eth_timelock_secs: u64,
    ) -> Candidate<'a> {
        Candidate {
            hashlock,
            taker_lez_account: taker,
            eth_timelock_secs,
        }
    }

    fn policy<'a>(lez_expiry_secs: u64, margin: Option<u64>) -> MakerPolicy<'a> {
        MakerPolicy {
            maker_lez_account: &MAKER,
            designated_taker: None,
            lez_expiry_secs,
            margin,
        }
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
            classify_candidate(
                &candidate(&hl, &TAKER, lez),
                &policy(lez, Some(margin)),
                &rejected,
                false
            ),
            CandidateDisposition::Reject(RejectReason::TimelockTooTight)
        );
        rejected.insert(hl);
        // Every subsequent replay of the same lock → Skip (never re-processed).
        assert_eq!(
            classify_candidate(
                &candidate(&hl, &TAKER, lez),
                &policy(lez, Some(margin)),
                &rejected,
                false
            ),
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
            classify_candidate(
                &candidate(&hl, &TAKER, lez + 10_000),
                &policy(lez, Some(300)),
                &rejected,
                true
            ),
            CandidateDisposition::Skip
        );
        // Not in the journal and timelock safe → Accept.
        assert_eq!(
            classify_candidate(
                &candidate(&hl, &TAKER, lez + 10_000),
                &policy(lez, Some(300)),
                &rejected,
                false
            ),
            CandidateDisposition::Accept
        );
    }

    // P0-3: the single-shot maker (no guards ⇒ margin None) now applies the
    // ordering gate UNCONDITIONALLY, using `DEFAULT_TIMELOCK_MARGIN_SECS`. A
    // tight/inverted timelock that used to be silently Accepted — the hole a
    // malicious taker exploited to refund ETH early and still claim LEZ — is
    // now Rejected, exactly like the loop-mode maker.
    #[test]
    fn single_shot_maker_applies_default_timelock_gate() {
        let hl = [0x33u8; 32];
        let rejected: HashSet<[u8; 32]> = HashSet::new();
        // A "tight" ETH timelock (well under the LEZ expiry) is REJECTED even
        // when margin is None — the whole point of the P0-3 fix.
        assert_eq!(
            classify_candidate(
                &candidate(&hl, &TAKER, 1),
                &policy(1_000_000, None),
                &rejected,
                false
            ),
            CandidateDisposition::Reject(RejectReason::TimelockTooTight)
        );
        // And an honest single-shot candidate — ETH expiry beyond LEZ expiry by
        // more than the default margin — is still Accepted (no honest-path
        // regression). lez_expiry small so `lez + DEFAULT_TIMELOCK_MARGIN_SECS`
        // is comfortably cleared.
        assert_eq!(
            classify_candidate(
                &candidate(&hl, &TAKER, DEFAULT_TIMELOCK_MARGIN_SECS + 1_000),
                &policy(100, None),
                &rejected,
                false
            ),
            CandidateDisposition::Accept
        );
    }

    // ── the taker-LEZ-account binding ───────────────────────────────────

    // THE attack this ordering exists for. The LEZ guest asserts
    // `maker_id != taker_id`, so a lock naming the MAKER'S OWN account makes the
    // Lock instruction fail. Discovered after `journal.record`, that failure
    // returns Err with a non-empty journal — which
    // `loop_must_stop_after_iteration` reads as "funds unresolved, stop for a
    // supervised restart". One cheap ETH tx per restart would then keep the bot
    // permanently down. It must be a PURE rejection instead, with no journal
    // write at all: below, the journal is untouched after classification.
    #[test]
    fn hostile_taker_account_equal_to_maker_is_rejected_without_journal_write() {
        let hl = [0x44u8; 32];
        let lez = 1_000u64;
        let rejected: HashSet<[u8; 32]> = HashSet::new();
        let journal = MockJournal::default();

        // The hostile lock names the maker's own LEZ account, and is otherwise
        // perfect: right recipient, generous timelock — it would pass every
        // pre-existing gate.
        let disposition = classify_candidate(
            &candidate(&hl, &MAKER, lez + 10_000),
            &MakerPolicy {
                maker_lez_account: &MAKER,
                designated_taker: None,
                lez_expiry_secs: lez,
                margin: Some(300),
            },
            &rejected,
            journal.contains(&hex::encode(hl)),
        );

        assert_eq!(
            disposition,
            CandidateDisposition::Reject(RejectReason::UnusableTakerLezAccount)
        );
        // The whole point: classification is side-effect-free, so reaching a
        // rejection cannot have recorded anything — and therefore cannot arm
        // the journal-driven loop stop.
        assert!(
            !journal.has_in_flight(),
            "a hostile takerLezAccount must never leave a journal entry — a non-empty \
             journal stops the maker loop, making this a free restart-DoS"
        );
        // And the conversion the lock path uses refuses it too (suspenders).
        assert!(designated_lez_claimant(&MAKER, &MAKER).is_none());
    }

    // The contract rejects bytes32(0) at lock time, but the maker must not
    // depend on the contract it is talking to being the one it thinks it is.
    #[test]
    fn zero_taker_account_is_rejected() {
        let hl = [0x55u8; 32];
        let zero = [0u8; 32];
        let rejected: HashSet<[u8; 32]> = HashSet::new();
        assert_eq!(
            classify_candidate(
                &candidate(&hl, &zero, 100_000),
                &policy(1_000, Some(300)),
                &rejected,
                false
            ),
            CandidateDisposition::Reject(RejectReason::UnusableTakerLezAccount)
        );
        assert!(designated_lez_claimant(&zero, &MAKER).is_none());
        // An honest stranger's account is usable and converts to that account.
        assert_eq!(
            designated_lez_claimant(&TAKER, &MAKER).map(|a| *a.value()),
            Some(TAKER)
        );
    }

    // A designated-counterparty maker (LEZ_TAKER_ACCOUNT_ID set) serves only
    // that taker; with it unset — the new default — it serves anyone.
    #[test]
    fn designated_taker_acts_as_an_allowlist_when_set() {
        let hl = [0x66u8; 32];
        let other = [0x99u8; 32];
        let rejected: HashSet<[u8; 32]> = HashSet::new();
        let designated = MakerPolicy {
            maker_lez_account: &MAKER,
            designated_taker: Some(&TAKER),
            lez_expiry_secs: 1_000,
            margin: Some(300),
        };
        assert_eq!(
            classify_candidate(
                &candidate(&hl, &other, 100_000),
                &designated,
                &rejected,
                false
            ),
            CandidateDisposition::Reject(RejectReason::NotDesignatedTaker)
        );
        assert_eq!(
            classify_candidate(
                &candidate(&hl, &TAKER, 100_000),
                &designated,
                &rejected,
                false
            ),
            CandidateDisposition::Accept
        );
        // Unset: the stranger is served. This is the whole point of the change.
        assert_eq!(
            classify_candidate(
                &candidate(&hl, &other, 100_000),
                &policy(1_000, Some(300)),
                &rejected,
                false
            ),
            CandidateDisposition::Accept
        );
    }

    // The swapId-malleability bypass. `takerLezAccount` is now part of the
    // swapId preimage, so ONE sender can mint unlimited distinct swap ids for a
    // single hashlock at will. A swap-id-keyed rejected set would therefore see
    // every hostile re-lock as a brand-new candidate and re-process it forever;
    // keying by hashlock closes it — the second attempt is Skipped even though
    // its swap id (and its takerLezAccount) are different.
    #[test]
    fn rejected_set_keyed_by_hashlock_survives_swap_id_malleability() {
        let hl = [0x77u8; 32]; // ONE hashlock...
        let lez = 1_000_000u64;
        let margin = 300u64;
        let mut rejected: HashSet<[u8; 32]> = HashSet::new();

        // Attempt 1: too-tight timelock → Reject; the caller records the
        // HASHLOCK (not the swap id).
        assert_eq!(
            classify_candidate(
                &candidate(&hl, &TAKER, lez),
                &policy(lez, Some(margin)),
                &rejected,
                false
            ),
            CandidateDisposition::Reject(RejectReason::TimelockTooTight)
        );
        rejected.insert(hl);

        // Attempt 2: the attacker varies takerLezAccount, which mints a fresh
        // swapId on-chain for free — the same secret, a new escrow id. Under
        // swap-id keying this would sail through as unseen; under hashlock
        // keying it is Skipped.
        let other_account = [0xABu8; 32];
        assert_eq!(
            classify_candidate(
                &candidate(&hl, &other_account, lez),
                &policy(lez, Some(margin)),
                &rejected,
                false
            ),
            CandidateDisposition::Skip,
            "varying takerLezAccount mints a new swapId for the SAME hashlock — it must \
             not re-enter processing"
        );
        // Even a now-generous timelock cannot revive the burnt hashlock: LEZ
        // derives one escrow PDA per hashlock, so there is nothing to re-serve.
        assert_eq!(
            classify_candidate(
                &candidate(&hl, &other_account, lez + 100_000),
                &policy(lez, Some(margin)),
                &rejected,
                false
            ),
            CandidateDisposition::Skip
        );
        // A genuinely different swap (fresh secret ⇒ fresh hashlock) is served
        // normally — the rejection burns the secret, not the taker.
        let fresh_hl = [0x78u8; 32];
        assert_eq!(
            classify_candidate(
                &candidate(&fresh_hl, &TAKER, lez + 100_000),
                &policy(lez, Some(margin)),
                &rejected,
                false
            ),
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

    // (The former P2-5 test asserted the opposite keying — rejected-set by ETH
    // swap id, so a corrected same-secret re-lock under a fresh escrow id was
    // still served. Binding `takerLezAccount` into the swapId preimage made
    // swap ids attacker-mintable per hashlock, which turned that leniency into
    // a free unlimited re-trigger; it is replaced by
    // `rejected_set_keyed_by_hashlock_survives_swap_id_malleability` above.)

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
