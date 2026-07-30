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

/// Validate the maker-safety timelock invariant before the first offer:
/// the ETH (taker, long) timelock must exceed the LEZ (maker, short) timelock
/// by at least `margin_minutes`. A margin below 5 minutes is rejected — the
/// EthHTLC contract enforces `minTimelockDelta = 300s` and the maker needs
/// room to observe the preimage and claim.
pub fn validate_timelocks(lez_minutes: u64, eth_minutes: u64, margin_minutes: u64) -> Result<()> {
    if margin_minutes < 5 {
        return Err(SwapError::InvalidConfig(format!(
            "timelock margin must be >= 5 minutes (got {margin_minutes}); \
             EthHTLC minTimelockDelta is 300s and the maker needs claim headroom"
        )));
    }
    if eth_minutes < lez_minutes + margin_minutes {
        return Err(SwapError::InvalidConfig(format!(
            "unsafe timelocks: ETH_TIMELOCK_MINUTES ({eth_minutes}) must be >= \
             LEZ_TIMELOCK_MINUTES ({lez_minutes}) + margin ({margin_minutes}). \
             The taker locks first with the LONG timelock; the maker locks second \
             with the SHORT one."
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
            },
        )
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

/// Re-read an escrow up to a bounded number of times, tolerating ambiguous
/// reads (P1-2, principle i). `get_escrow` returns `Ok(None)` for
/// missing/short/phantom data — which is NOT conclusive absence (the LEZ HTLC
/// program never deletes an account; refund leaves it `Refunded` with intact
/// data). A transient sequencer error is equally ambiguous. Returns:
/// - `Some(escrow)` on the first conclusive read of an existing escrow.
/// - `None` only when EVERY read was absent/errored — the caller must RETAIN the
///   journal entry, never drop it (a phantom/short read while the escrow exists
///   would otherwise strand locked LEZ). A never-locked entry that reads `None`
///   forever costs only a redundant idempotent reconcile each restart.
async fn read_escrow_bounded(lez: &LezClient, hashlock: &[u8; 32]) -> Option<HTLCEscrow> {
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
) -> Option<HTLCEscrow>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<Option<HTLCEscrow>>>,
{
    const MAX_READS: u32 = 5;
    for attempt in 1..=MAX_READS {
        match fetch().await {
            Ok(Some(escrow)) => return Some(escrow),
            Ok(None) => {
                debug!(
                    hashlock = %hex::encode(hashlock),
                    "reconcile: escrow read absent ({attempt}/{MAX_READS}) — ambiguous, retrying"
                );
            }
            Err(e) => {
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
    None
}

/// Highest escrow-PDA balance observed across bounded re-reads, short-circuiting
/// as soon as `enough` is reached. Used to judge the partial-lock wedge robustly:
/// a single phantom-zero balance read must never misclassify a funded escrow as
/// unfunded (which would wrongly quarantine live LEZ), so we take the MAX over
/// several reads — any read that reaches `enough` proves the escrow is funded.
async fn max_escrow_balance(lez: &LezClient, pda: &AccountId, enough: u128) -> u128 {
    const MAX_READS: u32 = 5;
    let mut max = 0u128;
    for attempt in 1..=MAX_READS {
        if let Ok(balance) = lez.get_balance(pda).await {
            max = max.max(balance);
            if max >= enough {
                return max;
            }
        }
        if attempt < MAX_READS {
            tokio::time::sleep(lez.poll_interval()).await;
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

        match read_escrow_bounded(&lez_client, &hashlock).await {
            Some(escrow) => match escrow.state {
                HTLCState::Claimed => {
                    // Taker revealed the preimage while we were down — collect the ETH.
                    match escrow.preimage.and_then(|p| <[u8; 32]>::try_from(p).ok()) {
                        Some(preimage) => {
                            info!(hashlock = %entry.hashlock, "reconcile: escrow claimed, claiming ETH");
                            let outcome = claim_eth(config, &entry.swap_id, preimage).await;
                            // Keep the entry on a transient ETH-claim failure so
                            // the next reconcile retries (P1-6).
                            if !retain_after_eth_claim(outcome) {
                                store.remove(&entry.hashlock);
                            }
                        }
                        None => {
                            // LEZ is terminally claimed but the preimage is
                            // missing/invalid — the ETH is unrecoverable and
                            // there is nothing to retry, so drop the entry.
                            error!(
                                hashlock = %entry.hashlock,
                                "reconcile: escrow claimed but preimage missing/invalid, dropping (ETH unrecoverable)"
                            );
                            store.remove(&entry.hashlock);
                        }
                    }
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
                    let balance = max_escrow_balance(&lez_client, &pda, escrow.amount).await;
                    if is_partial_lock_wedge(
                        escrow.state,
                        now_unix() * 1000,
                        escrow.timelock,
                        balance,
                        escrow.amount,
                    ) {
                        error!(
                            hashlock = %entry.hashlock,
                            balance,
                            amount = escrow.amount,
                            "reconcile: partial-lock wedge (funding never landed) — QUARANTINING; \
                             escrow can never be refunded, hashlock/secret is permanently unusable \
                             and must NOT be reused"
                        );
                        store.quarantine(
                            &entry.hashlock,
                            format!(
                                "partial lock: escrow Locked with balance {balance} < amount {} \
                                 (funding transfer never landed); refund cannot terminalize it. \
                                 Hashlock permanently unusable — do not reuse the secret.",
                                escrow.amount
                            ),
                        );
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
            },
            None => {
                // Every read was absent/errored — ambiguous, NOT conclusive
                // absence (P1-2 / principle i). `get_escrow` returns `None` for
                // missing/short/phantom data, and the LEZ HTLC program never
                // deletes an account. Dropping now would strand locked LEZ if the
                // escrow exists but read phantom/short; a never-locked entry costs
                // only a redundant idempotent reconcile next restart. Retain.
                warn!(
                    hashlock = %entry.hashlock,
                    "reconcile: escrow read absent/ambiguous after retries — retaining for next reconcile"
                );
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
        assert!(validate_timelocks(5, 10, 5).is_ok());
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
            })
            .unwrap();
        // Idempotent on hashlock.
        store
            .record(InFlightSwap {
                hashlock: "ab".repeat(32),
                swap_id: format!("0x{}", "cd".repeat(32)),
                recorded_at: 456,
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
            result.is_none(),
            "all-ambiguous reads must return None (retain)"
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
        assert!(result.is_some(), "a conclusive read after retries must win");
        assert_eq!(calls.load(Ordering::Relaxed), 3);
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
