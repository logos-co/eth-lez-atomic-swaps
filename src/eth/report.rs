//! Read-only, on-chain count of public-trial swap activity.
//!
//! ## Why the chain, and not the ops ledger
//!
//! [`crate::ops`] durably counts the swaps one maker's standing loop
//! **accepted**. That is the right source for *why* a swap failed (its bounded
//! [`crate::ops::FailureCode`] taxonomy has no on-chain equivalent), but it is
//! the wrong source for *how many people tried*:
//!
//! - it lives inside each maker container, so a fleet count needs operator
//!   access to every one of them and a manual sum;
//! - it is structurally blind to a taker who locked ETH and got **no
//!   counterparty at all** — nobody accepted, so nothing was ever recorded.
//!   During a public trial that gap is the most important signal there is.
//!
//! Every swap's ETH leg goes through the pinned Sepolia `EthHTLC`
//! ([`PINNED_ETH_HTLC_ADDRESS`]) — the taker locks ETH first — and the contract
//! emits `Locked` / `Claimed` / `Refunded`. Reading those logs gives swaps
//! attempted, completed, refunded, still-open, distinct taker wallets and a
//! per-maker split: globally, retroactively to deployment, publicly verifiable
//! by anyone, with no container access and no baseline snapshot taken in
//! advance.
//!
//! ## Privacy boundary
//!
//! The logs are public, but this module still aggregates: [`SwapCounts`]
//! reports distinct senders as a **count**, never as a list, and nothing here
//! writes a chain-derived address into the ops ledger's record shape. The only
//! addresses that ever reach the output are the `Locked.recipient` values — the
//! maker/venue addresses the per-maker split is explicitly about.
//!
//! ## One venue, one era
//!
//! `docs/testnet.md` records a superseded v1 deployment
//! ([`SUPERSEDED_ETH_HTLC_ADDRESS`]) whose `Locked` topic0 differs, because
//! `takerLezAccount` was added in v2. Querying the wrong one does not error —
//! an unmatched log filter legitimately returns an empty list — so a report run
//! against it would silently read zero. Two guards prevent that: the caller
//! runs [`crate::eth::client::verify_interface_version`] first (a v1
//! deployment has no such getter and fails loudly), and a non-pinned address
//! must carry its own explicit `--from-block` rather than inheriting the pinned
//! venue's deployment block.
//!
//! ## Proven, or not published
//!
//! The endpoint this runs against load-balances across nodes with different log
//! retention, and a node without the index answers `eth_getLogs` with an empty
//! list rather than an error (see [`Canary`]). So every response is batched
//! with anchor queries that prove the backend holds the scanned range, and a
//! scan that could not be anchored at both ends is refused
//! ([`ScanDecision::Refuse`]) rather than published, because its most likely
//! wrong answer is a zero indistinguishable from a quiet trial.
//!
//! What that refusal may be overridden by depends on what the probes actually
//! saw, and the line is drawn at *evidence*. A probe that came back **empty**
//! establishes nothing either way — a pruned backend and a genuinely log-free
//! block range answer identically — so that refusal is the default but the
//! operator may override it with `--allow-uncorroborated`, which is their
//! assertion that the quiet blocks really are quiet. A probe that **errored**
//! is different: the endpoint did not answer at all, which is a fact about the
//! endpoint that no assertion about the chain can address, so that refusal
//! stands regardless of the flag — as does an anchor that was corroborating
//! and then stopped mid-scan.

use std::collections::{BTreeMap, HashSet, VecDeque};

use alloy::primitives::{Address, B256, FixedBytes};
use alloy::providers::Provider;
use alloy::rpc::client::BatchRequest;
use alloy::rpc::types::Filter;
use alloy::sol_types::SolEvent as _;
use serde::Serialize;
use tracing::debug;

use super::client::EthHTLC;
use crate::error::{Result, SwapError};

/// The currently pinned Sepolia venue — the address `deploy/maker.env.example`
/// and `docs/testnet.md` both pin (`INTERFACE_VERSION=2`). Every public-trial
/// swap's ETH leg goes through this contract.
pub const PINNED_ETH_HTLC_ADDRESS: &str = "0x351B0EA07739FA9F6769213927D7836a790A5FAF";

/// Block the pinned venue was deployed in (`docs/testnet.md`'s deployment
/// record). The default `from_block`: earlier blocks cannot contain one of its
/// events, and starting at genesis makes public RPCs reject the range outright.
pub const PINNED_DEPLOY_BLOCK: u64 = 11_417_462;

/// The superseded v1 deployment (`docs/testnet.md`). Recorded here only so a
/// run pointed at it can be named for what it is; its `Locked` topic0 differs,
/// so its events are NEVER counted alongside the pinned venue's.
pub const SUPERSEDED_ETH_HTLC_ADDRESS: &str = "0x8636Fe66DFee166589a913140f14d5F57394834A";

/// Default `eth_getLogs` span per request. Public Sepolia RPCs cap the range
/// (publicnode's limit is well above this); a rejected chunk is retried at half
/// the span by [`scan_range`], so this is a starting point, not a hard cap.
pub const DEFAULT_CHUNK_BLOCKS: u64 = 10_000;

/// Smallest span [`scan_range`] will shrink to before giving up. Below this a
/// range rejection is not "too wide" — it is a broken endpoint, and shrinking
/// further would multiply the request count into a rate limit rather than get
/// an answer.
const MIN_CHUNK_BLOCKS: u64 = 1_000;

/// Pause between consecutive chunk requests. Public endpoints meter by request
/// rate, and a report that trips the limiter fails for a reason that has
/// nothing to do with the chain.
const CHUNK_PACING: std::time::Duration = std::time::Duration::from_millis(120);

/// Backoff schedule when the endpoint says "slow down": doubles from this, up
/// to [`MAX_RATE_LIMIT_RETRIES`] times, on the SAME span.
pub const RATE_LIMIT_BACKOFF: std::time::Duration = std::time::Duration::from_millis(500);
pub const MAX_RATE_LIMIT_RETRIES: u32 = 6;

/// How the scan should react to a failed `eth_getLogs`.
///
/// The distinction matters more than it looks: a rate limit answered by
/// *halving the span* doubles the request count, which is exactly the wrong
/// move — it turns one 429 into a cascade that ends with the report giving up
/// at the smallest span it will use. Observed against publicnode, which
/// answers a burst with HTTP 429 / JSON-RPC -32005.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RpcFailure {
    /// The endpoint is asking for fewer requests: back off, keep the span.
    RateLimited,
    /// The span is too wide for this endpoint: halve it.
    RangeTooWide,
    /// Anything else — surface it rather than guess.
    Other,
}

/// Classify a transport/RPC error message. Providers phrase both conditions a
/// dozen ways and share no error code for the range case, so this matches on
/// the wording — pure, and unit-tested against the phrasings actually seen.
pub fn classify_rpc_failure(message: &str) -> RpcFailure {
    let m = message.to_ascii_lowercase();
    if m.contains("rate limit")
        || m.contains("ratelimit")
        || m.contains("too many requests")
        || m.contains("429")
        || m.contains("-32005")
    {
        return RpcFailure::RateLimited;
    }
    if m.contains("block range")
        || m.contains("range too")
        || m.contains("query returned more than")
        || m.contains("too many results")
        || m.contains("log response size")
        || m.contains("exceed maximum block range")
    {
        return RpcFailure::RangeTooWide;
    }
    RpcFailure::Other
}

// ---------------------------------------------------------------------------
// Counting (pure — no chain, no I/O)
// ---------------------------------------------------------------------------

/// One decoded `Locked` event, reduced to exactly the fields the tally needs.
/// Deliberately narrow: amount, hashlock, timelock and `takerLezAccount` are
/// all public on-chain, but none of them is needed to count swaps, so none of
/// them is carried into the report.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LockedRow {
    /// EthHTLC swap id — the unique identity of one lock.
    pub swap_id: FixedBytes<32>,
    /// The taker: whoever locked the ETH. Aggregated to a count, never listed.
    pub sender: Address,
    /// The maker the ETH was locked for — the per-maker split's key.
    pub recipient: Address,
}

/// Accumulates decoded events into the counts. Kept separate from the querying
/// so the counting rules — including the still-open derivation and
/// distinct-taker aggregation — are unit-testable without a chain.
#[derive(Debug, Default, Clone)]
pub struct SwapTally {
    /// Locks observed inside the measured window, deduped by swap id.
    locks: BTreeMap<FixedBytes<32>, LockedRow>,
    claimed: HashSet<FixedBytes<32>>,
    refunded: HashSet<FixedBytes<32>>,
}

impl SwapTally {
    pub fn new() -> Self {
        Self::default()
    }

    /// Record one lock. Idempotent on `swap_id`: a log delivered twice (a
    /// re-org replay, or overlapping chunk boundaries) must not inflate the
    /// attempt count.
    pub fn record_lock(&mut self, row: LockedRow) {
        self.locks.entry(row.swap_id).or_insert(row);
    }

    /// Record a `Claimed`. A terminal event whose lock is outside the measured
    /// window is kept here but contributes nothing — [`SwapTally::counts`]
    /// only ever looks up terminal state for swaps that locked in-window.
    pub fn record_claim(&mut self, swap_id: FixedBytes<32>) {
        self.claimed.insert(swap_id);
    }

    /// Record a `Refunded`. Same out-of-window rule as [`Self::record_claim`].
    pub fn record_refund(&mut self, swap_id: FixedBytes<32>) {
        self.refunded.insert(swap_id);
    }

    /// Collapse the accumulated events into the reported counts.
    ///
    /// `still_open` is *derived*, never observed: it is every in-window lock
    /// for which neither terminal event was seen, i.e.
    /// `attempted - completed - refunded`. That is the number a trial actually
    /// cares about — ETH sitting in escrow right now, either mid-swap or
    /// stranded waiting for its timelock.
    ///
    /// `Claimed` wins over `Refunded` if a swap somehow shows both. The
    /// contract's state machine makes that impossible (both transition out of
    /// `OPEN`), but treating them as an ordered pair rather than two
    /// independent buckets is what keeps
    /// `attempted == completed + refunded + still_open` an identity instead of
    /// an assumption about the chain.
    pub fn counts(&self) -> SwapCounts {
        let mut per_maker: BTreeMap<Address, MakerCounts> = BTreeMap::new();
        let mut takers: HashSet<Address> = HashSet::new();
        let mut completed = 0usize;
        let mut refunded = 0usize;

        for row in self.locks.values() {
            takers.insert(row.sender);
            let entry = per_maker
                .entry(row.recipient)
                .or_insert_with(|| MakerCounts::empty(row.recipient));
            entry.attempted += 1;

            if self.claimed.contains(&row.swap_id) {
                completed += 1;
                entry.completed += 1;
            } else if self.refunded.contains(&row.swap_id) {
                refunded += 1;
                entry.refunded += 1;
            } else {
                entry.still_open += 1;
            }
        }

        let attempted = self.locks.len();
        SwapCounts {
            attempted,
            completed,
            refunded,
            still_open: attempted - completed - refunded,
            distinct_taker_wallets: takers.len(),
            by_maker: per_maker.into_values().collect(),
        }
    }
}

/// Per-maker split, keyed on `Locked.recipient`.
///
/// `attempted` sums to the global `attempted`, but the distinct-taker count
/// does **not** decompose this way and is deliberately absent here: one taker
/// who tried two makers is one wallet globally and would be double-counted by
/// any per-maker taker figure.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct MakerCounts {
    /// Checksummed maker address (`Locked.recipient`).
    pub recipient: String,
    pub attempted: usize,
    pub completed: usize,
    pub refunded: usize,
    pub still_open: usize,
}

impl MakerCounts {
    fn empty(recipient: Address) -> Self {
        Self {
            recipient: recipient.to_string(),
            attempted: 0,
            completed: 0,
            refunded: 0,
            still_open: 0,
        }
    }
}

/// The counts a report prints. Every field is an aggregate; no taker address
/// ever appears (see the module's privacy boundary).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SwapCounts {
    /// Swaps whose ETH leg was locked in the measured window — the attempt
    /// denominator, including takers no maker ever answered.
    pub attempted: usize,
    /// Of those, settled: the taker revealed the preimage and claimed.
    pub completed: usize,
    /// Of those, refunded after the timelock expired.
    pub refunded: usize,
    /// Of those, neither claimed nor refunded as of `outcomes_to_block`.
    pub still_open: usize,
    /// Distinct `Locked.sender` addresses — a **wallet** count, not a person
    /// count. One person with two wallets is two; one wallet shared by two
    /// people is one.
    pub distinct_taker_wallets: usize,
    /// Split by `Locked.recipient`, ascending by address.
    pub by_maker: Vec<MakerCounts>,
}

// ---------------------------------------------------------------------------
// Block-range plumbing
// ---------------------------------------------------------------------------

/// Split `[from, to]` (inclusive, both ends) into consecutive spans of at most
/// `chunk` blocks. Returns empty when `to < from`, so an empty window is a
/// no-op rather than an inverted query.
pub fn block_chunks(from: u64, to: u64, chunk: u64) -> Vec<(u64, u64)> {
    let chunk = chunk.max(1);
    let mut out = Vec::new();
    let mut start = from;
    while start <= to {
        let end = start.saturating_add(chunk - 1).min(to);
        out.push((start, end));
        if end == u64::MAX {
            break;
        }
        start = end + 1;
    }
    out
}

/// Which blocks a report measured. `locks_to` bounds the *attempt* window;
/// `outcomes_to` is how far terminal events were followed, which is normally
/// the chain head even when `locks_to` is earlier — otherwise a swap locked at
/// the end of the window would be reported still-open purely because the query
/// stopped before its claim.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReportWindow {
    pub locks_from: u64,
    pub locks_to: u64,
    pub outcomes_to: u64,
}

/// A canary query: a filter a healthy endpoint always answers non-empty,
/// batched alongside every real query so the answer carries a proof that the
/// backend serving it actually had the log data for this scan's block range.
///
/// ## Why this is necessary
///
/// The pinned `ETH_RPC_URL` (`wss://ethereum-sepolia-rpc.publicnode.com`) is a
/// load-balanced pool, and its backends **disagree**: for one and the same
/// `eth_getLogs` range, some return the logs and others return `[]` — no error,
/// no warning, just an empty array, because their log index does not cover
/// those blocks. Repeating an identical query against it returns a different
/// count roughly half the time, and the pool mixes at least a full-history
/// Geth with a reth whose receipts are pruned. Nothing in the response
/// distinguishes them, and both report the same head block.
///
/// For a swap count meant to be the trial's primary number, silently reading
/// low is the worst possible failure: it is indistinguishable from a quiet
/// trial, which is exactly the conclusion someone would draw from it.
///
/// ## How a canary makes an answer provable
///
/// A JSON-RPC **batch is served by a single backend** — verified against
/// publicnode: identical queries inside one HTTP batch always agree, while
/// separate requests do not. So a canary batched with the real query proves
/// something about *that* response, not about the pool. This is why
/// `cli::report::connect` insists on the HTTP transport: alloy's WebSocket
/// transport splits a batch into separate sends, which would let the canary and
/// the query it vouches for be answered by different nodes.
///
/// Two anchors are used, not one, and this is the whole subtlety: a backend
/// whose log index covers only part of the chain still answers correctly for
/// the part it has. A single anchor at the start of the scan was observed
/// passing on a backend that then returned `[]` for a range 20k blocks later
/// which really did contain three locks. Every client prunes to a *contiguous*
/// interval, so anchoring at **both ends** closes that hole: if a backend
/// serves a log at the oldest block of the scan and a log at the newest, its
/// interval contains everything in between.
///
/// Each anchor is a single log at a single block, so the per-request cost is a
/// couple of hundred bytes.
#[derive(Debug, Clone)]
pub struct Canary {
    filter: Filter,
    /// How many logs a healthy backend returns for `filter`.
    expected: usize,
}

/// Blocks to walk inward from each end of a scan looking for one with logs to
/// anchor on. Sepolia blocks are essentially never empty, so this succeeds on
/// the first try there; an end whose whole walk produces nothing is reported
/// by [`probe_canaries`] as a miss, with what the probes actually did.
const CANARY_PROBE_BLOCKS: u64 = 16;

/// Attempts per candidate anchor block. A backend without the log index
/// answers `[]`, so a single empty answer does not yet mean the block is empty.
const CANARY_PROBE_ATTEMPTS: usize = 5;

/// Extra tries granted to a rate-limited probe, on top of
/// [`CANARY_PROBE_ATTEMPTS`]. A 429 is the endpoint asking for fewer requests
/// (see [`RpcFailure`]), not a refusal to answer, so it must not burn one of
/// the attempts this block gets — but it cannot be retried forever either.
const CANARY_RATE_LIMIT_RETRIES: u32 = 3;

/// How many times one chunk may come back uncorroborated before the scan gives
/// up. Kept modest on purpose: these retries go out over the connection that is
/// already established, and a keep-alive connection to a pool that balances per
/// connection keeps landing on the same backend — so retrying here cannot
/// escape a pruned node. Getting a different one is the caller's job, by
/// reconnecting and rescanning (see `cli::report::MAX_SCAN_ATTEMPTS`); this
/// only has to ride out the per-request flakiness of a pool that does
/// rebalance mid-connection.
const MAX_CORROBORATION_ATTEMPTS: usize = 6;

const CORROBORATION_RETRY_DELAY: std::time::Duration = std::time::Duration::from_millis(150);

/// Whether a batched response may be counted, given how each canary in it
/// answered as `(observed, expected)`.
///
/// An empty slice only ever reaches here on the caller's explicit
/// `--allow-uncorroborated` opt-in over a scan where NO probe anchored (see
/// [`ScanDecision`]): there are then no anchors to verify against, so the
/// answer is taken and the report says it was uncorroborated. Without that
/// opt-in — and always, when one end anchored and the other did not — the scan
/// is refused before any of this runs.
///
/// Note what is deliberately *not* here: "a non-empty answer needs no proof".
/// It is not sound. A backend whose log index starts inside the queried chunk
/// returns the tail of that chunk and nothing else — non-empty, and wrong.
pub fn is_corroborated(canaries: &[(usize, usize)]) -> bool {
    canaries
        .iter()
        .all(|(observed, expected)| observed >= expected)
}

/// Which end of a scan a probe walk started from.
///
/// Worth naming in a refusal, because the two ends fail for different reasons:
/// a backend that has pruned old receipts misses the oldest end, while one
/// whose index has not caught up misses the newest.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScanEnd {
    Oldest,
    Newest,
}

impl ScanEnd {
    fn label(self) -> &'static str {
        match self {
            ScanEnd::Oldest => "oldest",
            ScanEnd::Newest => "newest",
        }
    }

    fn opposite(self) -> Self {
        match self {
            ScanEnd::Oldest => ScanEnd::Newest,
            ScanEnd::Newest => ScanEnd::Oldest,
        }
    }
}

/// Why one end's probe walk produced no anchor.
///
/// The counts are kept apart rather than summarised as "found nothing": "the
/// endpoint answered, and those blocks hold no logs", "the endpoint asked us to
/// slow down" and "the endpoint never answered at all" are different facts
/// about it, and a refusal that reports one as another sends the operator after
/// the wrong thing.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProbeMiss {
    /// Requests the endpoint answered with an empty log list.
    pub empty_responses: usize,
    /// Requests the endpoint answered with a rate limit. Tracked apart from
    /// [`Self::failed_requests`] on purpose: the module treats a 429 as the
    /// endpoint asking for fewer requests rather than as a fault (see
    /// [`RpcFailure`]), and publicnode answering a burst that way is documented
    /// expected behaviour — so it must not read as the endpoint being broken.
    pub rate_limited: usize,
    /// Requests that errored for some other reason, and never produced an
    /// answer at all.
    pub failed_requests: usize,
    /// The last non-rate-limit error seen, for the operator to act on.
    pub last_error: Option<String>,
}

impl ProbeMiss {
    /// **The** question this whole surface turns on: did the walk ever get a
    /// successful answer out of the endpoint?
    ///
    /// A walk only becomes a [`ProbeMiss`] when it found no anchor, so the only
    /// successful answers it can hold are empty ones — which is exactly the
    /// ambiguity [`Anchoring`] exists for: a pruned backend and a log-free
    /// block range are indistinguishable in an empty log list. Ambiguous, but
    /// it IS an observation of the chain, and an operator who knows their chain
    /// is quiet can resolve it.
    ///
    /// `false` means nothing whatsoever was established — every probe errored,
    /// or was throttled, or some mix. There is no ambiguity for the operator to
    /// resolve because there is no observation, so no opt-in applies.
    ///
    /// Deliberately phrased over "was there a successful answer" rather than
    /// over the kinds of failure: each past attempt to enumerate the failure
    /// kinds (an error is a fault / a 429 is not) closed one hole and opened
    /// its mirror image. This is the single place the rule lives.
    pub fn answered_successfully(&self) -> bool {
        self.empty_responses > 0
    }

    /// Negation of [`Self::answered_successfully`], for readability at the
    /// call sites that care about the refusing direction.
    pub fn never_answered(&self) -> bool {
        !self.answered_successfully()
    }

    fn describe(&self) -> String {
        let mut parts: Vec<String> = Vec::new();
        if self.empty_responses > 0 {
            parts.push(format!(
                "{} probe requests came back with an empty log list",
                self.empty_responses
            ));
        }
        if self.rate_limited > 0 {
            parts.push(format!("{} were rate-limited", self.rate_limited));
        }
        if self.failed_requests > 0 {
            let last = self.last_error.as_deref().unwrap_or("unknown");
            let alone = parts.is_empty();
            parts.push(if alone {
                format!(
                    "all {} probe requests failed (last error: {last})",
                    self.failed_requests
                )
            } else {
                format!("{} failed (last error: {last})", self.failed_requests)
            });
        }
        match parts.len() {
            0 => "no probe request was made".to_string(),
            1 => parts.swap_remove(0),
            _ => {
                let last = parts.pop().expect("more than one part");
                format!("{} and {last}", parts.join(", "))
            }
        }
    }
}

/// What one end's probe walk produced.
#[derive(Debug, Clone)]
enum ProbeOutcome {
    Anchored(Box<Canary>),
    Missed(ProbeMiss),
}

/// What the anchor probes established about the backend serving this scan.
///
/// The distinction this type draws is the whole reason it exists: an **empty**
/// probe response proves nothing, because a backend that has pruned this era's
/// log index answers `[]` exactly like a genuinely quiet chain does. Only a
/// successful, **non-empty** probe response is evidence that the backend
/// actually holds these blocks' logs. Both ends are always probed, so which of
/// the three outcomes below applies is a fact about the whole scan and not
/// about whichever end happened to be tried first.
#[derive(Debug, Clone)]
pub enum Anchoring {
    /// Both ends of the scan anchored on a block whose logs this endpoint
    /// served, so every response in the scan can be proven against them.
    ///
    /// One caveat on how much that proves: when the two probe walks meet on the
    /// same block — reachable on a span short enough that the inward walks
    /// overlap, roughly `2 * CANARY_PROBE_BLOCKS` — the anchors dedupe to a
    /// single canary, and what is then proven is a *point* rather than an
    /// interval. A backend whose index begins exactly at that block would still
    /// corroborate.
    Anchored(Vec<Canary>),
    /// One end anchored and the other did not.
    ///
    /// What this means depends entirely on `miss`, and the code cannot tell on
    /// its own: if the missing end's probes came back empty, those blocks may
    /// genuinely hold no logs *or* this backend may not hold them, and the
    /// responses look identical. Only an errored probe (see
    /// [`ProbeMiss::answered_successfully`]) leaves anything to resolve.
    PartiallyAnchored {
        /// The anchor the other end did produce. Kept, not discarded: an
        /// operator who overrides the refusal still gets their queries batched
        /// against it, which proves less than two anchors and more than none.
        anchored: Box<Canary>,
        /// The end that produced no anchor.
        missing: ScanEnd,
        /// What its probes actually did.
        miss: ProbeMiss,
    },
    /// Neither end anchored. What that means depends on whether the probes were
    /// answered at all (see [`ProbeMiss::answered_successfully`]): answered and
    /// empty leaves "the chain is quiet" and "this endpoint has no index for
    /// this era" indistinguishable, while a walk that was only errored or only
    /// throttled established neither. Either way the count is unproven.
    Unanchored { oldest: ProbeMiss, newest: ProbeMiss },
}

impl Anchoring {
    /// Every anchor the probes did produce: two when both ends anchored (one
    /// if they met on the same block), one when only one end did, none
    /// otherwise.
    ///
    /// A downgraded scan batches whatever is here rather than falling back to a
    /// bare `eth_getLogs`. Overriding the refusal accepts that the *interval*
    /// is unproven; it is not a reason to throw away the evidence that exists.
    pub fn canaries(&self) -> Vec<Canary> {
        match self {
            Anchoring::Anchored(canaries) => canaries.clone(),
            Anchoring::PartiallyAnchored { anchored, .. } => vec![(**anchored).clone()],
            Anchoring::Unanchored { .. } => Vec::new(),
        }
    }

    /// The ends whose probes never got a successful answer out of the endpoint
    /// (see [`ProbeMiss::answered_successfully`]). Non-empty means nothing was
    /// observed there, so the refusal stands whatever the caller passes.
    fn unanswered_ends(&self) -> Vec<(ScanEnd, &ProbeMiss)> {
        let ends: Vec<(ScanEnd, &ProbeMiss)> = match self {
            Anchoring::Anchored(_) => Vec::new(),
            Anchoring::PartiallyAnchored { missing, miss, .. } => vec![(*missing, miss)],
            Anchoring::Unanchored { oldest, newest } => {
                vec![(ScanEnd::Oldest, oldest), (ScanEnd::Newest, newest)]
            }
        };
        ends.into_iter()
            .filter(|(_, m)| m.never_answered())
            .collect()
    }
}

/// Whether a scan may be counted, given what its probes established.
///
/// Refusing is the default on purpose. An unproven scan's most likely wrong
/// answer is *zero*, and a zero that came from a pruned backend is
/// indistinguishable from a quiet trial — the exact failure this whole
/// corroboration apparatus exists to prevent.
///
/// The override is scoped by one question — did the probes ever get a
/// successful answer (see [`ProbeMiss::answered_successfully`])? An empty log
/// list leaves the question of *why* open, and an operator who knows the chain
/// is quiet may answer it with `--allow-uncorroborated`. A walk that was never
/// answered at all — errored, throttled, or both — established nothing for an
/// assertion about the chain to bear on, so that refusal is not overridable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScanDecision {
    /// Anchored at both ends: every response is batched with a canary, and one
    /// that cannot be corroborated is retried rather than counted.
    Corroborated,
    /// Not anchored at both ends, but every end that missed was at least
    /// answered, and the caller explicitly opted in: count it against whatever
    /// anchors were found, and report `corroborated: false` so the reader knows
    /// which kind of number it is.
    ProceedUnproven,
    /// Publish nothing.
    Refuse,
}

/// The decision rule itself — pure, so "when does this report refuse" is
/// testable without a chain.
pub fn scan_decision(anchoring: &Anchoring, allow_uncorroborated: bool) -> ScanDecision {
    if matches!(anchoring, Anchoring::Anchored(_)) {
        return ScanDecision::Corroborated;
    }
    // An end the endpoint never answered established nothing, so there is
    // nothing for the operator to vouch for and the flag does not reach it.
    if !anchoring.unanswered_ends().is_empty() {
        return ScanDecision::Refuse;
    }
    if allow_uncorroborated {
        ScanDecision::ProceedUnproven
    } else {
        ScanDecision::Refuse
    }
}

/// Why a scan was refused, in terms an operator can act on. `None` for an
/// anchoring that is not a refusal.
///
/// Pure, and separate from the probing, because the wording is load-bearing in
/// both directions: telling an operator the endpoint is broken when the chain
/// was merely quiet sends them chasing a phantom, and telling them the chain
/// was merely quiet when the endpoint is broken is how a silent under-count
/// gets published.
pub fn refusal_message(from: u64, to: u64, anchoring: &Anchoring) -> Option<String> {
    let unanswered = anchoring.unanswered_ends();
    if !unanswered.is_empty() {
        let ends = unanswered
            .iter()
            .map(|(end, _)| end.label())
            .collect::<Vec<_>>()
            .join(" and ");
        let detail = unanswered
            .iter()
            .map(|(end, miss)| format!("at the {} end {}", end.label(), miss.describe()))
            .collect::<Vec<_>>()
            .join(", ");

        // Neither branch may reach for the empty-log ambiguity or the quiet
        // localnet: no probe here came back with a log list at all, so the
        // chain was never observed and nothing about how quiet it is applies.
        let only_throttled = unanswered.iter().all(|(_, m)| m.failed_requests == 0);
        return Some(if only_throttled {
            format!(
                "blocks {from}-{to}: the RPC endpoint never answered an anchor probe at the \
                 {ends} end of this scan — it asked for fewer requests instead ({detail}). \
                 Nothing about the chain was established: not one probe came back with a log \
                 list, empty or otherwise, so there is nothing for --allow-uncorroborated to \
                 vouch for and it does not apply here. Wait for the throttle to clear and \
                 re-run, or narrow the window with --from-block/--since so the scan needs fewer \
                 probes."
            )
        } else {
            format!(
                "blocks {from}-{to}: the RPC endpoint did not answer a single anchor probe at \
                 the {ends} end of this scan — {detail}. Not one of those requests came back \
                 with a log list, so they produced no evidence in either direction, and this \
                 scan cannot be proven against a backend that would not reply. Reconnecting \
                 draws a different backend from the pool; if it keeps happening, point \
                 --eth-rpc-url at an endpoint that serves historical logs reliably, or narrow \
                 the window. --allow-uncorroborated cannot cover this: it asserts that a range \
                 of blocks holds no logs, which says nothing about an endpoint that failed to \
                 respond."
            )
        });
    }

    match anchoring {
        Anchoring::Anchored(_) => None,
        Anchoring::PartiallyAnchored { missing, miss, .. } => {
            let (missing_end, anchored_end) = (missing.label(), missing.opposite().label());
            let detail = miss.describe();
            Some(format!(
                "blocks {from}-{to}: the {anchored_end} end of this scan anchored, but the \
                 {missing_end} end did not — {detail}. That is ambiguous, and deliberately \
                 reported as such: those blocks may genuinely hold no logs, or this backend may \
                 not hold them, and an empty log list looks the same either way. Without an \
                 anchor at both ends nothing shows that its log index covers the whole scanned \
                 range, so no count is published by default. Reconnecting draws a different \
                 backend from the pool. If you know the {missing_end} end of this range is \
                 genuinely log-free, pass --allow-uncorroborated to accept a count proven only \
                 at the {anchored_end} end."
            ))
        }
        Anchoring::Unanchored { oldest, newest } => {
            let (o, n) = (oldest.describe(), newest.describe());
            Some(format!(
                "blocks {from}-{to}: neither end of this scan could be anchored — at the oldest \
                 end {o}, at the newest end {n} — so nothing shows that the RPC backend serving \
                 this connection holds these blocks' logs at all. That is ambiguous: a backend \
                 whose log index does not cover them answers with an empty list rather than an \
                 error, exactly as a genuinely log-free range does, so a count taken here could \
                 read zero on a busy venue. Point --eth-rpc-url at a full-history endpoint, or \
                 narrow the window to blocks it still retains. If this really is a chain with no \
                 logs to anchor on (a quiet localnet), pass --allow-uncorroborated to accept an \
                 unproven count."
            ))
        }
    }
}

/// Find the anchors for a scan: one as close as possible to `from`, one as
/// close as possible to `to`, so a backend that answers both provably covers
/// the interval between them (see [`Canary`]).
///
/// BOTH ends are always probed, whatever the first one does. Stopping at the
/// first miss would throw away an anchor that was found and leave the two
/// genuinely different failures — a partial log index and a chain with no logs
/// — indistinguishable to the caller.
pub async fn probe_canaries<P: Provider>(provider: &P, from: u64, to: u64) -> Result<Anchoring> {
    let forward: Vec<u64> = (from..=to).take(CANARY_PROBE_BLOCKS as usize).collect();
    let backward: Vec<u64> = (from..=to).rev().take(CANARY_PROBE_BLOCKS as usize).collect();

    let oldest = anchor_in(provider, &forward).await;
    let newest = anchor_in(provider, &backward).await;

    for (end, outcome) in [(ScanEnd::Oldest, &oldest), (ScanEnd::Newest, &newest)] {
        match outcome {
            ProbeOutcome::Anchored(c) => {
                debug!(end = end.label(), expected = c.expected, block = ?c.filter.get_from_block(), "chain-report: canary anchored")
            }
            ProbeOutcome::Missed(miss) => {
                debug!(
                    end = end.label(),
                    detail = miss.describe(),
                    "chain-report: no canary anchor found near this end of the scan"
                )
            }
        }
    }

    Ok(match (oldest, newest) {
        (ProbeOutcome::Anchored(a), ProbeOutcome::Anchored(b)) => {
            let mut out = vec![*a];
            if !out.iter().any(|c| c.filter == b.filter) {
                out.push(*b);
            }
            Anchoring::Anchored(out)
        }
        (ProbeOutcome::Anchored(anchored), ProbeOutcome::Missed(miss)) => {
            Anchoring::PartiallyAnchored {
                anchored,
                missing: ScanEnd::Newest,
                miss,
            }
        }
        (ProbeOutcome::Missed(miss), ProbeOutcome::Anchored(anchored)) => {
            Anchoring::PartiallyAnchored {
                anchored,
                missing: ScanEnd::Oldest,
                miss,
            }
        }
        (ProbeOutcome::Missed(oldest), ProbeOutcome::Missed(newest)) => {
            Anchoring::Unanchored { oldest, newest }
        }
    })
}

/// First block in `candidates` that has any logs, anchored on whichever address
/// logged least in it (so the canary stays a couple of hundred bytes).
///
/// On a miss it reports what the probes did rather than just that they failed:
/// empty answers and failed requests say different things about the endpoint.
async fn anchor_in<P: Provider>(provider: &P, candidates: &[u64]) -> ProbeOutcome {
    let mut miss = ProbeMiss::default();
    let mut first = true;
    for &block in candidates {
        let whole_block = Filter::new().from_block(block).to_block(block);
        let mut attempt = 0usize;
        let mut rate_limit_retries = 0u32;
        while attempt < CANARY_PROBE_ATTEMPTS {
            // Paced between candidate blocks as well as between attempts: this
            // walk is up to CANARY_PROBE_BLOCKS * CANARY_PROBE_ATTEMPTS
            // requests, which unpaced is exactly the burst shape a public
            // endpoint answers with a 429.
            if !std::mem::take(&mut first) {
                tokio::time::sleep(CHUNK_PACING).await;
            }
            let logs = match provider.get_logs(&whole_block).await {
                Ok(logs) => logs,
                // A probe failure is not fatal: the scan itself will surface any
                // persistent transport problem with a better message. A rate
                // limit is worth waiting out though — this probe is what the
                // whole scan's trustworthiness rests on — and it is the
                // endpoint asking for fewer requests rather than failing to
                // answer, so it neither burns an attempt nor counts as a fault.
                Err(e) => {
                    let msg = e.to_string();
                    debug!(%msg, block, "chain-report: canary probe request failed");
                    if classify_rpc_failure(&msg) == RpcFailure::RateLimited {
                        miss.rate_limited += 1;
                        if rate_limit_retries < CANARY_RATE_LIMIT_RETRIES {
                            rate_limit_retries += 1;
                            tokio::time::sleep(RATE_LIMIT_BACKOFF).await;
                            continue;
                        }
                    } else {
                        miss.failed_requests += 1;
                        miss.last_error = Some(msg);
                    }
                    attempt += 1;
                    continue;
                }
            };
            attempt += 1;
            if logs.is_empty() {
                miss.empty_responses += 1;
                continue;
            }
            let mut per_address: BTreeMap<Address, usize> = BTreeMap::new();
            for log in &logs {
                *per_address.entry(log.address()).or_insert(0) += 1;
            }
            let (anchor, expected) = per_address
                .into_iter()
                .min_by_key(|&(_, n)| n)
                .expect("logs is non-empty");
            return ProbeOutcome::Anchored(Box::new(Canary {
                filter: Filter::new()
                    .address(anchor)
                    .from_block(block)
                    .to_block(block),
                expected,
            }));
        }
    }
    ProbeOutcome::Missed(miss)
}

/// One scan's result: the tally, plus whether every response it counted came
/// with a canary proof (see [`Canary`]). `corroborated: false` is only ever
/// reachable through the caller's explicit opt-in (see [`ScanDecision`]), and
/// the flag is reported rather than hidden so a reader can tell an unproven
/// count from a proven one. A scan that queried nothing at all is vacuously
/// corroborated: no response went unproven, because there was no response.
#[derive(Debug, Clone)]
pub struct Scan {
    pub tally: SwapTally,
    pub corroborated: bool,
}

/// Query the contract's logs and fold them into a [`SwapTally`].
///
/// Read-only: `eth_getLogs` only. No key, no gas, no state mutation, and no
/// dependency on a running maker loop.
pub async fn collect_tally<P: Provider>(
    provider: &P,
    address: Address,
    window: &ReportWindow,
    chunk: u64,
    allow_uncorroborated: bool,
) -> Result<Scan> {
    let mut tally = SwapTally::new();

    let locked = EthHTLC::Locked::SIGNATURE_HASH;
    let claimed = EthHTLC::Claimed::SIGNATURE_HASH;
    let refunded = EthHTLC::Refunded::SIGNATURE_HASH;

    // An empty window selects no blocks, so nothing is queried and there is
    // nothing to prove — not an unproven scan, a scan that never happened. No
    // response went uncorroborated because there was no response, so the flag
    // stays `true` rather than reading as an unproven count to a script.
    let span_end = window.outcomes_to.max(window.locks_to);
    if span_end < window.locks_from {
        return Ok(Scan {
            tally,
            corroborated: true,
        });
    }

    // Anchor both ends of the widest span this scan touches — locks start to
    // outcomes end — so an accepted response provably came from a backend whose
    // log index covers the whole of it (see `Canary`).
    let anchoring = probe_canaries(provider, window.locks_from, span_end).await?;
    let decision = scan_decision(&anchoring, allow_uncorroborated);
    if decision == ScanDecision::Refuse {
        return Err(SwapError::EthLogsUncorroborated(
            refusal_message(window.locks_from, span_end, &anchoring)
                .expect("a refused scan always has a reason"),
        ));
    }
    let canaries = anchoring.canaries();

    // One combined filter over the attempt window: three topic0s in a single
    // eth_getLogs is a third of the round trips of three separate scans, and
    // the decode below dispatches on topic0 anyway.
    scan_range(
        provider,
        address,
        window.locks_from,
        window.locks_to,
        vec![locked, claimed, refunded],
        chunk,
        &canaries,
        &mut tally,
    )
    .await?;

    // Then follow outcomes past the attempt window (a no-op when they coincide,
    // which is the default). Locks found here are ignored by construction: the
    // filter carries no `Locked` topic, so nothing can widen the attempt window.
    if window.outcomes_to > window.locks_to {
        scan_range(
            provider,
            address,
            window.locks_to + 1,
            window.outcomes_to,
            vec![claimed, refunded],
            chunk,
            &canaries,
            &mut tally,
        )
        .await?;
    }

    Ok(Scan {
        tally,
        corroborated: decision == ScanDecision::Corroborated,
    })
}

/// Scan one block range in chunks, halving the span and retrying whenever the
/// endpoint rejects a request. Public RPCs disagree about the maximum
/// `eth_getLogs` span and say so only by failing, so the retry is what makes a
/// deployment-to-head scan work without the caller having to know the limit.
#[allow(clippy::too_many_arguments)]
async fn scan_range<P: Provider>(
    provider: &P,
    address: Address,
    from: u64,
    to: u64,
    topics: Vec<B256>,
    chunk: u64,
    canaries: &[Canary],
    tally: &mut SwapTally,
) -> Result<()> {
    let mut chunk = chunk.max(1);
    // The spans come from `block_chunks` rather than an inline cursor so the
    // arithmetic that decides which blocks get queried has exactly one
    // implementation — and so its unit test covers the code that really issues
    // `eth_getLogs`. A range rejection re-derives the remaining spans at the
    // smaller size; a span is only dropped once its logs are in the tally.
    let mut spans: VecDeque<(u64, u64)> = block_chunks(from, to, chunk).into();
    let mut rate_limit_retries = 0u32;
    let mut first = true;

    while let Some(&(cursor, end)) = spans.front() {
        if !std::mem::take(&mut first) {
            tokio::time::sleep(CHUNK_PACING).await;
        }
        let filter = Filter::new()
            .address(address)
            .event_signature(topics.clone())
            .from_block(cursor)
            .to_block(end);

        let mut fetched: Option<Vec<alloy::rpc::types::Log>> = None;
        let mut rpc_error: Option<String> = None;

        for attempt in 0..MAX_CORROBORATION_ATTEMPTS {
            match corroborated_get_logs(provider, &filter, canaries).await {
                Ok(Some(logs)) => {
                    fetched = Some(logs);
                    break;
                }
                Ok(None) => {
                    debug!(
                        from = cursor,
                        to = end,
                        attempt,
                        "chain-report: empty answer could not be corroborated, retrying"
                    );
                }
                Err(e) => {
                    rpc_error = Some(e.to_string());
                    break;
                }
            }
            tokio::time::sleep(CORROBORATION_RETRY_DELAY).await;
        }

        if let Some(msg) = rpc_error {
            match classify_rpc_failure(&msg) {
                RpcFailure::RateLimited if rate_limit_retries < MAX_RATE_LIMIT_RETRIES => {
                    let wait = RATE_LIMIT_BACKOFF * 2u32.pow(rate_limit_retries);
                    rate_limit_retries += 1;
                    debug!(?wait, %msg, "chain-report: endpoint rate-limited us, backing off");
                    tokio::time::sleep(wait).await;
                    continue;
                }
                RpcFailure::RangeTooWide if chunk > MIN_CHUNK_BLOCKS => {
                    chunk = (chunk / 2).max(MIN_CHUNK_BLOCKS);
                    spans = block_chunks(cursor, to, chunk).into();
                    debug!(chunk, %msg, "chain-report: shrinking block range");
                    continue;
                }
                _ => {
                    return Err(SwapError::EthRpc(format!(
                        "eth_getLogs failed for blocks {cursor}-{end} (span {chunk}): {msg}"
                    )));
                }
            }
        }

        let Some(logs) = fetched else {
            return Err(SwapError::EthLogsUncorroborated(format!(
                "blocks {cursor}-{end}: the backend serving this connection could not be shown \
                 to hold the log data for the scanned range, in {MAX_CORROBORATION_ATTEMPTS} \
                 attempts. Public endpoints load-balance across nodes with different log \
                 retention, and a node without the index answers with an empty list rather than \
                 an error, so counting it would silently under-report."
            )));
        };

        for log in &logs {
            ingest_log(&log.inner, tally)?;
        }
        debug!(
            from = cursor,
            to = end,
            logs = logs.len(),
            "chain-report: scanned block range"
        );
        spans.pop_front();
        rate_limit_retries = 0;
    }

    Ok(())
}

/// One `eth_getLogs`, batched with the canary so the answer carries its own
/// proof. `Ok(None)` means the response came from a backend that cannot be
/// shown to have the log index — retry, do not count it.
async fn corroborated_get_logs<P: Provider>(
    provider: &P,
    filter: &Filter,
    canaries: &[Canary],
) -> Result<Option<Vec<alloy::rpc::types::Log>>> {
    if canaries.is_empty() {
        let logs = provider
            .get_logs(filter)
            .await
            .map_err(|e| SwapError::EthRpc(e.to_string()))?;
        return Ok(Some(logs));
    }

    let mut batch = BatchRequest::new(provider.client());
    let subject = batch
        .add_call::<_, Vec<alloy::rpc::types::Log>>("eth_getLogs", &(filter.clone(),))
        .map_err(|e| SwapError::EthRpc(format!("could not build a batched eth_getLogs: {e}")))?;
    let probes = canaries
        .iter()
        .map(|c| {
            batch
                .add_call::<_, Vec<alloy::rpc::types::Log>>("eth_getLogs", &(c.filter.clone(),))
                .map_err(|e| {
                    SwapError::EthRpc(format!("could not build a canary eth_getLogs: {e}"))
                })
        })
        .collect::<Result<Vec<_>>>()?;

    batch
        .send()
        .await
        .map_err(|e| SwapError::EthRpc(e.to_string()))?;

    let logs = subject
        .await
        .map_err(|e| SwapError::EthRpc(e.to_string()))?;

    // A canary that errors is not the subject's failure: score it zero, which
    // reads as "not corroborated" and makes the caller retry.
    let mut observed = Vec::with_capacity(probes.len());
    for (probe, canary) in probes.into_iter().zip(canaries) {
        observed.push((probe.await.map(|l| l.len()).unwrap_or(0), canary.expected));
    }

    if !is_corroborated(&observed) {
        debug!(?observed, "chain-report: canary scores");
    }
    if is_corroborated(&observed) {
        Ok(Some(logs))
    } else {
        Ok(None)
    }
}

/// Decode one log into the tally. Anything that is not one of the three known
/// events is skipped — the filter already restricts topic0, so this is a belt
/// against a provider returning more than was asked for, not a real branch.
fn ingest_log(log: &alloy::primitives::Log, tally: &mut SwapTally) -> Result<()> {
    let Some(topic0) = log.topics().first().copied() else {
        return Ok(());
    };

    if topic0 == EthHTLC::Locked::SIGNATURE_HASH {
        let ev = EthHTLC::Locked::decode_log_data(&log.data).map_err(|e| {
            SwapError::EthAbiMismatch(format!("could not decode a Locked log: {e}"))
        })?;
        tally.record_lock(LockedRow {
            swap_id: ev.swapId,
            sender: ev.sender,
            recipient: ev.recipient,
        });
    } else if topic0 == EthHTLC::Claimed::SIGNATURE_HASH {
        let ev = EthHTLC::Claimed::decode_log_data(&log.data).map_err(|e| {
            SwapError::EthAbiMismatch(format!("could not decode a Claimed log: {e}"))
        })?;
        tally.record_claim(ev.swapId);
    } else if topic0 == EthHTLC::Refunded::SIGNATURE_HASH {
        let ev = EthHTLC::Refunded::decode_log_data(&log.data).map_err(|e| {
            SwapError::EthAbiMismatch(format!("could not decode a Refunded log: {e}"))
        })?;
        tally.record_refund(ev.swapId);
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Time → block
// ---------------------------------------------------------------------------

/// Parse a UTC time window bound: a Unix timestamp (`1755734400`), a date
/// (`2026-08-21`, midnight UTC), or a date-time (`2026-08-21T14:30:00`, with an
/// optional trailing `Z`). No date dependency is added for this — see
/// [`days_from_civil`].
pub fn parse_time_arg(s: &str) -> std::result::Result<u64, String> {
    let s = s.trim();
    if s.is_empty() {
        return Err("empty time value".into());
    }
    if s.bytes().all(|b| b.is_ascii_digit()) {
        return s
            .parse::<u64>()
            .map_err(|e| format!("invalid Unix timestamp '{s}': {e}"));
    }

    let body = s.strip_suffix('Z').unwrap_or(s);
    let (date, time) = match body.split_once(['T', ' ']) {
        Some((d, t)) => (d, Some(t)),
        None => (body, None),
    };

    let mut date_parts = date.split('-');
    let (Some(y), Some(m), Some(d), None) = (
        date_parts.next(),
        date_parts.next(),
        date_parts.next(),
        date_parts.next(),
    ) else {
        return Err(format!(
            "invalid time '{s}': expected a Unix timestamp, YYYY-MM-DD, or YYYY-MM-DDTHH:MM:SS (UTC)"
        ));
    };
    let year: i64 = y.parse().map_err(|_| format!("invalid year in '{s}'"))?;
    let month: u32 = m.parse().map_err(|_| format!("invalid month in '{s}'"))?;
    let day: u32 = d.parse().map_err(|_| format!("invalid day in '{s}'"))?;
    if !(1..=12).contains(&month) {
        return Err(format!("invalid date '{date}' in '{s}': no month {month}"));
    }
    let last = days_in_month(year, month);
    if !(1..=last).contains(&day) {
        return Err(format!(
            "invalid date '{date}' in '{s}': {year}-{month:02} has {last} days"
        ));
    }

    let (mut hh, mut mm, mut ss) = (0u64, 0u64, 0u64);
    if let Some(t) = time {
        let mut tp = t.split(':');
        hh = tp
            .next()
            .ok_or_else(|| format!("invalid time in '{s}'"))?
            .parse()
            .map_err(|_| format!("invalid hour in '{s}'"))?;
        if let Some(part) = tp.next() {
            mm = part
                .parse()
                .map_err(|_| format!("invalid minute in '{s}'"))?;
        }
        if let Some(part) = tp.next() {
            ss = part
                .parse()
                .map_err(|_| format!("invalid second in '{s}'"))?;
        }
        if tp.next().is_some() || hh > 23 || mm > 59 || ss > 59 {
            return Err(format!("invalid time-of-day in '{s}'"));
        }
    }

    let days = days_from_civil(year, month, day);
    let secs = days * 86_400 + (hh * 3600 + mm * 60 + ss) as i64;
    u64::try_from(secs).map_err(|_| format!("time '{s}' is before the Unix epoch"))
}

/// Whether `y` is a leap year in the proleptic Gregorian calendar.
fn is_leap_year(y: i64) -> bool {
    (y % 4 == 0 && y % 100 != 0) || y % 400 == 0
}

/// Length of `month` in `year`. Guards [`days_from_civil`], which is pure
/// arithmetic with no range check: fed an impossible day it happily returns the
/// epoch day of a DIFFERENT date, so `--since 2026-02-30` would measure a
/// window starting two days later than asked and print that as if it were what
/// the operator wrote.
fn days_in_month(year: i64, month: u32) -> u32 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if is_leap_year(year) => 29,
        2 => 28,
        _ => 0,
    }
}

/// Days since 1970-01-01 for a proleptic-Gregorian civil date (Howard
/// Hinnant's `days_from_civil`). Inlined rather than pulling in a date crate:
/// this is the only calendar arithmetic in the repo, and a new dependency for
/// six lines is a worse trade than six lines.
fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400; // [0, 399]
    let mp = ((m + 9) % 12) as i64; // Mar=0 … Feb=11
    let doy = (153 * mp + 2) / 5 + d as i64 - 1; // [0, 365]
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy; // [0, 146096]
    era * 146_097 + doe - 719_468
}

/// The smallest block in `[lo, hi]` whose timestamp is `>= target`, or `None`
/// when every block in the range is older than `target` (an empty window).
///
/// Generic over the timestamp lookup so the search itself is testable against a
/// synthetic chain — block times are the one thing a unit test cannot get from
/// a real endpoint cheaply.
pub async fn first_block_at_or_after<F, Fut>(
    lo: u64,
    hi: u64,
    target: u64,
    block_timestamp: F,
) -> Result<Option<u64>>
where
    F: Fn(u64) -> Fut,
    Fut: std::future::Future<Output = Result<u64>>,
{
    if lo > hi {
        return Ok(None);
    }
    if block_timestamp(hi).await? < target {
        return Ok(None);
    }
    let (mut lo, mut hi) = (lo, hi);
    while lo < hi {
        let mid = lo + (hi - lo) / 2;
        if block_timestamp(mid).await? >= target {
            hi = mid;
        } else {
            lo = mid + 1;
        }
    }
    Ok(Some(lo))
}

/// The largest block in `[lo, hi]` whose timestamp is `<= target`, or `None`
/// when even `lo` is newer than `target`.
pub async fn last_block_at_or_before<F, Fut>(
    lo: u64,
    hi: u64,
    target: u64,
    block_timestamp: F,
) -> Result<Option<u64>>
where
    F: Fn(u64) -> Fut,
    Fut: std::future::Future<Output = Result<u64>>,
{
    if lo > hi {
        return Ok(None);
    }
    if block_timestamp(lo).await? > target {
        return Ok(None);
    }
    let (mut lo, mut hi) = (lo, hi);
    while lo < hi {
        let mid = hi - (hi - lo) / 2; // bias up, so `lo` only ever advances
        if block_timestamp(mid).await? <= target {
            lo = mid;
        } else {
            hi = mid - 1;
        }
    }
    Ok(Some(lo))
}

/// Fetch one block's timestamp. Separate from the searches above so they stay
/// chain-free and unit-testable.
pub async fn block_timestamp<P: Provider>(provider: &P, block: u64) -> Result<u64> {
    provider
        .get_block_by_number(block.into())
        .await
        .map_err(|e| SwapError::EthRpc(format!("eth_getBlockByNumber({block}) failed: {e}")))?
        .map(|b| b.header.timestamp)
        .ok_or_else(|| SwapError::EthRpc(format!("block {block} not found")))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn addr(byte: u8) -> Address {
        Address::from([byte; 20])
    }

    fn swap(byte: u8) -> FixedBytes<32> {
        FixedBytes::from([byte; 32])
    }

    fn lock(swap_id: u8, sender: u8, recipient: u8) -> LockedRow {
        LockedRow {
            swap_id: swap(swap_id),
            sender: addr(sender),
            recipient: addr(recipient),
        }
    }

    // The pinned constants are what "which venue did this measure" is anchored
    // to. They must parse, and they must not be the same era.
    #[test]
    fn pinned_venue_constants_are_distinct_and_parseable() {
        let pinned: Address = PINNED_ETH_HTLC_ADDRESS.parse().unwrap();
        let superseded: Address = SUPERSEDED_ETH_HTLC_ADDRESS.parse().unwrap();
        assert_ne!(pinned, superseded);
    }

    // The headline identity: every attempted swap is in exactly one of
    // completed / refunded / still-open, and still-open is what is left.
    #[test]
    fn counts_partition_attempts_into_completed_refunded_and_open() {
        let mut t = SwapTally::new();
        t.record_lock(lock(1, 0xAA, 0x11));
        t.record_lock(lock(2, 0xAA, 0x11));
        t.record_lock(lock(3, 0xBB, 0x11));
        t.record_claim(swap(1));
        t.record_refund(swap(2));
        // swap 3 has no terminal event — still open.

        let c = t.counts();
        assert_eq!(c.attempted, 3);
        assert_eq!(c.completed, 1);
        assert_eq!(c.refunded, 1);
        assert_eq!(c.still_open, 1);
        assert_eq!(c.completed + c.refunded + c.still_open, c.attempted);
    }

    // A taker who got no counterparty and refunded is exactly what the ops
    // ledger cannot see, so it must land squarely in `attempted`.
    #[test]
    fn an_unanswered_taker_still_counts_as_an_attempt() {
        let mut t = SwapTally::new();
        t.record_lock(lock(9, 0xCC, 0x11));
        t.record_refund(swap(9));

        let c = t.counts();
        assert_eq!(c.attempted, 1, "a refunded attempt is still an attempt");
        assert_eq!(c.refunded, 1);
        assert_eq!(c.completed, 0);
        assert_eq!(c.still_open, 0);
    }

    // Chunk boundaries and re-orgs can re-deliver a log; an attempt count that
    // moves when a range is re-scanned is worthless.
    #[test]
    fn duplicate_events_never_inflate_counts() {
        let mut t = SwapTally::new();
        t.record_lock(lock(1, 0xAA, 0x11));
        t.record_lock(lock(1, 0xAA, 0x11));
        t.record_claim(swap(1));
        t.record_claim(swap(1));

        let c = t.counts();
        assert_eq!(c.attempted, 1);
        assert_eq!(c.completed, 1);
        assert_eq!(c.distinct_taker_wallets, 1);
    }

    // Terminal events for a swap locked BEFORE the window (or at another venue)
    // arrive in the same scan; they must not fabricate an attempt.
    #[test]
    fn terminal_event_without_an_in_window_lock_is_ignored() {
        let mut t = SwapTally::new();
        t.record_claim(swap(0x40));
        t.record_refund(swap(0x41));

        let c = t.counts();
        assert_eq!(c.attempted, 0);
        assert_eq!(c.completed, 0);
        assert_eq!(c.refunded, 0);
        assert!(c.by_maker.is_empty());
    }

    // The contract's state machine forbids this, but the identity must hold
    // regardless of what the chain hands back.
    #[test]
    fn claimed_wins_over_refunded_so_the_partition_stays_exact() {
        let mut t = SwapTally::new();
        t.record_lock(lock(1, 0xAA, 0x11));
        t.record_claim(swap(1));
        t.record_refund(swap(1));

        let c = t.counts();
        assert_eq!(c.attempted, 1);
        assert_eq!(c.completed, 1);
        assert_eq!(c.refunded, 0);
        assert_eq!(c.still_open, 0);
    }

    // Distinct senders is a set, not a sum: three locks from two wallets is
    // two, and the number never appears per-maker (a taker who used two makers
    // would be double-counted).
    #[test]
    fn distinct_takers_aggregates_by_wallet_not_by_lock() {
        let mut t = SwapTally::new();
        t.record_lock(lock(1, 0xAA, 0x11));
        t.record_lock(lock(2, 0xAA, 0x22));
        t.record_lock(lock(3, 0xBB, 0x22));

        let c = t.counts();
        assert_eq!(c.attempted, 3);
        assert_eq!(
            c.distinct_taker_wallets, 2,
            "the same wallet across two makers is one wallet"
        );
        // Summing the per-maker attempt counts reproduces the total, which is
        // exactly why the taker count cannot be split the same way.
        assert_eq!(
            c.by_maker.iter().map(|m| m.attempted).sum::<usize>(),
            c.attempted
        );
    }

    #[test]
    fn per_maker_split_is_keyed_on_the_recipient() {
        let mut t = SwapTally::new();
        t.record_lock(lock(1, 0xAA, 0x11));
        t.record_lock(lock(2, 0xBB, 0x11));
        t.record_lock(lock(3, 0xCC, 0x22));
        t.record_claim(swap(1));
        t.record_refund(swap(3));

        let c = t.counts();
        assert_eq!(c.by_maker.len(), 2);
        let m1 = &c.by_maker[0];
        assert_eq!(m1.recipient, addr(0x11).to_string());
        assert_eq!(m1.attempted, 2);
        assert_eq!(m1.completed, 1);
        assert_eq!(m1.still_open, 1);
        let m2 = &c.by_maker[1];
        assert_eq!(m2.recipient, addr(0x22).to_string());
        assert_eq!(m2.attempted, 1);
        assert_eq!(m2.refunded, 1);
    }

    // No taker address may appear anywhere in a serialized report.
    #[test]
    fn serialized_counts_carry_no_taker_address() {
        let mut t = SwapTally::new();
        t.record_lock(lock(1, 0xAA, 0x11));
        let json = serde_json::to_string(&t.counts()).unwrap();

        assert!(!json.contains(&addr(0xAA).to_string()));
        assert!(json.contains(&addr(0x11).to_string()));
        assert!(json.contains("distinct_taker_wallets"));
    }

    #[test]
    fn empty_tally_reports_zeroes() {
        let c = SwapTally::new().counts();
        assert_eq!(c.attempted, 0);
        assert_eq!(c.still_open, 0);
        assert_eq!(c.distinct_taker_wallets, 0);
    }

    // The rule that decides whether a count is publishable. The endpoint this
    // report runs against silently answers `[]` from backends whose log index
    // does not cover the blocks asked for, so an answer must be provable.
    #[test]
    fn an_answer_is_only_trusted_when_every_canary_proves_it() {
        // Both ends of the scan came back intact: this backend's (contiguous)
        // log index spans the whole scan, so its answer counts.
        assert!(is_corroborated(&[(3, 3), (1, 1)]));
        assert!(is_corroborated(&[(4, 3), (1, 1)]));

        // Either end short or missing means the backend's index does not
        // provably cover the scan — retry, do not count.
        assert!(!is_corroborated(&[(0, 3), (1, 1)]));
        assert!(!is_corroborated(&[(3, 3), (0, 1)]));
        assert!(!is_corroborated(&[(2, 3), (1, 1)]));

        // No anchors at all only reaches this function on the explicit
        // `--allow-uncorroborated` opt-in; there is then nothing to verify
        // against, so the answer is taken and flagged (see `scan_decision`).
        assert!(is_corroborated(&[]));
    }

    fn canary_at(block: u64) -> Canary {
        Canary {
            filter: Filter::new().from_block(block).to_block(block),
            expected: 1,
        }
    }

    fn empty_miss(n: usize) -> ProbeMiss {
        ProbeMiss {
            empty_responses: n,
            ..ProbeMiss::default()
        }
    }

    fn failed_miss(n: usize) -> ProbeMiss {
        ProbeMiss {
            failed_requests: n,
            last_error: Some("connection reset by peer".into()),
            ..ProbeMiss::default()
        }
    }

    fn mixed_miss(empty: usize, failed: usize) -> ProbeMiss {
        ProbeMiss {
            empty_responses: empty,
            failed_requests: failed,
            last_error: Some("connection reset by peer".into()),
            ..ProbeMiss::default()
        }
    }

    fn rate_limited_miss(n: usize) -> ProbeMiss {
        ProbeMiss {
            rate_limited: n,
            ..ProbeMiss::default()
        }
    }

    fn throttled_and_empty(empty: usize, throttled: usize) -> ProbeMiss {
        ProbeMiss {
            empty_responses: empty,
            rate_limited: throttled,
            ..ProbeMiss::default()
        }
    }

    fn throttled_and_failed(throttled: usize, failed: usize) -> ProbeMiss {
        ProbeMiss {
            rate_limited: throttled,
            failed_requests: failed,
            last_error: Some("connection reset by peer".into()),
            ..ProbeMiss::default()
        }
    }

    fn partial(missing: ScanEnd, miss: ProbeMiss) -> Anchoring {
        Anchoring::PartiallyAnchored {
            anchored: Box::new(canary_at(7)),
            missing,
            miss,
        }
    }

    // Refusing is the default, and the reason is the whole feature: probes that
    // all came back empty are exactly what a pruned backend produces, and the
    // count published on that basis is a zero that reads as a quiet trial.
    #[test]
    fn a_scan_no_probe_could_anchor_is_refused_unless_the_operator_opts_in() {
        let anchored = Anchoring::Anchored(vec![canary_at(7)]);

        // Probes anchored at both ends: corroboration proceeds, and the opt-in
        // flag does not weaken it.
        assert_eq!(scan_decision(&anchored, false), ScanDecision::Corroborated);
        assert_eq!(scan_decision(&anchored, true), ScanDecision::Corroborated);

        // No probe anywhere produced an anchor: refuse by default...
        let unanchored = Anchoring::Unanchored {
            oldest: empty_miss(80),
            newest: empty_miss(80),
        };
        assert_eq!(scan_decision(&unanchored, false), ScanDecision::Refuse);
        // ...and only an explicit opt-in downgrades that to a flagged count.
        assert_eq!(
            scan_decision(&unanchored, true),
            ScanDecision::ProceedUnproven
        );
    }

    // An end whose probes came back EMPTY says nothing about the endpoint — a
    // log-free block range answers exactly like a pruned index — so this is a
    // question the operator is allowed to answer, at either end and at both.
    #[test]
    fn an_empty_probe_is_ambiguous_so_the_operator_may_override_it() {
        for missing in [ScanEnd::Oldest, ScanEnd::Newest] {
            let half = partial(missing, empty_miss(80));
            assert_eq!(
                scan_decision(&half, false),
                ScanDecision::Refuse,
                "unproven by default, whichever end is missing ({missing:?})"
            );
            assert_eq!(
                scan_decision(&half, true),
                ScanDecision::ProceedUnproven,
                "an empty probe is not evidence the endpoint is at fault ({missing:?})"
            );
        }
    }

    // The invariant, stated three ways. A walk that got even one successful
    // answer — however many of its other requests errored or were throttled —
    // has OBSERVED the chain, ambiguously, and that ambiguity is the
    // operator's to resolve. A walk that got none observed nothing, and no
    // assertion about how quiet the chain is can substitute for an
    // observation, so no flag reaches it.
    #[test]
    fn only_a_walk_that_was_answered_is_the_operators_to_override() {
        let answered = [
            ("all empty", empty_miss(80)),
            ("mostly empty, one error", mixed_miss(79, 1)),
            ("empty and throttled", throttled_and_empty(40, 40)),
        ];
        for (label, miss) in answered {
            assert!(miss.answered_successfully(), "{label}");
            for missing in [ScanEnd::Oldest, ScanEnd::Newest] {
                assert_eq!(
                    scan_decision(&partial(missing, miss.clone()), false),
                    ScanDecision::Refuse,
                    "{label} is still unproven by default"
                );
                assert_eq!(
                    scan_decision(&partial(missing, miss.clone()), true),
                    ScanDecision::ProceedUnproven,
                    "{label} is an observation, so the operator may resolve it"
                );
            }
        }

        let unanswered = [
            ("all errored", failed_miss(80)),
            ("all throttled", rate_limited_miss(80)),
            ("throttled and errored, never answered", throttled_and_failed(40, 40)),
        ];
        for (label, miss) in unanswered {
            assert!(miss.never_answered(), "{label}");
            for missing in [ScanEnd::Oldest, ScanEnd::Newest] {
                assert_eq!(
                    scan_decision(&partial(missing, miss.clone()), false),
                    ScanDecision::Refuse,
                    "{label}"
                );
                assert_eq!(
                    scan_decision(&partial(missing, miss.clone()), true),
                    ScanDecision::Refuse,
                    "{label}: there is no observation for the flag to vouch for"
                );
            }
            assert_eq!(
                scan_decision(
                    &Anchoring::Unanchored {
                        oldest: miss.clone(),
                        newest: miss.clone(),
                    },
                    true
                ),
                ScanDecision::Refuse,
                "{label}"
            );
        }
    }

    // A walk answered by nothing but 429s established nothing about the chain,
    // so it must not be told the chain might be quiet — that suggestion is what
    // walks an operator into passing the flag against a throttling endpoint and
    // publishing whatever a bare unbatched query happens to return.
    #[test]
    fn a_throttled_walk_is_told_it_was_throttled_not_that_the_chain_is_quiet() {
        for anchoring in [
            partial(ScanEnd::Oldest, rate_limited_miss(128)),
            Anchoring::Unanchored {
                oldest: rate_limited_miss(128),
                newest: rate_limited_miss(128),
            },
        ] {
            let msg = refusal_message(100, 200, &anchoring).expect("a refusal");
            assert!(msg.contains("128 were rate-limited"), "{msg}");
            assert!(
                msg.contains("asked for fewer requests"),
                "the throttle must be named for what it is: {msg}"
            );
            assert!(
                !msg.contains("localnet"),
                "a throttled endpoint says nothing about a quiet chain: {msg}"
            );
            assert!(
                !msg.contains("empty log list"),
                "no probe came back with a log list at all: {msg}"
            );
            assert!(
                !msg.contains("ambiguous"),
                "nothing was observed, so nothing is ambiguous: {msg}"
            );
            assert!(
                msg.contains("does not apply here"),
                "the flag must be ruled out: {msg}"
            );
            // The advice must be about the throttle, not about a broken node.
            assert!(msg.contains("narrow the window"), "{msg}");
        }
    }

    // A probe walk that was never answered at all is a hard refusal, and the
    // assertive endpoint wording belongs only to the branch where requests
    // genuinely errored.
    #[test]
    fn an_errored_probe_refuses_even_with_the_opt_in() {
        for missing in [ScanEnd::Oldest, ScanEnd::Newest] {
            let half = partial(missing, failed_miss(80));
            assert_eq!(scan_decision(&half, false), ScanDecision::Refuse);
            assert_eq!(
                scan_decision(&half, true),
                ScanDecision::Refuse,
                "--allow-uncorroborated must not cover an endpoint that would not answer"
            );
        }

        // Same rule when neither end anchored and either end was answered by
        // nothing but errors.
        for (oldest, newest) in [
            (failed_miss(80), empty_miss(80)),
            (empty_miss(80), failed_miss(80)),
            (failed_miss(80), failed_miss(80)),
        ] {
            let unanchored = Anchoring::Unanchored { oldest, newest };
            assert_eq!(scan_decision(&unanchored, true), ScanDecision::Refuse);
        }

        // A walk that saw both throttles and real errors is still the
        // endpoint-at-fault branch: something actually failed.
        let both = refusal_message(1, 2, &partial(ScanEnd::Oldest, throttled_and_failed(4, 76)))
            .expect("a refusal");
        assert!(both.contains("did not answer a single anchor probe"), "{both}");
        assert!(both.contains("4 were rate-limited"), "{both}");
        assert!(both.contains("76 failed"), "{both}");
        assert!(!both.contains("localnet"), "{both}");
    }

    // A downgrade gives up the two-point interval proof, not the evidence: the
    // anchor that WAS found still batches with every query.
    #[test]
    fn a_downgraded_half_anchored_scan_still_batches_its_one_anchor() {
        assert_eq!(partial(ScanEnd::Oldest, empty_miss(80)).canaries().len(), 1);
        assert_eq!(
            Anchoring::Anchored(vec![canary_at(1), canary_at(9)])
                .canaries()
                .len(),
            2
        );
        assert!(
            Anchoring::Unanchored {
                oldest: empty_miss(80),
                newest: empty_miss(80),
            }
            .canaries()
            .is_empty()
        );
    }

    // The refusal has to tell an operator what to do next, and what to do
    // differs by case — so the wordings must not be interchangeable. This
    // command's numbers get read out loud; a message that asserts more than the
    // probes established is the defect, not the phrasing.
    #[test]
    fn each_refusal_claims_only_what_its_probes_established() {
        // Nothing anchored, nothing errored: ambiguous, and the localnet
        // opt-in is the honest suggestion.
        let unanchored = refusal_message(
            100,
            200,
            &Anchoring::Unanchored {
                oldest: empty_miss(80),
                newest: empty_miss(80),
            },
        )
        .expect("an unanchored scan is a refusal");
        assert!(unanchored.contains("100-200"));
        assert!(unanchored.contains("--eth-rpc-url"));
        assert!(unanchored.contains("--allow-uncorroborated"));
        assert!(unanchored.contains("localnet"));

        // Half anchored on empty probes: name the end, own the ambiguity, and
        // do NOT claim a partial log index — the probes cannot show that.
        let ambiguous = refusal_message(100, 200, &partial(ScanEnd::Oldest, empty_miss(80)))
            .expect("a half-anchored scan is a refusal");
        assert!(ambiguous.contains("empty log list"));
        assert!(ambiguous.contains("oldest"));
        assert!(ambiguous.contains("ambiguous"));
        assert!(
            !ambiguous.to_lowercase().contains("partial log index"),
            "an empty probe is not evidence of a partial index: {ambiguous}"
        );
        assert!(
            ambiguous.contains("pass --allow-uncorroborated"),
            "the operator's override must be offered here: {ambiguous}"
        );

        // Errored probes: this one IS about the endpoint, and the override is
        // explicitly ruled out.
        let broken = refusal_message(100, 200, &partial(ScanEnd::Newest, failed_miss(80)))
            .expect("an errored probe is a refusal");
        assert!(broken.contains("newest"));
        assert!(broken.contains("did not answer a single anchor probe"));
        assert!(
            broken.contains("all 80 probe requests failed"),
            "the assertion and the detail must agree: {broken}"
        );
        assert!(
            broken.contains("--allow-uncorroborated cannot cover this"),
            "the override must be ruled out where it does not apply: {broken}"
        );
        assert!(
            !broken.contains("localnet"),
            "an endpoint that errored is not a quiet chain: {broken}"
        );

        // An anchored scan is not a refusal at all.
        assert!(refusal_message(1, 2, &Anchoring::Anchored(vec![canary_at(1)])).is_none());
    }

    // "Never answered" and "answered, and the blocks were empty" send an
    // operator after different things, so a refusal must not report the first
    // as the second.
    #[test]
    fn a_refusal_distinguishes_failed_requests_from_empty_answers() {
        let errored = refusal_message(
            1,
            2,
            &Anchoring::Unanchored {
                oldest: failed_miss(80),
                newest: failed_miss(80),
            },
        )
        .unwrap();
        assert!(errored.contains("failed"));
        assert!(errored.contains("connection reset by peer"));

        let empty = refusal_message(
            1,
            2,
            &Anchoring::Unanchored {
                oldest: empty_miss(80),
                newest: empty_miss(80),
            },
        )
        .unwrap();
        assert!(empty.contains("empty log list"));
        assert!(
            !empty.contains("failed"),
            "no request failed, so nothing may say one did: {empty}"
        );

        // A walk that saw both must say both — and must not claim the endpoint
        // never answered, because 40 requests did.
        let mixed = ProbeMiss {
            empty_responses: 40,
            failed_requests: 40,
            last_error: Some("504 gateway timeout".into()),
            ..ProbeMiss::default()
        };
        assert!(mixed.answered_successfully());
        let described = mixed.describe();
        assert!(described.contains("empty log list"));
        assert!(described.contains("40 failed"));
        assert!(described.contains("504 gateway timeout"));
        assert!(
            !described.contains("all 40 probe requests failed"),
            "'all' is false when 40 were answered: {described}"
        );
    }

    // `scan_range` derives every `eth_getLogs` span from this helper, so these
    // are the block boundaries the report really queries.
    #[test]
    fn block_chunks_covers_the_range_exactly_once() {
        assert_eq!(block_chunks(10, 19, 10), vec![(10, 19)]);
        assert_eq!(block_chunks(10, 20, 10), vec![(10, 19), (20, 20)]);
        assert_eq!(block_chunks(5, 5, 100), vec![(5, 5)]);
        assert_eq!(block_chunks(5, 4, 100), Vec::new());
        assert_eq!(block_chunks(0, 4, 2), vec![(0, 1), (2, 3), (4, 4)]);
        // A zero chunk would loop forever; it is clamped to one block.
        assert_eq!(block_chunks(1, 2, 0), vec![(1, 1), (2, 2)]);

        // Whatever the chunk size, the spans are contiguous and cover [from,to].
        for chunk in [1u64, 3, 7, 1000] {
            let spans = block_chunks(100, 137, chunk);
            assert_eq!(spans.first().unwrap().0, 100);
            assert_eq!(spans.last().unwrap().1, 137);
            for w in spans.windows(2) {
                assert_eq!(w[0].1 + 1, w[1].0);
            }
        }
    }

    // A synthetic chain with 12s blocks: block n has timestamp 1_000 + 12n.
    async fn synthetic_ts(block: u64) -> Result<u64> {
        Ok(1_000 + 12 * block)
    }

    #[tokio::test]
    async fn first_block_at_or_after_finds_the_boundary() {
        // Exactly on a block boundary → that block.
        assert_eq!(
            first_block_at_or_after(0, 100, 1_120, synthetic_ts)
                .await
                .unwrap(),
            Some(10)
        );
        // Between blocks → the first one at or after.
        assert_eq!(
            first_block_at_or_after(0, 100, 1_121, synthetic_ts)
                .await
                .unwrap(),
            Some(11)
        );
        // Before the range starts → the range's own start.
        assert_eq!(
            first_block_at_or_after(5, 100, 0, synthetic_ts)
                .await
                .unwrap(),
            Some(5)
        );
        // After the range's newest block → nothing in range.
        assert_eq!(
            first_block_at_or_after(0, 100, 999_999, synthetic_ts)
                .await
                .unwrap(),
            None
        );
        assert_eq!(
            first_block_at_or_after(10, 9, 1_000, synthetic_ts)
                .await
                .unwrap(),
            None
        );
    }

    #[tokio::test]
    async fn last_block_at_or_before_finds_the_boundary() {
        assert_eq!(
            last_block_at_or_before(0, 100, 1_120, synthetic_ts)
                .await
                .unwrap(),
            Some(10)
        );
        assert_eq!(
            last_block_at_or_before(0, 100, 1_131, synthetic_ts)
                .await
                .unwrap(),
            Some(10)
        );
        // Newer than the whole range → the range's own end.
        assert_eq!(
            last_block_at_or_before(0, 100, 999_999, synthetic_ts)
                .await
                .unwrap(),
            Some(100)
        );
        // Older than the range's first block → nothing in range.
        assert_eq!(
            last_block_at_or_before(50, 100, 1_000, synthetic_ts)
                .await
                .unwrap(),
            None
        );
    }

    // The two searches must agree on a window: [since, until] with since ==
    // until on a block boundary is exactly that one block.
    #[tokio::test]
    async fn since_and_until_agree_on_a_single_block_window() {
        let from = first_block_at_or_after(0, 100, 1_120, synthetic_ts)
            .await
            .unwrap()
            .unwrap();
        let to = last_block_at_or_before(0, 100, 1_131, synthetic_ts)
            .await
            .unwrap()
            .unwrap();
        assert_eq!((from, to), (10, 10));
    }

    #[test]
    fn parse_time_arg_accepts_unix_dates_and_datetimes() {
        assert_eq!(parse_time_arg("0").unwrap(), 0);
        assert_eq!(parse_time_arg("1755734400").unwrap(), 1_755_734_400);
        // 2026-08-21T00:00:00Z — cross-checked against the epoch-day formula.
        assert_eq!(parse_time_arg("2026-08-21").unwrap(), 1_787_270_400);
        assert_eq!(
            parse_time_arg("2026-08-21T00:00:00Z").unwrap(),
            1_787_270_400
        );
        assert_eq!(
            parse_time_arg("2026-08-21T12:30:45").unwrap(),
            1_787_270_400 + 12 * 3600 + 30 * 60 + 45
        );
        assert_eq!(parse_time_arg("1970-01-01").unwrap(), 0);
        assert_eq!(parse_time_arg(" 2026-08-21 ").unwrap(), 1_787_270_400);
    }

    // A date that does not exist must fail loudly. `days_from_civil` is pure
    // arithmetic: fed 2026-02-30 it returns the epoch day of 2026-03-02, so an
    // unguarded parse silently measures a different trial window and prints the
    // shifted one as if it were what was asked for.
    #[test]
    fn parse_time_arg_rejects_impossible_calendar_dates() {
        for bad in [
            "2026-02-29", // 2026 is not a leap year
            "2026-02-30",
            "2026-04-31",
            "2026-06-31",
            "2026-09-31",
            "2026-11-31",
            "2026-02-31T12:00:00",
        ] {
            assert!(
                parse_time_arg(bad).is_err(),
                "'{bad}' is not a real date and must not parse"
            );
        }

        // Real leap days still parse, and land where the calendar says.
        assert_eq!(
            parse_time_arg("2024-02-29").unwrap(),
            parse_time_arg("2024-02-28").unwrap() + 86_400
        );
        assert_eq!(
            parse_time_arg("2024-03-01").unwrap(),
            parse_time_arg("2024-02-29").unwrap() + 86_400
        );
        // ...and a non-leap February ends where it should.
        assert_eq!(
            parse_time_arg("2026-03-01").unwrap(),
            parse_time_arg("2026-02-28").unwrap() + 86_400
        );
    }

    #[test]
    fn parse_time_arg_rejects_junk() {
        for bad in [
            "",
            "yesterday",
            "2026-13-01",
            "2026-08-32",
            "2026-08",
            "2026-08-21T25:00:00",
            "2026-08-21T12:60",
            "1969-12-31",
        ] {
            assert!(
                parse_time_arg(bad).is_err(),
                "'{bad}' should not parse as a time"
            );
        }
    }

    // The epoch-day helper is the only calendar arithmetic in the repo; leap
    // years and century rules are exactly where a hand-rolled version breaks.
    #[test]
    fn days_from_civil_handles_leap_and_century_rules() {
        assert_eq!(days_from_civil(1970, 1, 1), 0);
        assert_eq!(days_from_civil(1970, 12, 31), 364);
        assert_eq!(days_from_civil(1972, 2, 29), 789); // a leap day
        assert_eq!(days_from_civil(2000, 3, 1), 11017); // 2000 IS a leap year
        assert_eq!(days_from_civil(2100, 3, 1), 47541); // 2100 is NOT
        assert_eq!(days_from_civil(2026, 8, 21), 1_787_270_400 / 86_400);
    }
}
