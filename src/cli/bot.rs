//! Liquidity-bot support for `swap-cli maker --loop`.
//!
//! Provides the pieces that turn the auto-accept maker loop
//! ([`crate::swap::maker::run_maker_loop`]) into an unattended daemon:
//!
//! - **State file + reconciliation** — the LEZ chain has no "list escrows by
//!   owner" query, so in-flight hashlocks are journaled to a small JSON file.
//!   On startup each journaled escrow is inspected on-chain and either
//!   refunded (LEZ timelock expired), completed (taker already revealed the
//!   preimage — claim the ETH), resumed (still live — background watcher), or
//!   dropped (terminal on-chain state).
//! - **Startup guards** — timelock-margin and LEZ-inventory validation.
//! - **Heartbeat offer publisher** — supervises a long-lived Node.js
//!   `@waku/sdk` lightpush sidecar that republishes the offer on an interval
//!   (the fleet runs `store=false`, so late-joining subscribers only see live
//!   messages).
//! - **Pinata faucet sidecar** (`--fund-to`) — loops `wallet pinata claim`
//!   (150 LEZ per claim) until the maker balance reaches a target.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use alloy::primitives::FixedBytes;
use lee::AccountId;
use lee_core::program::ProgramId;
use lez_htlc_program::{HTLCEscrow, HTLCState};
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;
use tracing::{debug, error, info, warn};

use crate::config::{SwapConfig, account_id_to_base58};
use crate::error::{Result, SwapError};
use crate::eth::client::EthClient;
use crate::lez::client::{LezClient, RefundOutcome};
use crate::lez::watcher::{self as lez_watcher, LezHtlcEvent};
use crate::swap::refund::now_unix;

// ---------------------------------------------------------------------------
// Timelock / inventory guards
// ---------------------------------------------------------------------------

/// Loop-mode timelock defaults (minutes), applied when the operator does not set
/// `LEZ_TIMELOCK_MINUTES` / `ETH_TIMELOCK_MINUTES`. The standing bot uses LONGER
/// defaults than the single-shot flow (5/10): on the public testnet, LEZ lock
/// *confirmation alone* can take up to 300s, so a 5-minute LEZ timelock leaves
/// almost no margin for the maker to observe the preimage and claim before the
/// taker's ETH refund window opens. 20/40 (with the 5-minute safety margin)
/// gives the standing bot a safe window; single-shot / demo runs keep 5/10 for
/// fast local iteration. An explicit env/flag value always wins over both.
pub const LOOP_DEFAULT_LEZ_TIMELOCK_MINUTES: u64 = 20;
pub const LOOP_DEFAULT_ETH_TIMELOCK_MINUTES: u64 = 40;

/// Transit slack (minutes) the startup timelock guard demands ON TOP of the
/// safety margin. The runtime gate compares the taker's *absolute* on-chain ETH
/// expiry (anchored when the TAKER parsed its config: `taker_now + eth_minutes`)
/// against a *fresh* LEZ expiry the maker computes at match time
/// (`match_now + lez_minutes`). So the gate effectively requires
/// `eth_minutes >= lez_minutes + margin + (match_now - taker_now)` — and that
/// last delta (taker config-parse → lock tx signed → mined on Sepolia →
/// delivered by the event watcher) is never zero. A config that only satisfies
/// `eth >= lez + margin` exactly therefore passes startup validation but loses
/// the runtime race on EVERY candidate, wedging the loop at WaitingForEthLock
/// (live-repro'd 2026-08-03 with the shipped 10/5/5 defaults). Two minutes
/// comfortably covers Sepolia inclusion under congestion (~12s blocks, a few
/// blocks of mempool delay) plus watcher polling, while keeping demo configs
/// fast.
pub const TIMELOCK_TRANSIT_SLACK_MINUTES: u64 = 2;

/// Validate the maker-safety timelock invariant before the first offer:
/// the ETH (taker, long) timelock must exceed the LEZ (maker, short) timelock
/// by at least `margin_minutes` PLUS [`TIMELOCK_TRANSIT_SLACK_MINUTES`]. A
/// margin below 5 minutes is rejected — the EthHTLC contract enforces
/// `minTimelockDelta = 300s` and the maker needs room to observe the preimage
/// and claim. The extra slack accounts for the taker's lock-transit delay: the
/// runtime gate re-anchors the LEZ expiry at match time, so a boundary-exact
/// config would start cleanly and then reject every single ETH lock (see the
/// slack constant's doc for the arithmetic).
pub fn validate_timelocks(lez_minutes: u64, eth_minutes: u64, margin_minutes: u64) -> Result<()> {
    if margin_minutes < 5 {
        return Err(SwapError::InvalidConfig(format!(
            "timelock margin must be >= 5 minutes (got {margin_minutes}); \
             EthHTLC minTimelockDelta is 300s and the maker needs claim headroom"
        )));
    }
    let required = lez_minutes
        .saturating_add(margin_minutes)
        .saturating_add(TIMELOCK_TRANSIT_SLACK_MINUTES);
    if eth_minutes < required {
        return Err(SwapError::InvalidConfig(format!(
            "unsafe timelocks: ETH_TIMELOCK_MINUTES ({eth_minutes}) must be >= \
             LEZ_TIMELOCK_MINUTES ({lez_minutes}) + margin ({margin_minutes}) + \
             transit slack ({TIMELOCK_TRANSIT_SLACK_MINUTES}) = {required}. \
             The taker locks first with the LONG timelock; the maker locks second \
             with the SHORT one, and the maker's runtime gate re-anchors the LEZ \
             expiry at match time — without the slack, every taker lock loses the \
             transit race and is rejected. Raise ETH_TIMELOCK_MINUTES to at least \
             {required} (or lower LEZ_TIMELOCK_MINUTES / the margin)."
        )));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Offer payload helpers
// ---------------------------------------------------------------------------

/// Encode a LEZ `ProgramId` (`[u32; 8]`) to the 64-hex wire form used in the
/// offer payload. Inverse of [`crate::config::parse_program_id`] (little-endian
/// per u32 word).
pub fn program_id_to_hex(id: &ProgramId) -> String {
    let mut bytes = Vec::with_capacity(32);
    for word in id {
        bytes.extend_from_slice(&word.to_le_bytes());
    }
    hex::encode(bytes)
}

// ---------------------------------------------------------------------------
// In-flight swap journal (crash recovery)
// ---------------------------------------------------------------------------

/// Lifecycle stage of a journaled in-flight swap (P2-2). The entry is written
/// `PreLock` durably BEFORE `LezClient::lock`, and upgraded to `Funded` only
/// after the lock is confirmed on-chain. The distinction lets reconciliation
/// tell apart two "no escrow on chain" cases that are otherwise identical:
///   - `PreLock` + conclusively-absent escrow ⇒ the lock never committed
///     (RPC failure/rejection before commit); nothing was ever locked, so the
///     entry is RETIRED (tombstoned) instead of retained-forever, which would
///     otherwise wedge sequential startup on every restart.
///   - `Funded` + absent escrow ⇒ ambiguous phantom/short read of an escrow
///     that DOES exist; must be retained (dropping it would strand locked LEZ).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SwapStage {
    /// Journaled before the LEZ lock; no on-chain escrow is guaranteed to exist.
    PreLock,
    /// The LEZ lock is confirmed; an on-chain escrow exists for this hashlock.
    Funded,
}

impl Default for SwapStage {
    /// Back-compat: entries in state files written before the `stage` field
    /// existed were only ever persisted AFTER a confirmed lock, so they are
    /// `Funded`. `#[serde(default)]` on the field uses this.
    fn default() -> Self {
        Self::Funded
    }
}

/// One journaled in-flight swap: recorded when the maker matches an ETH lock
/// (just before locking LEZ), removed when the swap reaches a terminal state.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InFlightSwap {
    /// 64-char lowercase hex, no 0x prefix.
    pub hashlock: String,
    /// 0x-prefixed 32-byte hex (EthHTLC swap id), if known.
    pub swap_id: String,
    /// Unix seconds when the entry was recorded.
    pub recorded_at: u64,
    /// Lifecycle stage (P2-2). `#[serde(default)]` = `Funded` for state files
    /// written before this field existed (those were only persisted post-lock).
    #[serde(default)]
    pub stage: SwapStage,
}

/// A swap moved out of the active journal because it can never terminalize and
/// would otherwise wedge sequential startup forever. See [`is_partial_lock_wedge`].
/// Quarantined entries are NEVER retried and NEVER block startup — they are kept
/// only so operators can see the permanently-stranded hashlock and know the
/// secret must not be reused.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuarantinedSwap {
    /// 64-char lowercase hex, no 0x prefix.
    pub hashlock: String,
    /// 0x-prefixed 32-byte hex (EthHTLC swap id), if known.
    pub swap_id: String,
    /// Why the entry was quarantined.
    pub reason: String,
    /// Unix seconds when the entry was quarantined.
    pub quarantined_at: u64,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct BotState {
    in_flight: Vec<InFlightSwap>,
    /// Entries that can never reach a terminal state (e.g. a committed lock whose
    /// funding transfer never landed). Separate from `in_flight` so they do not
    /// block startup or get retried (P1-4). `#[serde(default)]` keeps older
    /// state files (without this field) loadable.
    #[serde(default)]
    quarantined: Vec<QuarantinedSwap>,
}

/// Small JSON journal of in-flight swaps, persisted on every mutation.
/// Needed because LEZ escrow PDAs are derived from the hashlock and cannot be
/// enumerated by owner — without the journal a crash strands locked LEZ until
/// someone replays the hashlock by hand.
pub struct StateStore {
    path: PathBuf,
    state: Mutex<BotState>,
}

impl StateStore {
    pub fn load(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref().to_path_buf();
        let state = match std::fs::read_to_string(&path) {
            Ok(raw) => serde_json::from_str(&raw).map_err(|e| {
                SwapError::InvalidConfig(format!(
                    "corrupt maker state file {}: {e} (move it aside to start fresh)",
                    path.display()
                ))
            })?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => BotState::default(),
            Err(e) => {
                return Err(SwapError::InvalidConfig(format!(
                    "cannot read maker state file {}: {e}",
                    path.display()
                )));
            }
        };
        Ok(Self {
            path,
            state: Mutex::new(state),
        })
    }

    /// Durably serialize the journal: write a temp file, `fsync` it, atomically
    /// rename over the target, then best-effort `fsync` the directory so the
    /// rename itself survives a crash. Returns `Err` on any I/O failure so the
    /// caller can refuse to proceed (e.g. lock LEZ) without a durable record.
    fn persist(&self, state: &BotState) -> std::io::Result<()> {
        use std::io::Write as _;

        let json = serde_json::to_string_pretty(state)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        let tmp = self.path.with_extension("json.tmp");
        {
            let mut f = std::fs::File::create(&tmp)?;
            f.write_all(json.as_bytes())?;
            f.sync_all()?;
        }
        std::fs::rename(&tmp, &self.path)?;
        // fsync the directory so the rename's new dir entry survives power loss.
        // On the critical pre-lock `record` path this MUST NOT be swallowed: a
        // dropped dir entry after `record` returned Ok would lose the journal on
        // power loss and strand locked LEZ (the escrow PDA can't be enumerated by
        // owner). Propagate every failure so the caller refuses to lock (P1-D).
        let dir = match self.path.parent() {
            Some(p) if !p.as_os_str().is_empty() => p,
            _ => Path::new("."),
        };
        let d = std::fs::File::open(dir)?;
        d.sync_all()?;
        Ok(())
    }

    /// Best-effort persist: logs on failure. Used by non-critical mutations.
    fn persist_logged(&self, state: &BotState) {
        if let Err(e) = self.persist(state) {
            error!("maker state write failed ({}): {e}", self.path.display());
        }
    }

    /// Durably record an in-flight swap (idempotent on hashlock), returning
    /// `Err` if the write cannot be made durable. This is the pre-lock journal
    /// path: the maker must not lock LEZ unless this succeeds.
    pub fn record(&self, swap: InFlightSwap) -> Result<()> {
        let mut state = self.state.lock().expect("state lock");
        if state.in_flight.iter().any(|s| s.hashlock == swap.hashlock) {
            return Ok(());
        }
        state.in_flight.push(swap);
        self.persist(&state).map_err(|e| {
            // Roll back the in-memory add so a later retry can re-record.
            state.in_flight.pop();
            SwapError::InvalidConfig(format!(
                "failed to durably journal in-flight swap to {}: {e}",
                self.path.display()
            ))
        })
    }

    /// Upgrade a journaled entry from `PreLock` to `Funded` (P2-2), called after
    /// the LEZ lock is confirmed on-chain. Best-effort persist: a lost upgrade is
    /// self-healing — reconciliation will find the still-`PreLock` entry next to
    /// its existing on-chain escrow and re-upgrade it, so a dropped write only
    /// costs a redundant upgrade, never a wrong retirement. No-op if absent.
    pub fn mark_funded(&self, hashlock: &str) {
        let mut state = self.state.lock().expect("state lock");
        let mut changed = false;
        for s in &mut state.in_flight {
            if s.hashlock == hashlock && s.stage != SwapStage::Funded {
                s.stage = SwapStage::Funded;
                changed = true;
            }
        }
        if changed {
            self.persist_logged(&state);
        }
    }

    /// Remove a swap by hashlock (no-op if absent).
    pub fn remove(&self, hashlock: &str) {
        let mut state = self.state.lock().expect("state lock");
        let before = state.in_flight.len();
        state.in_flight.retain(|s| s.hashlock != hashlock);
        if state.in_flight.len() != before {
            self.persist_logged(&state);
        }
    }

    pub fn snapshot(&self) -> Vec<InFlightSwap> {
        self.state.lock().expect("state lock").in_flight.clone()
    }

    /// Snapshot of quarantined (permanently-unusable) entries.
    pub fn quarantined_snapshot(&self) -> Vec<QuarantinedSwap> {
        self.state.lock().expect("state lock").quarantined.clone()
    }

    /// Move an in-flight entry to quarantine with a reason (idempotent on
    /// hashlock). Used for the partial-lock wedge that can never terminalize
    /// (P1-4): it is removed from `in_flight` (so it stops blocking startup and
    /// is never retried) and recorded under `quarantined` for operator
    /// visibility. Best-effort persist.
    pub fn quarantine(&self, hashlock: &str, reason: String) {
        let mut state = self.state.lock().expect("state lock");
        let entry = state.in_flight.iter().find(|s| s.hashlock == hashlock);
        let swap_id = entry.map(|s| s.swap_id.clone()).unwrap_or_default();
        state.in_flight.retain(|s| s.hashlock != hashlock);
        if !state.quarantined.iter().any(|q| q.hashlock == hashlock) {
            state.quarantined.push(QuarantinedSwap {
                hashlock: hashlock.to_string(),
                swap_id,
                reason,
                quarantined_at: now_unix(),
            });
        }
        self.persist_logged(&state);
    }

    /// Whether a hashlock is currently journaled as in-flight.
    pub fn contains(&self, hashlock: &str) -> bool {
        self.state
            .lock()
            .expect("state lock")
            .in_flight
            .iter()
            .any(|s| s.hashlock == hashlock)
    }

    /// Whether any (non-quarantined) in-flight entry remains.
    pub fn has_in_flight(&self) -> bool {
        !self.state.lock().expect("state lock").in_flight.is_empty()
    }
}

/// The maker loop drives the journal through this trait (defined in the swap
/// layer): a durable pre-lock record and a post-terminal clear.
impl crate::swap::maker::SwapJournal for StateStore {
    fn record(&self, hashlock_hex: &str, swap_id: &str) -> Result<()> {
        StateStore::record(
            self,
            InFlightSwap {
                hashlock: hashlock_hex.to_string(),
                swap_id: swap_id.to_string(),
                recorded_at: now_unix(),
                // P2-2: written BEFORE the lock — no on-chain escrow yet.
                stage: SwapStage::PreLock,
            },
        )
    }

    fn mark_funded(&self, hashlock_hex: &str) {
        StateStore::mark_funded(self, hashlock_hex);
    }

    fn clear(&self, hashlock_hex: &str) {
        self.remove(hashlock_hex);
    }

    fn contains(&self, hashlock_hex: &str) -> bool {
        StateStore::contains(self, hashlock_hex)
    }

    fn has_in_flight(&self) -> bool {
        StateStore::has_in_flight(self)
    }
}

// ---------------------------------------------------------------------------
// Reconciliation
// ---------------------------------------------------------------------------

fn parse_hashlock_hex(s: &str) -> Option<[u8; 32]> {
    let bytes = hex::decode(s.trim_start_matches("0x")).ok()?;
    bytes.try_into().ok()
}

/// Whether a reconciled escrow is the unrecoverable partial-lock wedge (P1-4):
/// the `Lock` instruction committed (the PDA exists and is `Locked`) but the
/// funding transfer never landed, leaving the escrow balance below the locked
/// amount. The LEZ HTLC `Refund` requires `balance >= amount`, so such an escrow
/// can NEVER be terminalized — a resume watcher would wait until expiry and then
/// refund forever without confirming, and the retained entry would wedge
/// sequential startup into a restart-forever loop. The funding transfer is a
/// single feeless all-or-nothing move, so in practice `balance == 0` here.
///
/// Conclusiveness (principle i): the wedge verdict additionally requires the
/// escrow to be EXPIRED. An unexpired underfunded escrow is ambiguous — the
/// funding transfer may still be in flight after a crash-and-fast-restart, and
/// quarantining it prematurely would strand LEZ that lands a block later. Such
/// an escrow resumes as live instead; if the funding truly never lands, the
/// refund cannot confirm, the entry is retained, and the next restart AFTER
/// expiry reaches the conclusive wedge verdict here.
fn is_partial_lock_wedge(
    state: HTLCState,
    now_ms: u64,
    timelock_ms: u64,
    balance: u128,
    amount: u128,
) -> bool {
    matches!(state, HTLCState::Locked) && now_ms >= timelock_ms && balance < amount
}

/// Disposition of a suspected partial-lock wedge AFTER a fresh, paired re-read
/// (P1-1). The initial reconcile observation `(Locked, balance < amount)` is a
/// wedge *candidate* only — a taker `Claim` can land AFTER that read, revealing
/// the preimage. Quarantine is IRREVERSIBLE (it strands the LEZ and marks the
/// hashlock/secret permanently unusable), so it must never act on a single
/// stale observation. We re-read the escrow state and balance and classify the
/// fresh pair here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WedgeDisposition {
    /// Fresh read still shows `Locked` AND the fresh balance is still below the
    /// amount at/after expiry — the wedge is genuine and conclusive: quarantine.
    Quarantine,
    /// Fresh read shows the taker `Claimed` after our stale read — the preimage
    /// is on-chain: recover the ETH side instead of quarantining.
    RecoverClaimed,
    /// Fresh read shows `Refunded` — already terminal, drop the entry.
    Dropped,
    /// Fresh read inconclusive: absent/phantom, all balance reads failed
    /// (outage — P1-2), or no longer wedged. Retain the entry and retry on the
    /// next reconcile; never quarantine on a non-conclusive read.
    Retain,
}

/// Classify a suspected wedge from its FRESH paired re-read, enforcing the read
/// ORDER `state → balance → STATE` (P1-1 / P1-2). Pure so the decision is
/// unit-testable without a live sequencer.
///
/// A taker `Claim` can commit in the WINDOW between the pre-balance state read
/// and the balance read: the escrow drains to zero, so a cached `Locked`
/// pre-state paired with a fresh zero balance would misfire quarantine and burn
/// the revealed preimage / forfeit the ETH claim. Closing that window requires a
/// FINAL state read taken AFTER the balance observation — quarantine is
/// conclusive only if BOTH the pre- and post-balance state reads still show
/// `Locked`. A `Claimed` at EITHER read means the preimage is on-chain and routes
/// to ETH recovery.
///
/// - `pre_state`: escrow state read BEFORE the balance (`None` = absent/phantom).
/// - `balance`: highest balance across the bounded re-read (`None` = every read
///   failed; an outage must NOT be read as zero → no quarantine).
/// - `post_state`: escrow state read AFTER the balance — the authoritative final
///   read that a mid-window claim would reveal (`None` = absent/phantom).
fn classify_wedge_reread(
    pre_state: Option<HTLCState>,
    balance: Option<u128>,
    post_state: Option<HTLCState>,
    now_ms: u64,
    timelock_ms: u64,
    amount: u128,
) -> WedgeDisposition {
    // A taker `Claim` observed at EITHER the pre- or post-balance state read
    // means the preimage is on-chain — recover the ETH, never quarantine. This
    // is the P1-1 window: a claim that commits between the state and balance
    // reads drains the escrow to zero, so a cached `Locked` + fresh zero must not
    // be mistaken for the funding-never-landed wedge.
    if matches!(pre_state, Some(HTLCState::Claimed))
        || matches!(post_state, Some(HTLCState::Claimed))
    {
        return WedgeDisposition::RecoverClaimed;
    }
    // Refunded at the FINAL read is terminal on the maker side.
    if matches!(post_state, Some(HTLCState::Refunded)) {
        return WedgeDisposition::Dropped;
    }
    // Quarantine is conclusive ONLY when BOTH the pre- and post-balance state
    // reads show `Locked` (so no claim slipped through the window) AND the low
    // balance is confirmed at/after expiry. Any ambiguity — an absent read at
    // either end, all-failed balance (`None`), or a now-sufficient balance —
    // routes to retain-and-retry; quarantine is irreversible.
    match (pre_state, post_state, balance) {
        (Some(HTLCState::Locked), Some(HTLCState::Locked), Some(b))
            if is_partial_lock_wedge(HTLCState::Locked, now_ms, timelock_ms, b, amount) =>
        {
            WedgeDisposition::Quarantine
        }
        _ => WedgeDisposition::Retain,
    }
}

/// What reconciliation does with a journal entry whose escrow reads absent on
/// every bounded read — decided by the journal STAGE (P2-2) and by whether the
/// absence was AUTHORITATIVE (round 6): only `Ok(None)` reads (the sequencer
/// answered and reported no account) are authoritative; an `Err` read (RPC
/// outage) says nothing about the escrow. Pure so the decision is unit-testable
/// without a live sequencer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AbsentEscrowAction {
    /// `PreLock` entry + every read was an authoritative `Ok(None)` ⇒ the LEZ
    /// lock failed pre-commit; nothing was ever locked. RETIRE it (tombstone
    /// into quarantine with `pre-lock-abandoned`) so it stops wedging startup —
    /// but never reuse the hashlock/secret.
    RetirePreLock,
    /// `Funded` entry ⇒ an escrow DID exist on-chain; an all-`None` read is an
    /// ambiguous phantom/short read (the LEZ HTLC program never deletes an
    /// account). RETAIN and retry so locked LEZ is never stranded.
    RetainFunded,
    /// At least one read attempt ERRORED (sequencer unreachable), so the
    /// all-absent result is an outage artifact, not authoritative absence.
    /// RETAIN regardless of stage (round-6 P2): a `PreLock` entry whose lock
    /// DID commit (crash before `mark_funded` persisted) would otherwise be
    /// retired during an outage, nobody would watch the funded escrow, and the
    /// taker's claim would reveal the preimage with no maker ETH claim —
    /// fund loss. The genuinely never-locked entry still retires on the next
    /// reconcile once the sequencer is reachable and reports absence.
    RetainInconclusive,
}

/// Classify an all-absent escrow read by journal stage and read conclusiveness
/// (P2-2, round 6).
fn classify_absent_escrow(stage: SwapStage, any_read_errored: bool) -> AbsentEscrowAction {
    if any_read_errored {
        return AbsentEscrowAction::RetainInconclusive;
    }
    match stage {
        SwapStage::PreLock => AbsentEscrowAction::RetirePreLock,
        SwapStage::Funded => AbsentEscrowAction::RetainFunded,
    }
}

/// Outcome of a bounded escrow read (round 6): the escrow, if any read
/// succeeded, plus enough about the failed attempts for the caller to tell an
/// AUTHORITATIVE all-absent result (every read `Ok(None)` — the sequencer
/// answered and reported no account) apart from an outage-tainted one (some
/// read `Err` — the sequencer may simply have been unreachable).
#[derive(Debug)]
struct BoundedEscrowRead {
    /// The escrow from the first conclusive read, or `None` when every read
    /// was absent/errored.
    escrow: Option<HTLCEscrow>,
    /// `true` iff at least one read attempt returned `Err`. When `escrow` is
    /// `None` and this is set, the absence is NOT authoritative and the caller
    /// must RETAIN the journal entry.
    any_err: bool,
    /// Number of authoritative `Ok(None)` reads observed (for operator-facing
    /// messages).
    absent_reads: u32,
}

/// Re-read an escrow up to a bounded number of times, tolerating ambiguous
/// reads (P1-2, principle i). `get_escrow` returns `Ok(None)` for
/// missing/short/phantom data — which is NOT conclusive absence (the LEZ HTLC
/// program never deletes an account; refund leaves it `Refunded` with intact
/// data). A transient sequencer error is equally ambiguous. Returns a
/// [`BoundedEscrowRead`] whose `escrow` is:
/// - `Some(escrow)` on the first conclusive read of an existing escrow.
/// - `None` only when EVERY read was absent/errored — the caller must RETAIN the
///   journal entry, never drop it (a phantom/short read while the escrow exists
///   would otherwise strand locked LEZ), UNLESS every read was an authoritative
///   `Ok(None)` (`any_err == false`) AND the caller's own state says nothing was
///   ever locked (the `PreLock` retire path). A never-locked entry that reads
///   `None` forever costs only a redundant idempotent reconcile each restart.
async fn read_escrow_bounded(lez: &LezClient, hashlock: &[u8; 32]) -> BoundedEscrowRead {
    let hl = *hashlock;
    read_escrow_bounded_with(|| lez.get_escrow(&hl), lez.poll_interval(), hashlock).await
}

/// Generic core of [`read_escrow_bounded`], parameterized over the fetch so the
/// bounded-retry / retain-on-ambiguity behaviour is unit-testable without a
/// live sequencer.
async fn read_escrow_bounded_with<F, Fut>(
    mut fetch: F,
    poll: std::time::Duration,
    hashlock: &[u8; 32],
) -> BoundedEscrowRead
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<Option<HTLCEscrow>>>,
{
    const MAX_READS: u32 = 5;
    let mut any_err = false;
    let mut absent_reads = 0u32;
    for attempt in 1..=MAX_READS {
        match fetch().await {
            Ok(Some(escrow)) => {
                return BoundedEscrowRead {
                    escrow: Some(escrow),
                    any_err,
                    absent_reads,
                }
            }
            Ok(None) => {
                absent_reads += 1;
                debug!(
                    hashlock = %hex::encode(hashlock),
                    "reconcile: escrow read absent ({attempt}/{MAX_READS}) — ambiguous, retrying"
                );
            }
            Err(e) => {
                any_err = true;
                warn!(
                    hashlock = %hex::encode(hashlock),
                    "reconcile: escrow read error ({attempt}/{MAX_READS}): {e} — retrying"
                );
            }
        }
        if attempt < MAX_READS {
            tokio::time::sleep(poll).await;
        }
    }
    BoundedEscrowRead {
        escrow: None,
        any_err,
        absent_reads,
    }
}

/// Highest escrow-PDA balance observed across bounded re-reads, short-circuiting
/// as soon as `enough` is reached. Used to judge the partial-lock wedge robustly:
/// a single phantom-zero balance read must never misclassify a funded escrow as
/// unfunded (which would wrongly quarantine live LEZ), so we take the MAX over
/// several reads — any read that reaches `enough` proves the escrow is funded.
///
/// Returns `None` when EVERY read failed (P1-2): an all-failed outage is
/// indistinguishable from an observed zero, so the caller must NOT treat it as
/// "balance is zero" (which would falsely quarantine a recoverable escrow). A
/// `None` result means "unknown — retain and retry"; `Some(b)` means at least
/// one read succeeded and `b` is the highest balance observed.
async fn max_escrow_balance(lez: &LezClient, pda: &AccountId, enough: u128) -> Option<u128> {
    max_escrow_balance_with(|| lez.get_balance(pda), lez.poll_interval(), enough).await
}

/// Generic core of [`max_escrow_balance`], parameterized over the balance fetch
/// so the max-over-reads / all-failed→`None` behaviour is unit-testable without
/// a live sequencer.
async fn max_escrow_balance_with<F, Fut>(
    mut fetch: F,
    poll: std::time::Duration,
    enough: u128,
) -> Option<u128>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<u128>>,
{
    const MAX_READS: u32 = 5;
    let mut max: Option<u128> = None;
    for attempt in 1..=MAX_READS {
        if let Ok(balance) = fetch().await {
            let m = max.map_or(balance, |cur| cur.max(balance));
            max = Some(m);
            if m >= enough {
                return max;
            }
        }
        if attempt < MAX_READS {
            tokio::time::sleep(poll).await;
        }
    }
    max
}

fn parse_swap_id(s: &str) -> Option<FixedBytes<32>> {
    s.parse().ok()
}

/// Disposition of an ETH-claim recovery attempt — decides whether the journal
/// entry may be dropped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EthClaimOutcome {
    /// The maker's ETH was claimed by this call.
    Claimed,
    /// The ETH HTLC is already terminal on-chain (claimed or refunded) — there
    /// is nothing left to do, so the journal entry can be dropped.
    AlreadyTerminal,
    /// The claim could not be completed (unparseable id, RPC/init failure, or a
    /// still-OPEN HTLC that reverted). The journal entry MUST be retained so a
    /// later reconcile retries before the taker's ETH refund deadline.
    Failed,
}

/// Whether the journal entry must be retained after an ETH-claim attempt.
/// Retain only when the attempt failed and the ETH is still recoverable (P1-6).
pub fn retain_after_eth_claim(outcome: EthClaimOutcome) -> bool {
    matches!(outcome, EthClaimOutcome::Failed)
}

/// Claim the ETH side with a revealed preimage. Returns an [`EthClaimOutcome`]
/// so callers keep the journal entry on transient failure instead of dropping
/// it (a transient WS/RPC failure must not permanently disable the retry while
/// the taker's ETH refund deadline approaches).
async fn claim_eth(config: &SwapConfig, swap_id_str: &str, preimage: [u8; 32]) -> EthClaimOutcome {
    let Some(swap_id) = parse_swap_id(swap_id_str) else {
        warn!("reconcile: unparseable swap_id '{swap_id_str}', cannot claim ETH");
        // Unparseable id is permanent, not transient — nothing to retry.
        return EthClaimOutcome::AlreadyTerminal;
    };
    let eth = match EthClient::new(config).await {
        Ok(eth) => eth,
        Err(e) => {
            warn!("reconcile: ETH client init failed: {e}");
            return EthClaimOutcome::Failed;
        }
    };

    // Already terminal? Then there is nothing to claim and nothing to retry.
    if let Ok(htlc) = eth.get_htlc(swap_id).await
        && !matches!(htlc.state, crate::eth::client::EthHTLC::SwapState::OPEN)
    {
        info!(%swap_id, "reconcile: ETH HTLC already terminal, dropping entry");
        return EthClaimOutcome::AlreadyTerminal;
    }

    match eth.claim(swap_id, preimage).await {
        Ok(tx) => {
            info!(%tx, "reconcile: claimed ETH with recovered preimage");
            EthClaimOutcome::Claimed
        }
        Err(e) => {
            // The claim may have reverted because it landed after all — re-check.
            if let Ok(htlc) = eth.get_htlc(swap_id).await
                && matches!(htlc.state, crate::eth::client::EthHTLC::SwapState::CLAIMED)
            {
                info!(%swap_id, "reconcile: ETH already claimed on re-check, dropping entry");
                return EthClaimOutcome::AlreadyTerminal;
            }
            warn!("reconcile: ETH claim failed for {swap_id_str}, keeping entry: {e}");
            EthClaimOutcome::Failed
        }
    }
}

/// Recover the ETH side of an escrow the taker has already CLAIMED on LEZ (the
/// preimage is on-chain). Drops the journal entry once the ETH is claimed or is
/// already terminal; RETAINS it on a transient ETH-claim failure so the next
/// reconcile retries before the taker's ETH refund deadline (P1-6). Shared by
/// the up-front `Claimed` branch and the wedge re-read's `RecoverClaimed` path
/// (P1-1), so both recover identically.
async fn recover_claimed_escrow(
    config: &SwapConfig,
    entry: &InFlightSwap,
    escrow: &HTLCEscrow,
    store: &Arc<StateStore>,
) {
    match escrow
        .preimage
        .as_ref()
        .and_then(|p| <[u8; 32]>::try_from(p.as_slice()).ok())
    {
        Some(preimage) => {
            info!(hashlock = %entry.hashlock, "reconcile: escrow claimed, claiming ETH");
            let outcome = claim_eth(config, &entry.swap_id, preimage).await;
            if !retain_after_eth_claim(outcome) {
                store.remove(&entry.hashlock);
            }
        }
        None => {
            // LEZ is terminally claimed but the preimage is missing/invalid —
            // the ETH is unrecoverable and there is nothing to retry, so drop.
            error!(
                hashlock = %entry.hashlock,
                "reconcile: escrow claimed but preimage missing/invalid, dropping (ETH unrecoverable)"
            );
            store.remove(&entry.hashlock);
        }
    }
}

/// Background resumption of one live (unexpired, still-Locked) escrow found at
/// startup: watch it until the taker claims (→ claim ETH with the revealed
/// preimage) or the LEZ timelock expires (→ refund LEZ).
async fn resume_swap(
    config: SwapConfig,
    entry: InFlightSwap,
    hashlock: [u8; 32],
    timelock_ms: u64,
    store: Arc<StateStore>,
) {
    let lez_client = match LezClient::new(&config) {
        Ok(c) => c,
        Err(e) => {
            error!("resume: LEZ client init failed for {}: {e}", entry.hashlock);
            return;
        }
    };

    let (tx, mut rx) = mpsc::channel::<LezHtlcEvent>(16);
    let watcher_client = match LezClient::new(&config) {
        Ok(c) => c,
        Err(e) => {
            error!(
                "resume: LEZ watcher init failed for {}: {e}",
                entry.hashlock
            );
            return;
        }
    };
    let poll = config.poll_interval;
    let watcher = tokio::spawn(async move {
        let _ = lez_watcher::watch_escrow(&watcher_client, hashlock, poll, tx).await;
    });

    let expiry_secs = (timelock_ms / 1000).saturating_sub(now_unix());
    info!(
        hashlock = %entry.hashlock,
        "resume: watching live escrow ({expiry_secs}s until LEZ timelock)"
    );

    // Whether the journal entry may be cleared. Only a CONFIRMED terminal state
    // (ETH claimed, or LEZ refund observed on-chain) drops it — a transient
    // failure keeps it for the next reconcile (P1-6 / P1-7).
    let remove_entry = loop {
        tokio::select! {
            Some(event) = rx.recv() => match event {
                LezHtlcEvent::Claimed { preimage, .. } => {
                    match <[u8; 32]>::try_from(preimage) {
                        Ok(preimage) => {
                            info!(hashlock = %entry.hashlock, "resume: taker claimed LEZ, claiming ETH");
                            let outcome = claim_eth(&config, &entry.swap_id, preimage).await;
                            break !retain_after_eth_claim(outcome);
                        }
                        Err(_) => {
                            warn!(hashlock = %entry.hashlock, "resume: preimage wrong length, keeping entry");
                            break false;
                        }
                    }
                }
                LezHtlcEvent::Refunded { .. } => {
                    info!(hashlock = %entry.hashlock, "resume: escrow already refunded");
                    break true;
                }
                LezHtlcEvent::Locked { .. } => {}
            },
            _ = tokio::time::sleep(std::time::Duration::from_secs(expiry_secs.max(1))) => {
                info!(hashlock = %entry.hashlock, "resume: LEZ timelock expired, refunding");
                match lez_client.refund_confirmed(&hashlock).await {
                    Ok(RefundOutcome::Refunded(tx)) => {
                        if !tx.is_empty() {
                            info!(%tx, "resume: LEZ refunded");
                        }
                        break true;
                    }
                    Ok(RefundOutcome::ClaimedByTaker(preimage)) => {
                        info!(hashlock = %entry.hashlock, "resume: taker claimed during refund race, claiming ETH");
                        let outcome = claim_eth(&config, &entry.swap_id, preimage).await;
                        break !retain_after_eth_claim(outcome);
                    }
                    Err(e) => {
                        warn!(hashlock = %entry.hashlock, "resume: LEZ refund not confirmed, keeping entry: {e}");
                        break false;
                    }
                }
            }
        }
    };

    watcher.abort();
    if remove_entry {
        store.remove(&entry.hashlock);
    }
}

/// Startup reconciliation pass: inspect every journaled in-flight swap
/// on-chain and refund / complete / resume / drop it. Never fatal — failures
/// are logged and the entry retained for the next restart.
///
/// Returns join handles for any still-live escrows resumed in the background so
/// the caller can await them before exiting (rather than stranding a locked
/// escrow when the process shuts down right after startup).
#[must_use]
pub async fn reconcile(
    config: &SwapConfig,
    store: &Arc<StateStore>,
    json: bool,
) -> Vec<tokio::task::JoinHandle<()>> {
    let mut resume_handles = Vec::new();
    let entries = store.snapshot();
    if entries.is_empty() {
        return resume_handles;
    }
    if !json {
        println!(
            "Reconciling {} in-flight swap(s) from previous run...",
            entries.len()
        );
    }

    let lez_client = match LezClient::new(config) {
        Ok(c) => c,
        Err(e) => {
            error!("reconcile: LEZ client init failed, keeping journal: {e}");
            return resume_handles;
        }
    };

    for entry in entries {
        let Some(hashlock) = parse_hashlock_hex(&entry.hashlock) else {
            warn!(
                "reconcile: dropping unparseable hashlock '{}'",
                entry.hashlock
            );
            store.remove(&entry.hashlock);
            continue;
        };

        let bounded_read = read_escrow_bounded(&lez_client, &hashlock).await;
        match bounded_read.escrow {
            Some(escrow) => {
                // P2-2: an on-chain escrow exists for this hashlock, so the lock
                // DID commit. If the entry is still `PreLock` (crash between the
                // pre-lock journal write and the post-lock upgrade), promote it to
                // `Funded` before handling — it must never be mistaken for a
                // never-locked entry and retired.
                if entry.stage == SwapStage::PreLock {
                    info!(
                        hashlock = %entry.hashlock,
                        "reconcile: PreLock entry has an on-chain escrow — upgrading to Funded"
                    );
                    store.mark_funded(&entry.hashlock);
                }
                match escrow.state {
                HTLCState::Claimed => {
                    // Taker revealed the preimage while we were down — collect the ETH.
                    recover_claimed_escrow(config, &entry, &escrow, store).await;
                }
                HTLCState::Refunded => {
                    info!(hashlock = %entry.hashlock, "reconcile: escrow already refunded, dropping");
                    store.remove(&entry.hashlock);
                }
                HTLCState::Locked => {
                    // P1-4: detect the partial-lock wedge BEFORE the
                    // refund-vs-resume decision. If the Lock committed but the
                    // funding transfer never landed, the escrow is `Locked` with
                    // balance < amount and can NEVER be refunded (the program
                    // requires balance >= amount) — a resume/refund would loop
                    // forever and the retained entry would wedge sequential
                    // startup into a restart-forever cycle. Quarantine it: remove
                    // from the active journal (so startup proceeds and it is
                    // never retried) and record it for operator visibility. The
                    // balance is confirmed across bounded re-reads so a transient
                    // phantom-zero read can never misclassify a funded escrow,
                    // and only an EXPIRED underfunded escrow is conclusive (an
                    // unexpired one may still be waiting on an in-flight funding
                    // transfer — it resumes as live below instead).
                    let pda = lez_client.escrow_pda(&hashlock);
                    // P1-2: distinguish "observed low balance" from "every
                    // balance read failed" (an outage). `None` means unknown, not
                    // zero — retain and retry rather than falsely quarantine a
                    // recoverable escrow.
                    let balance = match max_escrow_balance(&lez_client, &pda, escrow.amount).await {
                        Some(b) => b,
                        None => {
                            warn!(
                                hashlock = %entry.hashlock,
                                "reconcile: every escrow-balance read failed (LEZ outage?) — cannot \
                                 judge partial-lock wedge; retaining entry for next reconcile"
                            );
                            continue;
                        }
                    };
                    if is_partial_lock_wedge(
                        escrow.state,
                        now_unix() * 1000,
                        escrow.timelock,
                        balance,
                        escrow.amount,
                    ) {
                        // P1-1: the initial read is a wedge CANDIDATE only — a
                        // taker `Claim` can land AFTER it. Quarantine is
                        // irreversible (strands the LEZ, burns the secret), so
                        // RE-READ following the ORDER state → balance → STATE and
                        // act only on the fresh triple. A claim that commits in the
                        // window between the pre-state read and the balance read
                        // drains the escrow to zero; the FINAL state read after the
                        // balance closes that hole. If the taker claimed, recover
                        // the ETH; quarantine only if BOTH state reads are still
                        // Locked and the escrow is still underfunded.
                        let pre = read_escrow_bounded(&lez_client, &hashlock).await.escrow;
                        let pre_state = pre.as_ref().map(|e| e.state);
                        let pre_amount = pre.as_ref().map_or(escrow.amount, |e| e.amount);
                        let pre_timelock = pre.as_ref().map_or(escrow.timelock, |e| e.timelock);
                        let fresh_balance =
                            max_escrow_balance(&lez_client, &pda, pre_amount).await;
                        // FINAL state read AFTER the balance observation.
                        let post = read_escrow_bounded(&lez_client, &hashlock).await.escrow;
                        let post_state = post.as_ref().map(|e| e.state);
                        let fresh_amount = post.as_ref().map_or(pre_amount, |e| e.amount);
                        let fresh_timelock = post.as_ref().map_or(pre_timelock, |e| e.timelock);
                        match classify_wedge_reread(
                            pre_state,
                            fresh_balance,
                            post_state,
                            now_unix() * 1000,
                            fresh_timelock,
                            fresh_amount,
                        ) {
                            WedgeDisposition::Quarantine => {
                                let fresh_balance_val = fresh_balance.unwrap_or(balance);
                                error!(
                                    hashlock = %entry.hashlock,
                                    balance = fresh_balance_val,
                                    amount = fresh_amount,
                                    "reconcile: partial-lock wedge (funding never landed) — QUARANTINING; \
                                     escrow can never be refunded, hashlock/secret is permanently unusable \
                                     and must NOT be reused"
                                );
                                store.quarantine(
                                    &entry.hashlock,
                                    format!(
                                        "partial lock: escrow Locked with balance {fresh_balance_val} < \
                                         amount {fresh_amount} (funding transfer never landed); refund \
                                         cannot terminalize it. Hashlock permanently unusable — do not \
                                         reuse the secret."
                                    ),
                                );
                            }
                            WedgeDisposition::RecoverClaimed => {
                                // Taker claimed after our stale read (possibly in
                                // the state→balance window) — the preimage is
                                // on-chain; recover the ETH instead of eating it.
                                info!(
                                    hashlock = %entry.hashlock,
                                    "reconcile: taker claimed after the wedge candidate read — claiming ETH"
                                );
                                let claimed = post
                                    .filter(|e| e.state == HTLCState::Claimed)
                                    .or_else(|| {
                                        pre.filter(|e| e.state == HTLCState::Claimed)
                                    })
                                    .expect("RecoverClaimed implies a Claimed escrow read");
                                recover_claimed_escrow(config, &entry, &claimed, store).await;
                            }
                            WedgeDisposition::Dropped => {
                                info!(
                                    hashlock = %entry.hashlock,
                                    "reconcile: escrow refunded on fresh re-read — dropping"
                                );
                                store.remove(&entry.hashlock);
                            }
                            WedgeDisposition::Retain => {
                                warn!(
                                    hashlock = %entry.hashlock,
                                    "reconcile: partial-lock wedge NOT reconfirmed on fresh paired \
                                     re-read — retaining entry for next reconcile"
                                );
                            }
                        }
                        continue;
                    }
                    // Escrow timelock is stored in milliseconds.
                    if now_unix() * 1000 >= escrow.timelock {
                        info!(hashlock = %entry.hashlock, "reconcile: escrow expired, refunding LEZ");
                        // Confirm a terminal state before dropping: a taker claim
                        // can win the race, in which case we claim the ETH side
                        // instead (P1-7).
                        match lez_client.refund_confirmed(&hashlock).await {
                            Ok(RefundOutcome::Refunded(tx)) => {
                                if !tx.is_empty() {
                                    info!(%tx, "reconcile: LEZ refunded");
                                }
                                store.remove(&entry.hashlock);
                            }
                            Ok(RefundOutcome::ClaimedByTaker(preimage)) => {
                                info!(hashlock = %entry.hashlock, "reconcile: taker claimed during refund race, claiming ETH");
                                let outcome = claim_eth(config, &entry.swap_id, preimage).await;
                                if !retain_after_eth_claim(outcome) {
                                    store.remove(&entry.hashlock);
                                }
                            }
                            Err(e) => {
                                // Keep the entry: retry on next restart.
                                warn!(hashlock = %entry.hashlock, "reconcile: refund not confirmed, keeping: {e}");
                            }
                        }
                    } else {
                        // Still live — resume it in the background.
                        resume_handles.push(tokio::spawn(resume_swap(
                            config.clone(),
                            entry.clone(),
                            hashlock,
                            escrow.timelock,
                            store.clone(),
                        )));
                    }
                }
                }
            }
            None => {
                // Every read was absent/errored after bounded retries. What this
                // means depends on the journal STAGE (P2-2) and on whether the
                // absence was AUTHORITATIVE (round 6): an errored read means the
                // sequencer may have been unreachable, so an all-None result is
                // an outage artifact, never grounds to retire.
                match classify_absent_escrow(entry.stage, bounded_read.any_err) {
                    AbsentEscrowAction::RetirePreLock => {
                    // The entry was written just BEFORE the lock, and every read
                    // was an authoritative `Ok(None)`: the sequencer answered and
                    // reported no escrow account, so the lock most plausibly
                    // failed pre-commit (RPC failure/rejection). A `Funded`-style
                    // retain would keep an all-None entry forever and wedge
                    // sequential startup on every restart. RETIRE it. Per the
                    // no-secret-reuse rule it is tombstoned into quarantine (not
                    // silently deleted) so the operator can see the hashlock must
                    // not be reused.
                    warn!(
                        hashlock = %entry.hashlock,
                        "reconcile: PreLock entry — no escrow observed on the sequencer ({n} \
                         authoritative absent reads, no errors); retiring (pre-lock-abandoned). \
                         Do NOT reuse the hashlock/secret.",
                        n = bounded_read.absent_reads,
                    );
                    store.quarantine(
                        &entry.hashlock,
                        format!(
                            "pre-lock-abandoned: journaled PreLock but no escrow observed on the \
                             sequencer ({} authoritative absent reads, no read errors); hashlock \
                             retired and must NOT be reused.",
                            bounded_read.absent_reads
                        ),
                    );
                    }
                    AbsentEscrowAction::RetainInconclusive => {
                    // Round-6 P2: at least one read ERRORED, so absence is not
                    // authoritative — this may be a sequencer outage while a
                    // committed lock sits on-chain (crash between the lock and
                    // the `mark_funded` journal upgrade). Retiring here would
                    // leave that funded escrow unwatched: the taker claims LEZ
                    // (revealing the preimage) and the maker never claims the
                    // ETH. Retain and retry; a genuinely never-locked entry
                    // still retires once the sequencer is reachable and
                    // authoritatively reports absence.
                    warn!(
                        hashlock = %entry.hashlock,
                        stage = ?entry.stage,
                        "reconcile: escrow read absent but at least one read errored (LEZ \
                         outage?) — absence not authoritative; retaining for next reconcile"
                    );
                    }
                    AbsentEscrowAction::RetainFunded => {
                    // Funded stage: an escrow DID exist on-chain. An all-None read
                    // is ambiguous, NOT conclusive absence (P1-2 / principle i):
                    // `get_escrow` returns `None` for missing/short/phantom data
                    // and the LEZ HTLC program never deletes an account. Dropping
                    // now would strand locked LEZ if the escrow read phantom/short.
                    // Retain and retry on the next reconcile.
                    warn!(
                        hashlock = %entry.hashlock,
                        "reconcile: Funded escrow read absent/ambiguous after retries — retaining for next reconcile"
                    );
                    }
                }
            }
        }
    }

    resume_handles
}

// ---------------------------------------------------------------------------
// Heartbeat offer publisher (Node.js @waku/sdk lightpush sidecar)
// ---------------------------------------------------------------------------

/// Environment passed to the publisher sidecar. The sidecar recomputes fresh
/// absolute timelocks from the minute durations on every heartbeat.
pub struct OfferPublisherEnv {
    pub lez_amount: u128,
    pub eth_amount_wei: u128,
    pub maker_eth_address: String,
    pub maker_lez_account: String,
    pub lez_timelock_minutes: u64,
    pub eth_timelock_minutes: u64,
    pub lez_htlc_program_id_hex: String,
    pub eth_htlc_address: String,
}

impl OfferPublisherEnv {
    pub fn from_config(config: &SwapConfig, lez_minutes: u64, eth_minutes: u64) -> Self {
        let maker_lez_account = match &config.lez_auth {
            crate::config::LezAuth::Wallet { account_id, .. } => account_id_to_base58(account_id),
            crate::config::LezAuth::RawKey(_) => String::new(), // filled by caller via client
        };
        Self {
            lez_amount: config.lez_amount,
            eth_amount_wei: config.eth_amount,
            maker_eth_address: config.eth_recipient_address.to_string(),
            maker_lez_account,
            lez_timelock_minutes: lez_minutes,
            eth_timelock_minutes: eth_minutes,
            lez_htlc_program_id_hex: program_id_to_hex(&config.lez_htlc_program_id),
            eth_htlc_address: config.eth_htlc_address.to_string(),
        }
    }

    fn to_env(&self, heartbeat_secs: u64) -> Vec<(String, String)> {
        vec![
            ("OFFER_LEZ_AMOUNT".into(), self.lez_amount.to_string()),
            (
                "OFFER_ETH_AMOUNT_WEI".into(),
                self.eth_amount_wei.to_string(),
            ),
            (
                "OFFER_MAKER_ETH_ADDRESS".into(),
                self.maker_eth_address.clone(),
            ),
            (
                "OFFER_MAKER_LEZ_ACCOUNT".into(),
                self.maker_lez_account.clone(),
            ),
            (
                "OFFER_LEZ_TIMELOCK_MINUTES".into(),
                self.lez_timelock_minutes.to_string(),
            ),
            (
                "OFFER_ETH_TIMELOCK_MINUTES".into(),
                self.eth_timelock_minutes.to_string(),
            ),
            (
                "OFFER_LEZ_HTLC_PROGRAM_ID".into(),
                self.lez_htlc_program_id_hex.clone(),
            ),
            (
                "OFFER_ETH_HTLC_ADDRESS".into(),
                self.eth_htlc_address.clone(),
            ),
            ("OFFER_HEARTBEAT_SECS".into(), heartbeat_secs.to_string()),
        ]
    }
}

/// Spawn and supervise the long-lived Node.js lightpush sidecar. The child
/// connects to the Waku fleet once and republishes the offer every
/// `heartbeat_secs`. If it exits (crash, network loss) it is restarted with a
/// 30s backoff until `cancel` is set. The offer heartbeat is best-effort: its
/// failure never affects the swap loop (which coordinates purely on-chain).
/// Resolve once the cancel flag is set (polled). Used to race a child process
/// wait against a graceful-shutdown request.
async fn wait_for_cancel(cancel: &AtomicBool) {
    loop {
        if cancel.load(Ordering::Relaxed) {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    }
}

pub fn spawn_offer_publisher(
    script: PathBuf,
    env: OfferPublisherEnv,
    heartbeat_secs: u64,
    cancel: Arc<AtomicBool>,
) -> tokio::task::JoinHandle<()> {
    let env_vars = env.to_env(heartbeat_secs);
    tokio::spawn(async move {
        loop {
            if cancel.load(Ordering::Relaxed) {
                return;
            }
            info!(
                "offer publisher: starting {} (heartbeat {heartbeat_secs}s)",
                script.display()
            );
            let mut cmd = tokio::process::Command::new("node");
            cmd.arg(&script)
                .envs(env_vars.iter().cloned())
                .kill_on_drop(true);
            match cmd.spawn() {
                Ok(mut child) => {
                    // Race the child against cancellation. On SIGTERM/stop we must
                    // KILL and REAP the child immediately — otherwise the Node
                    // sidecar is orphaned and keeps advertising an offline maker
                    // (relying on kill_on_drop alone can race process teardown).
                    tokio::select! {
                        status = child.wait() => match status {
                            Ok(status) => {
                                warn!("offer publisher exited ({status}); restarting in 30s")
                            }
                            Err(e) => warn!("offer publisher wait failed: {e}; restarting in 30s"),
                        },
                        _ = wait_for_cancel(&cancel) => {
                            info!("offer publisher: cancellation received — killing sidecar");
                            let _ = child.kill().await;
                            let _ = child.wait().await;
                            return;
                        }
                    }
                }
                Err(e) => warn!(
                    "offer publisher spawn failed ({}): {e}; retrying in 30s \
                     (is node installed and `npm install` run in offer-publisher?)",
                    script.display()
                ),
            }
            for _ in 0..30 {
                if cancel.load(Ordering::Relaxed) {
                    return;
                }
                tokio::time::sleep(std::time::Duration::from_secs(1)).await;
            }
        }
    })
}

// ---------------------------------------------------------------------------
// Pinata faucet sidecar (--fund-to)
// ---------------------------------------------------------------------------

/// Loop `wallet pinata claim --to <maker>` until the maker LEZ balance reaches
/// `target`. Requires wallet-mode auth (LEZ_WALLET_HOME + LEZ_ACCOUNT_ID) —
/// the pinata faucet is driven through the standalone `wallet` binary.
/// Returns the final balance.
pub async fn fund_to_target(
    config: &SwapConfig,
    wallet_bin: &str,
    target: u128,
    json: bool,
) -> Result<u128> {
    let (home, account_id) = match &config.lez_auth {
        crate::config::LezAuth::Wallet { home, account_id } => (home.clone(), *account_id),
        crate::config::LezAuth::RawKey(_) => {
            return Err(SwapError::InvalidConfig(
                "--fund-to requires wallet-mode auth (LEZ_WALLET_HOME + LEZ_ACCOUNT_ID): \
                 pinata claims go through the `wallet` binary"
                    .into(),
            ));
        }
    };
    let account_b58 = account_id_to_base58(&account_id);
    let lez_client = LezClient::new(config)?;

    let mut consecutive_failures: u32 = 0;
    let mut claims: u64 = 0;
    loop {
        let balance = lez_client.get_balance(&account_id).await?;
        if balance >= target {
            if !json {
                println!(
                    "Funding target reached: balance {balance} >= {target} ({claims} claim(s))"
                );
            }
            return Ok(balance);
        }
        let needed = target - balance;
        if !json {
            println!(
                "Balance {balance} < target {target} (need {needed}); claiming 150 LEZ from pinata..."
            );
        }
        let output = tokio::process::Command::new(wallet_bin)
            .env("LEE_WALLET_HOME_DIR", &home)
            .args(["pinata", "claim", "--to", &account_b58])
            .output()
            .await
            .map_err(|e| {
                SwapError::InvalidConfig(format!(
                    "failed to run `{wallet_bin} pinata claim` (set LEZ_WALLET_BIN?): {e}"
                ))
            })?;
        if output.status.success() {
            consecutive_failures = 0;
            claims += 1;
        } else {
            consecutive_failures += 1;
            warn!(
                "pinata claim failed ({}): {}",
                output.status,
                String::from_utf8_lossy(&output.stderr).trim()
            );
            if consecutive_failures >= 5 {
                return Err(SwapError::InvalidConfig(
                    "5 consecutive pinata claim failures — aborting funding loop".into(),
                ));
            }
        }
        // Let the claim land before re-checking the balance.
        tokio::time::sleep(std::time::Duration::from_secs(3)).await;
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::parse_program_id;

    #[test]
    fn timelock_guard_accepts_safe_margins() {
        assert!(validate_timelocks(20, 40, 5).is_ok());
        // The shipped GUI defaults (LEZ 5 / ETH 15 / margin 5): 15 >= 5+5+2.
        assert!(validate_timelocks(5, 15, 5).is_ok());
        // Exactly on the strict boundary (margin + transit slack) passes.
        assert!(validate_timelocks(5, 12, 5).is_ok());
    }

    #[test]
    fn timelock_guard_rejects_inverted_or_tight() {
        // Inverted: LEZ longer than ETH.
        assert!(validate_timelocks(40, 20, 5).is_err());
        // Equal — no margin at all.
        assert!(validate_timelocks(20, 20, 5).is_err());
        // Margin too small even if requested.
        assert!(validate_timelocks(20, 40, 2).is_err());
        // Delta smaller than requested margin.
        assert!(validate_timelocks(20, 24, 5).is_err());
    }

    // The old shipped defaults (LEZ 5 / ETH 10 / margin 5) sit exactly on the
    // margin boundary: they used to pass startup validation but the runtime
    // gate — which re-anchors the LEZ expiry at match time — then rejected
    // every taker lock by the tx-transit delta, silently wedging the maker
    // loop at WaitingForEthLock (live-repro'd 2026-08-03). The strict guard
    // must fail fast on any config without transit slack.
    #[test]
    fn timelock_guard_rejects_margin_boundary_without_transit_slack() {
        // The exact old-default wedge case.
        assert!(validate_timelocks(5, 10, 5).is_err());
        // One minute of slack is still short of the required two.
        assert!(validate_timelocks(5, 11, 5).is_err());
        // Same boundary at loop-scale values.
        assert!(validate_timelocks(20, 25, 5).is_err());
    }

    #[test]
    fn program_id_hex_roundtrips() {
        let hex_in = "0102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f20";
        let id = parse_program_id(hex_in).unwrap();
        assert_eq!(program_id_to_hex(&id), hex_in);
    }

    #[test]
    fn state_store_roundtrips_and_persists() {
        let path =
            std::env::temp_dir().join(format!("maker-state-test-{}.json", std::process::id()));
        let _ = std::fs::remove_file(&path);

        let store = StateStore::load(&path).unwrap();
        assert!(store.snapshot().is_empty());

        store
            .record(InFlightSwap {
                hashlock: "ab".repeat(32),
                swap_id: format!("0x{}", "cd".repeat(32)),
                recorded_at: 123,
                stage: SwapStage::PreLock,
            })
            .unwrap();
        // Idempotent on hashlock.
        store
            .record(InFlightSwap {
                hashlock: "ab".repeat(32),
                swap_id: format!("0x{}", "cd".repeat(32)),
                recorded_at: 456,
                stage: SwapStage::PreLock,
            })
            .unwrap();
        assert_eq!(store.snapshot().len(), 1);

        // Reload from disk — entry survives a "crash".
        let store2 = StateStore::load(&path).unwrap();
        assert_eq!(store2.snapshot().len(), 1);
        assert_eq!(store2.snapshot()[0].recorded_at, 123);

        store2.remove(&"ab".repeat(32));
        assert!(store2.snapshot().is_empty());
        let store3 = StateStore::load(&path).unwrap();
        assert!(store3.snapshot().is_empty());

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn corrupt_state_file_is_an_error_not_a_wipe() {
        let path =
            std::env::temp_dir().join(format!("maker-state-corrupt-{}.json", std::process::id()));
        std::fs::write(&path, "{not json").unwrap();
        assert!(StateStore::load(&path).is_err());
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn eth_claim_outcome_retain_semantics() {
        // Only a transient failure retains the journal entry (P1-6): a
        // confirmed claim or an already-terminal HTLC drops it, a failure keeps
        // it so a later reconcile retries before the taker's refund deadline.
        assert!(retain_after_eth_claim(EthClaimOutcome::Failed));
        assert!(!retain_after_eth_claim(EthClaimOutcome::Claimed));
        assert!(!retain_after_eth_claim(EthClaimOutcome::AlreadyTerminal));
    }

    #[test]
    fn durable_record_survives_reload_and_is_idempotent() {
        // P1-4 / P1-5: the pre-lock record must be durable (fsync'd) so a crash
        // between locking LEZ and any later mutation still leaves the entry for
        // reconcile — and it must not be dropped merely because a swap failed.
        let path =
            std::env::temp_dir().join(format!("maker-record-test-{}.json", std::process::id()));
        let _ = std::fs::remove_file(&path);

        let store = StateStore::load(&path).unwrap();
        let swap = InFlightSwap {
            hashlock: "ab".repeat(32),
            swap_id: format!("0x{}", "cd".repeat(32)),
            recorded_at: 7,
            stage: SwapStage::PreLock,
        };
        store.record(swap.clone()).expect("durable record");
        // Idempotent on hashlock.
        store.record(swap).expect("idempotent record");
        assert_eq!(store.snapshot().len(), 1);

        // Simulate a crash: reload from disk. Entry must still be present
        // (retained across the "crash" — not silently lost).
        let reloaded = StateStore::load(&path).unwrap();
        assert_eq!(reloaded.snapshot().len(), 1);
        assert_eq!(reloaded.snapshot()[0].recorded_at, 7);

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn journal_contains_reports_in_flight_hashlocks() {
        // P1-A belt: the loop skips a hashlock the journal already holds, so
        // `contains` must reflect recorded/removed entries exactly.
        let path =
            std::env::temp_dir().join(format!("maker-contains-test-{}.json", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let store = StateStore::load(&path).unwrap();
        let hl = "ab".repeat(32);
        assert!(!store.contains(&hl));
        store
            .record(InFlightSwap {
                hashlock: hl.clone(),
                swap_id: format!("0x{}", "cd".repeat(32)),
                recorded_at: 1,
                stage: SwapStage::PreLock,
            })
            .unwrap();
        assert!(store.contains(&hl));
        assert!(!store.contains(&"ff".repeat(32)));
        store.remove(&hl);
        assert!(!store.contains(&hl));
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn hashlock_and_swap_id_parse() {
        assert!(parse_hashlock_hex(&"ab".repeat(32)).is_some());
        assert!(parse_hashlock_hex("zz").is_none());
        assert!(parse_hashlock_hex("abcd").is_none());
        assert!(parse_swap_id(&format!("0x{}", "cd".repeat(32))).is_some());
        assert!(parse_swap_id("nope").is_none());
    }

    fn test_escrow(state: HTLCState, amount: u128) -> HTLCEscrow {
        use lee_core::program::PdaSeed;
        let dummy = lee::AccountId::for_public_pda(&[1u32; 8], &PdaSeed::new([0u8; 32]));
        HTLCEscrow {
            hashlock: [0xABu8; 32],
            maker_id: dummy,
            taker_id: dummy,
            amount,
            state,
            timelock: 1_000_000,
            preimage: None,
        }
    }

    // P1-2 (principle i): a single `Ok(None)` escrow read is ambiguous — the
    // LEZ HTLC program never deletes accounts, so absence can be a
    // phantom/short read. The bounded reader must retry, and if EVERY read is
    // ambiguous return `None` so the caller RETAINS the journal entry.
    #[tokio::test(start_paused = true)]
    async fn ok_none_retries_then_retains() {
        use std::sync::atomic::{AtomicU32, Ordering};

        // All reads ambiguous → None (caller retains), after exactly 5 attempts.
        let calls = AtomicU32::new(0);
        let result = read_escrow_bounded_with(
            || {
                calls.fetch_add(1, Ordering::Relaxed);
                async { Ok(None) }
            },
            std::time::Duration::from_millis(10),
            &[0xABu8; 32],
        )
        .await;
        assert!(
            result.escrow.is_none(),
            "all-ambiguous reads must return None (retain)"
        );
        assert!(
            !result.any_err,
            "all-`Ok(None)` reads carry no error taint — absence is authoritative"
        );
        assert_eq!(
            result.absent_reads, 5,
            "every authoritative absent read is counted"
        );
        assert_eq!(
            calls.load(Ordering::Relaxed),
            5,
            "must retry the bounded max"
        );

        // Two ambiguous reads (one None, one Err) then a conclusive escrow →
        // the escrow is returned; a transient blip never masks a live escrow.
        let calls = AtomicU32::new(0);
        let result = read_escrow_bounded_with(
            || {
                let n = calls.fetch_add(1, Ordering::Relaxed);
                async move {
                    match n {
                        0 => Ok(None),
                        1 => Err(SwapError::LezSequencer("transient".into())),
                        _ => Ok(Some(test_escrow(HTLCState::Locked, 150))),
                    }
                }
            },
            std::time::Duration::from_millis(10),
            &[0xABu8; 32],
        )
        .await;
        assert!(
            result.escrow.is_some(),
            "a conclusive read after retries must win"
        );
        assert_eq!(calls.load(Ordering::Relaxed), 3);
    }

    // Round-6 P2: the bounded reader must distinguish an RPC outage (`Err`)
    // from authoritative absence (`Ok(None)`) — an all-errored or mixed result
    // carries `any_err = true` so the caller RETAINS a PreLock entry instead of
    // retiring it while a committed lock may sit unwatched on-chain.
    #[tokio::test(start_paused = true)]
    async fn all_errored_reads_are_not_authoritative_absence() {
        use std::sync::atomic::{AtomicU32, Ordering};

        // (a) EVERY read errors (sequencer unreachable) → escrow None, but
        // `any_err` is set: absence is an outage artifact, PreLock is RETAINED.
        let calls = AtomicU32::new(0);
        let result = read_escrow_bounded_with(
            || {
                calls.fetch_add(1, Ordering::Relaxed);
                async { Err(SwapError::LezSequencer("outage".into())) }
            },
            std::time::Duration::from_millis(10),
            &[0xABu8; 32],
        )
        .await;
        assert!(result.escrow.is_none());
        assert!(result.any_err, "errored reads must taint the result");
        assert_eq!(result.absent_reads, 0, "no authoritative absent read seen");
        assert_eq!(calls.load(Ordering::Relaxed), 5);
        assert_eq!(
            classify_absent_escrow(SwapStage::PreLock, result.any_err),
            AbsentEscrowAction::RetainInconclusive,
            "all-errored PreLock must be retained, never retired"
        );

        // (b) mixed Err + Ok(None) → still tainted; PreLock is RETAINED.
        let calls = AtomicU32::new(0);
        let result = read_escrow_bounded_with(
            || {
                let n = calls.fetch_add(1, Ordering::Relaxed);
                async move {
                    if n % 2 == 0 {
                        Err(SwapError::LezSequencer("transient".into()))
                    } else {
                        Ok(None)
                    }
                }
            },
            std::time::Duration::from_millis(10),
            &[0xABu8; 32],
        )
        .await;
        assert!(result.escrow.is_none());
        assert!(
            result.any_err,
            "one errored read is enough to make absence inconclusive"
        );
        assert_eq!(result.absent_reads, 2);
        assert_eq!(
            classify_absent_escrow(SwapStage::PreLock, result.any_err),
            AbsentEscrowAction::RetainInconclusive,
            "mixed err+absent PreLock must be retained, never retired"
        );
    }

    // Round-6 P2 (wedge fix preserved): when EVERY read is an authoritative
    // `Ok(None)` — the sequencer answered and reported no account — a PreLock
    // entry IS retired, so a genuinely never-locked entry cannot wedge
    // sequential startup forever.
    #[tokio::test(start_paused = true)]
    async fn all_authoritative_absent_prelock_is_retired() {
        let result = read_escrow_bounded_with(
            || async { Ok(None) },
            std::time::Duration::from_millis(10),
            &[0xABu8; 32],
        )
        .await;
        assert!(result.escrow.is_none());
        assert!(!result.any_err);
        assert_eq!(result.absent_reads, 5);
        assert_eq!(
            classify_absent_escrow(SwapStage::PreLock, result.any_err),
            AbsentEscrowAction::RetirePreLock,
            "authoritative all-absent PreLock retires — the startup-wedge fix is preserved"
        );
        // A Funded entry stays retained even on authoritative absence (phantom
        // reads; the program never deletes accounts).
        assert_eq!(
            classify_absent_escrow(SwapStage::Funded, result.any_err),
            AbsentEscrowAction::RetainFunded
        );
    }

    // P1-4: the partial-lock wedge predicate — an EXPIRED Locked escrow with
    // balance below the locked amount can never be refunded (the program
    // requires balance >= amount), so it is conclusively unrecoverable.
    #[test]
    fn partial_lock_wedge_predicate() {
        let (before, expiry, after) = (999_999u64, 1_000_000u64, 1_000_001u64);
        // The wedge: expired, Lock committed, funding never landed.
        assert!(is_partial_lock_wedge(
            HTLCState::Locked,
            after,
            expiry,
            0,
            150
        ));
        assert!(is_partial_lock_wedge(
            HTLCState::Locked,
            expiry,
            expiry,
            149,
            150
        ));
        // Unexpired + underfunded is AMBIGUOUS (funding may still be in
        // flight after a fast restart) — not the wedge; it resumes as live.
        assert!(!is_partial_lock_wedge(
            HTLCState::Locked,
            before,
            expiry,
            0,
            150
        ));
        // Fully funded — healthy escrow, never a wedge.
        assert!(!is_partial_lock_wedge(
            HTLCState::Locked,
            after,
            expiry,
            150,
            150
        ));
        assert!(!is_partial_lock_wedge(
            HTLCState::Locked,
            after,
            expiry,
            151,
            150
        ));
        // Terminal states are never the wedge regardless of balance/expiry.
        assert!(!is_partial_lock_wedge(
            HTLCState::Claimed,
            after,
            expiry,
            0,
            150
        ));
        assert!(!is_partial_lock_wedge(
            HTLCState::Refunded,
            after,
            expiry,
            0,
            150
        ));
    }

    // P1-1: a suspected wedge (Locked + low balance at the initial read) must
    // NOT be quarantined if a taker `Claim` landed AFTER that stale read. The
    // fresh paired re-read decides: a `Claimed` state routes to the ETH-claim
    // recovery path, NEVER to the irreversible quarantine.
    #[test]
    fn classify_wedge_reread_recovers_taker_claim_not_quarantine() {
        let (expiry, after) = (1_000_000u64, 1_000_001u64);
        // Post-balance read shows the taker claimed after our stale Locked read,
        // even though the balance still reads low → recover ETH, not quarantine.
        assert_eq!(
            classify_wedge_reread(
                Some(HTLCState::Locked),
                Some(0),
                Some(HTLCState::Claimed),
                after,
                expiry,
                150
            ),
            WedgeDisposition::RecoverClaimed,
            "a taker claim after the stale read must route to ETH recovery, not quarantine"
        );
        // Both pre- and post-balance reads Locked + still underfunded + expired →
        // genuine wedge, quarantine.
        assert_eq!(
            classify_wedge_reread(
                Some(HTLCState::Locked),
                Some(0),
                Some(HTLCState::Locked),
                after,
                expiry,
                150
            ),
            WedgeDisposition::Quarantine
        );
        // Fresh read shows a now-sufficient balance → not wedged, retain/retry.
        assert_eq!(
            classify_wedge_reread(
                Some(HTLCState::Locked),
                Some(150),
                Some(HTLCState::Locked),
                after,
                expiry,
                150
            ),
            WedgeDisposition::Retain
        );
        // Final read shows Refunded → already terminal, drop.
        assert_eq!(
            classify_wedge_reread(
                Some(HTLCState::Locked),
                Some(0),
                Some(HTLCState::Refunded),
                after,
                expiry,
                150
            ),
            WedgeDisposition::Dropped
        );
        // Final escrow read absent/phantom → not conclusive, retain.
        assert_eq!(
            classify_wedge_reread(Some(HTLCState::Locked), Some(0), None, after, expiry, 150),
            WedgeDisposition::Retain
        );
    }

    // P1-1: enforce the read ORDER state → balance → STATE. A taker `Claim` can
    // commit in the window between the pre-balance state read and the balance
    // read, draining the escrow to zero. A cached `Locked` pre-state paired with
    // a fresh zero balance must therefore NEVER quarantine on its own — the FINAL
    // (post-balance) state read is authoritative. Quarantine requires BOTH state
    // reads to still be `Locked`.
    #[test]
    fn classify_wedge_reread_state_balance_state_ordering() {
        let (expiry, after) = (1_000_000u64, 1_000_001u64);
        // The exact race: pre-state Locked, balance drained to zero, then the
        // FINAL state read reveals the claim → recover, never quarantine.
        assert_eq!(
            classify_wedge_reread(
                Some(HTLCState::Locked),
                Some(0),
                Some(HTLCState::Claimed),
                after,
                expiry,
                150
            ),
            WedgeDisposition::RecoverClaimed,
            "claim revealed at the FINAL state read must recover ETH, not quarantine"
        );
        // A claim already visible at the PRE read (before balance) also recovers,
        // whatever the later balance/state read shows.
        assert_eq!(
            classify_wedge_reread(
                Some(HTLCState::Claimed),
                Some(0),
                Some(HTLCState::Locked),
                after,
                expiry,
                150
            ),
            WedgeDisposition::RecoverClaimed,
            "a claim seen at the pre-balance read must recover ETH"
        );
        // Pre-state Locked but the FINAL read is absent/phantom → not both-Locked,
        // so NOT conclusive; retain rather than quarantine on the stale pre-read.
        assert_eq!(
            classify_wedge_reread(Some(HTLCState::Locked), Some(0), None, after, expiry, 150),
            WedgeDisposition::Retain,
            "an absent FINAL state read must not quarantine on the stale pre-read"
        );
        // Pre-state absent but the FINAL read Locked + underfunded → still not
        // both-Locked; retain.
        assert_eq!(
            classify_wedge_reread(None, Some(0), Some(HTLCState::Locked), after, expiry, 150),
            WedgeDisposition::Retain,
            "an absent pre-balance state read must not quarantine"
        );
        // Only when BOTH reads are Locked and the escrow is conclusively
        // underfunded at/after expiry is the wedge genuine → quarantine.
        assert_eq!(
            classify_wedge_reread(
                Some(HTLCState::Locked),
                Some(0),
                Some(HTLCState::Locked),
                after,
                expiry,
                150
            ),
            WedgeDisposition::Quarantine,
            "both-Locked + conclusively-underfunded + expired is the genuine wedge"
        );
    }

    // P1-2: when EVERY balance read on the fresh paired re-read failed (an
    // outage, `None`), a still-Locked escrow must be RETAINED, not quarantined —
    // all-failed is indistinguishable from observed-zero and quarantine is
    // irreversible, so the recoverable escrow is kept for the next reconcile.
    #[test]
    fn classify_wedge_reread_all_balance_reads_failed_retains() {
        let (expiry, after) = (1_000_000u64, 1_000_001u64);
        assert_eq!(
            classify_wedge_reread(
                Some(HTLCState::Locked),
                None,
                Some(HTLCState::Locked),
                after,
                expiry,
                150
            ),
            WedgeDisposition::Retain,
            "all-balance-reads-failed must retain a Locked escrow, never quarantine"
        );
    }

    // P2-2 / round 6: an all-absent escrow read is retired vs retained by
    // journal stage AND conclusiveness — a PreLock entry retires only on
    // authoritative absence (no errored reads); any errored read retains
    // regardless of stage; a Funded entry is always retained.
    #[test]
    fn classify_absent_escrow_by_stage() {
        assert_eq!(
            classify_absent_escrow(SwapStage::PreLock, false),
            AbsentEscrowAction::RetirePreLock,
            "a PreLock entry with authoritatively-absent escrow was never locked — retire it"
        );
        assert_eq!(
            classify_absent_escrow(SwapStage::Funded, false),
            AbsentEscrowAction::RetainFunded,
            "a Funded entry that reads absent is an ambiguous phantom read — retain it"
        );
        assert_eq!(
            classify_absent_escrow(SwapStage::PreLock, true),
            AbsentEscrowAction::RetainInconclusive,
            "an errored read makes absence outage-ambiguous — retain even a PreLock entry"
        );
        assert_eq!(
            classify_absent_escrow(SwapStage::Funded, true),
            AbsentEscrowAction::RetainInconclusive,
        );
    }

    // P2-2: mark_funded upgrades a PreLock entry in place and persists across a
    // reload; the reconcile-side effect of the RetirePreLock path (tombstone into
    // quarantine) is exercised end-to-end on the store.
    #[test]
    fn prelock_marked_funded_then_survives_reload() {
        let path = std::env::temp_dir()
            .join(format!("maker-prelock-funded-{}.json", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let store = StateStore::load(&path).unwrap();
        let hl = "ab".repeat(32);
        store
            .record(InFlightSwap {
                hashlock: hl.clone(),
                swap_id: format!("0x{}", "cd".repeat(32)),
                recorded_at: 1,
                stage: SwapStage::PreLock,
            })
            .unwrap();
        assert_eq!(store.snapshot()[0].stage, SwapStage::PreLock);
        // Post-lock upgrade.
        store.mark_funded(&hl);
        assert_eq!(store.snapshot()[0].stage, SwapStage::Funded);
        // Durable across a "crash".
        let reloaded = StateStore::load(&path).unwrap();
        assert_eq!(reloaded.snapshot()[0].stage, SwapStage::Funded);
        let _ = std::fs::remove_file(&path);
    }

    // P2-2: the RetirePreLock disposition tombstones (does NOT silently delete) a
    // PreLock entry whose lock never committed — it leaves in_flight (unwedging
    // startup) and lands in quarantine with a pre-lock-abandoned reason.
    #[test]
    fn prelock_retire_tombstones_into_quarantine() {
        let path = std::env::temp_dir()
            .join(format!("maker-prelock-retire-{}.json", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let store = StateStore::load(&path).unwrap();
        let hl = "ab".repeat(32);
        store
            .record(InFlightSwap {
                hashlock: hl.clone(),
                swap_id: format!("0x{}", "cd".repeat(32)),
                recorded_at: 1,
                stage: SwapStage::PreLock,
            })
            .unwrap();
        assert_eq!(classify_absent_escrow(store.snapshot()[0].stage, false), AbsentEscrowAction::RetirePreLock);
        // The retire path: tombstone into quarantine (never a silent delete).
        store.quarantine(&hl, "pre-lock-abandoned: no escrow ever appeared".into());
        assert!(!store.has_in_flight(), "retired entry leaves in_flight — startup unwedged");
        let q = store.quarantined_snapshot();
        assert_eq!(q.len(), 1);
        assert!(q[0].reason.contains("pre-lock-abandoned"));
        let _ = std::fs::remove_file(&path);
    }

    // P2-2 serde back-compat: a state file written before the `stage` field
    // existed loads with stage = Funded (its entries were only ever persisted
    // AFTER a confirmed lock), so it is retained on an absent read, not retired.
    #[test]
    fn legacy_state_file_without_stage_loads_as_funded() {
        let path = std::env::temp_dir()
            .join(format!("maker-legacy-stage-{}.json", std::process::id()));
        let legacy = serde_json::json!({
            "in_flight": [{
                "hashlock": "ab".repeat(32),
                "swap_id": format!("0x{}", "cd".repeat(32)),
                "recorded_at": 42u64
            }]
        });
        std::fs::write(&path, serde_json::to_string_pretty(&legacy).unwrap()).unwrap();
        let store = StateStore::load(&path).unwrap();
        let snap = store.snapshot();
        assert_eq!(snap.len(), 1);
        assert_eq!(snap[0].stage, SwapStage::Funded, "missing stage defaults to Funded");
        assert_eq!(
            classify_absent_escrow(snap[0].stage, false),
            AbsentEscrowAction::RetainFunded
        );
        let _ = std::fs::remove_file(&path);
    }

    // P1-2: the balance reader returns `None` only when EVERY read failed, and
    // `Some(max)` as soon as at least one read succeeds — so the caller can tell
    // an outage (unknown) apart from a genuinely observed zero balance.
    #[tokio::test(start_paused = true)]
    async fn max_escrow_balance_all_reads_failed_returns_none() {
        use std::sync::atomic::{AtomicU32, Ordering};

        // Every read errors → None (unknown), after exactly 5 attempts.
        let calls = AtomicU32::new(0);
        let result = max_escrow_balance_with(
            || {
                calls.fetch_add(1, Ordering::Relaxed);
                async { Err(SwapError::LezSequencer("outage".into())) }
            },
            std::time::Duration::from_millis(10),
            150,
        )
        .await;
        assert_eq!(result, None, "all-failed reads must return None (unknown)");
        assert_eq!(calls.load(Ordering::Relaxed), 5, "must retry the bounded max");

        // One successful zero read among failures → Some(0): a real observed
        // zero is distinct from an outage.
        let calls = AtomicU32::new(0);
        let result = max_escrow_balance_with(
            || {
                let n = calls.fetch_add(1, Ordering::Relaxed);
                async move {
                    if n == 2 {
                        Ok(0u128)
                    } else {
                        Err(SwapError::LezSequencer("transient".into()))
                    }
                }
            },
            std::time::Duration::from_millis(10),
            150,
        )
        .await;
        assert_eq!(result, Some(0), "one successful read yields Some, even if zero");

        // Short-circuits as soon as `enough` is reached (max over reads).
        let calls = AtomicU32::new(0);
        let result = max_escrow_balance_with(
            || {
                calls.fetch_add(1, Ordering::Relaxed);
                async { Ok(150u128) }
            },
            std::time::Duration::from_millis(10),
            150,
        )
        .await;
        assert_eq!(result, Some(150));
        assert_eq!(
            calls.load(Ordering::Relaxed),
            1,
            "reaching `enough` short-circuits further reads"
        );
    }

    // P1-4: quarantining a partial-lock entry moves it OUT of the active
    // journal (so startup no longer blocks on it and it is never retried) and
    // INTO the quarantined section with a reason — durably, across reload.
    #[test]
    fn partial_lock_quarantined_and_startup_proceeds() {
        let path =
            std::env::temp_dir().join(format!("maker-quarantine-test-{}.json", std::process::id()));
        let _ = std::fs::remove_file(&path);

        let store = StateStore::load(&path).unwrap();
        let hl = "ab".repeat(32);
        store
            .record(InFlightSwap {
                hashlock: hl.clone(),
                swap_id: format!("0x{}", "cd".repeat(32)),
                recorded_at: 1,
                stage: SwapStage::PreLock,
            })
            .unwrap();
        assert!(store.has_in_flight(), "recorded entry is in-flight");

        store.quarantine(&hl, "partial lock: funding never landed".into());

        // The active journal is empty → the P1-3 stop gate and the startup
        // "still unresolved" gate both see a clean journal and proceed.
        assert!(!store.has_in_flight());
        assert!(store.snapshot().is_empty());
        assert!(!store.contains(&hl));
        // ... but the entry is preserved in quarantine with its reason.
        let q = store.quarantined_snapshot();
        assert_eq!(q.len(), 1);
        assert_eq!(q[0].hashlock, hl);
        assert!(q[0].reason.contains("partial lock"));
        assert!(q[0].quarantined_at > 0);
        // Carried the swap id over from the in-flight entry.
        assert_eq!(q[0].swap_id, format!("0x{}", "cd".repeat(32)));

        // Survives a restart (reload from disk) and stays out of in_flight.
        let reloaded = StateStore::load(&path).unwrap();
        assert!(!reloaded.has_in_flight());
        assert_eq!(reloaded.quarantined_snapshot().len(), 1);

        // Idempotent: quarantining again does not duplicate.
        reloaded.quarantine(&hl, "again".into());
        assert_eq!(reloaded.quarantined_snapshot().len(), 1);

        let _ = std::fs::remove_file(&path);
    }

    // Older state files (no `quarantined` field) still load.
    #[test]
    fn state_file_without_quarantine_field_loads() {
        let path =
            std::env::temp_dir().join(format!("maker-oldstate-test-{}.json", std::process::id()));
        std::fs::write(&path, r#"{"in_flight":[]}"#).unwrap();
        let store = StateStore::load(&path).unwrap();
        assert!(store.quarantined_snapshot().is_empty());
        assert!(!store.has_in_flight());
        let _ = std::fs::remove_file(&path);
    }
}
