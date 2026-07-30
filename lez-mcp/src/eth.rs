//! Read-only Sepolia EthHTLC access. No ETH key required — the provider has
//! no wallet; only `eth_call` / `eth_getLogs` are used.

use std::future::Future;

use alloy::{
    primitives::{Address, FixedBytes},
    providers::{DynProvider, Provider, ProviderBuilder, WsConnect},
    rpc::types::Filter,
    sol_types::SolEvent as _,
};
use swap_orchestrator::eth::client::EthHTLC;
use tokio::sync::Mutex;

/// Max block span per `eth_getLogs` call (public providers cap ranges).
const LOG_SCAN_CHUNK: u64 = 40_000;

/// Per-call `eth_getLogs` request budget. A single `sepolia_htlc_status` call
/// scans at most this many chunks (`MAX_CHUNKS_PER_CALL * LOG_SCAN_CHUNK`
/// blocks) before returning. Progress is cached, so a repeated call resumes
/// where the last left off instead of rescanning — an unknown hashlock can no
/// longer be replayed to exhaust the provider's rate/window limits.
const MAX_CHUNKS_PER_CALL: u64 = 16;

/// On each scan we rewind the resume point by this many blocks and re-read the
/// tail, so a chain reorg near the previous head cannot leave orphaned events
/// cached (nor permanently skip a canonical block that a reorg introduced).
/// Sepolia reorgs are shallow; 64 blocks is a comfortable safety margin.
const REORG_SAFETY_WINDOW: u64 = 64;

/// A `Locked` event we have already seen, remembered across calls so repeated
/// scans never re-read the same range. Identity for deduplication is
/// `(tx_hash, log_index)`; `block` places it for reorg-window replacement.
#[derive(Clone, Copy)]
struct SeenLock {
    swap_id: [u8; 32],
    hashlock: [u8; 32],
    block: u64,
    tx_hash: [u8; 32],
    log_index: u64,
}

/// Lowest block to re-scan this call: rewind `next_block` by `window`, but never
/// below `from_block` (the deployment block / configured floor).
fn rewind_start(next_block: u64, from_block: u64, window: u64) -> u64 {
    next_block.saturating_sub(window).max(from_block)
}

/// Replace every cached event in the (re-scanned) range `[start, end]` with the
/// `fresh` reads for that range. Cached events in the range that are absent from
/// `fresh` are dropped (reorg orphans); `fresh` events are added, deduped
/// against survivors by `(tx_hash, log_index)` so overlapping windows across
/// calls never double-count.
fn reconcile_range(locked: &mut Vec<SeenLock>, start: u64, end: u64, fresh: Vec<SeenLock>) {
    // Drop everything in the rescanned range — orphans included.
    locked.retain(|l| l.block < start || l.block > end);
    for f in fresh {
        let dup = locked
            .iter()
            .any(|l| l.tx_hash == f.tx_hash && l.log_index == f.log_index);
        if !dup {
            locked.push(f);
        }
    }
}

/// Incremental scan state. `next_block` is the lowest block not yet scanned;
/// `locked` accumulates every `Locked` event discovered so far.
struct ScanCache {
    next_block: u64,
    locked: Vec<SeenLock>,
}

/// Handle a chain head that has REGRESSED below the cache's high-water mark
/// (provider lag/failover, or a reorg deeper than [`REORG_SAFETY_WINDOW`]).
/// Cached events above `latest` are no longer known-canonical: prune them and
/// pull `next_block` back to `latest + 1` so the tail is genuinely re-scanned,
/// instead of keeping orphaned events above head and reporting
/// `reached_head = true` over them.
fn prune_head_regression(cache: &mut ScanCache, latest: u64, from_block: u64) {
    let high_water = cache.next_block.saturating_sub(1);
    if latest < high_water {
        cache.locked.retain(|l| l.block <= latest);
        cache.next_block = latest.saturating_add(1).max(from_block);
    }
}

/// What a completed (or budget-exhausted) scan pass yields, before the
/// per-swap-id state fetches.
struct ScanPass {
    /// Distinct swap ids whose `Locked` event carries the queried hashlock.
    matched: Vec<[u8; 32]>,
    /// Highest block fully scanned across all calls.
    coverage_to: u64,
    reached_head: bool,
}

/// Drive one bounded, incremental scan pass, reading the chain head via
/// `read_head` and `Locked` events for each `[start, end]` range via `fetch`.
///
/// The ENTIRE pass — chain-head read, head-regression pruning, rewind, chunked
/// fetches, reconciliation, resume-point commits, and match extraction — runs
/// under the cache's async lock, so concurrent `sepolia_htlc_status` calls
/// serialize: a slower, older scan can never interleave with (and reconcile
/// stale reads over) a newer one; queued callers simply resume from the
/// refreshed cache.
///
/// [P2-3] `read_head` is invoked INSIDE the lock (not passed in as a value read
/// beforehand): head/prune/reconcile/commit must be one atomic unit per pass.
/// A head read outside the lock let a stalled call resume later carrying a
/// stale (older) head and run `prune_head_regression` against a cache a newer
/// pass had already advanced past — deleting legitimate progress and events.
async fn run_scan<F, Fut, H, HFut>(
    cache: &Mutex<ScanCache>,
    from_block: u64,
    read_head: H,
    hashlock: [u8; 32],
    fetch: F,
) -> Result<ScanPass, String>
where
    F: Fn(u64, u64) -> Fut,
    Fut: Future<Output = Result<Vec<SeenLock>, String>>,
    H: FnOnce() -> HFut,
    HFut: Future<Output = Result<u64, String>>,
{
    let mut cache = cache.lock().await;

    // [P2-3] Read the chain head UNDER the scan lock, immediately before the
    // regression check that consumes it. This binds the head to the cache
    // snapshot it is compared against, so a stalled/queued call cannot prune a
    // newer cache with an older head.
    let latest = read_head().await?;

    // If head regressed below our high-water mark, drop now-unverifiable
    // cached events above it and rewind before scanning.
    prune_head_regression(&mut cache, latest, from_block);

    // Rewind the resume point by the safety window to re-scan a possibly
    // reorged tail (clamped to the deployment/floor block).
    let mut start = rewind_start(cache.next_block, from_block, REORG_SAFETY_WINDOW);
    let mut chunks_used = 0u64;

    while start <= latest && chunks_used < MAX_CHUNKS_PER_CALL {
        let end = (start + LOG_SCAN_CHUNK - 1).min(latest);
        let fresh = fetch(start, end).await?;

        // Replace this chunk's cached events with the fresh reads (drops
        // reorg orphans) and advance the resume point.
        reconcile_range(&mut cache.locked, start, end, fresh);
        if end + 1 > cache.next_block {
            cache.next_block = end + 1;
        }

        chunks_used += 1;
        start = end + 1;
    }

    // Recompute matches from the reconciled cache (so a reorged-away match
    // is not resurrected) and the coverage high-water mark.
    let mut matched: Vec<[u8; 32]> = Vec::new();
    for l in &cache.locked {
        if l.hashlock == hashlock && !matched.contains(&l.swap_id) {
            matched.push(l.swap_id);
        }
    }
    // next_block - 1 is the highest block fully scanned across all calls.
    let coverage_to = cache.next_block.saturating_sub(1);
    Ok(ScanPass {
        matched,
        coverage_to,
        reached_head: coverage_to >= latest,
    })
}

pub struct EthReader {
    contract: EthHTLC::EthHTLCInstance<DynProvider>,
    from_block: u64,
    cache: Mutex<ScanCache>,
}

pub struct FoundHtlc {
    pub swap_id: [u8; 32],
    pub htlc: EthHTLC::HTLC,
}

/// Outcome of a bounded hashlock scan.
pub struct ScanOutcome {
    pub found: Vec<FoundHtlc>,
    /// Lowest block covered by the cumulative scan (the deployment block).
    pub coverage_from: u64,
    /// Highest block covered so far across all calls.
    pub coverage_to: u64,
    /// True once the scan has reached chain head; false when this call hit its
    /// request budget first (caller may retry to continue from `coverage_to`).
    pub reached_head: bool,
}

impl EthReader {
    pub async fn connect(rpc_url: &str, address: &str, from_block: u64) -> Result<Self, String> {
        let address: Address = address
            .parse()
            .map_err(|e| format!("invalid ETH_HTLC_ADDRESS: {e}"))?;
        let provider = ProviderBuilder::new()
            .connect_ws(WsConnect::new(rpc_url))
            .await
            .map_err(|e| format!("ETH WebSocket connect failed: {e}"))?
            .erased();
        Ok(Self {
            contract: EthHTLC::new(address, provider),
            from_block,
            cache: Mutex::new(ScanCache {
                next_block: from_block,
                locked: Vec::new(),
            }),
        })
    }

    pub fn address(&self) -> Address {
        *self.contract.address()
    }

    /// Read the HTLC record for a swap id (EMPTY state = not found).
    pub async fn htlc_by_swap_id(&self, swap_id: [u8; 32]) -> Result<EthHTLC::HTLC, String> {
        self.contract
            .getHTLC(FixedBytes(swap_id))
            .call()
            .await
            .map_err(|e| format!("getHTLC call failed: {e}"))
    }

    /// Resolve a hashlock to its HTLC(s) by scanning `Locked` events. Bounded,
    /// incremental, serialized, and reorg-safe:
    /// - The whole scan→reconcile→commit pass runs under the cache's async lock
    ///   (see [`run_scan`]), so concurrent calls cannot run overlapping getLogs
    ///   sweeps where a stale older scan reconciles over newer canonical state —
    ///   queued callers await and then resume from the refreshed cache.
    /// - Each call rewinds the resume point by [`REORG_SAFETY_WINDOW`] blocks
    ///   and re-reads that tail, replacing the cached events in the rewound
    ///   range with fresh reads (deduped by `(tx_hash, log_index)`). A reorg can
    ///   therefore neither strand an orphaned event in the cache nor permanently
    ///   skip a canonical block introduced by the reorg.
    /// - A head BELOW the cached high-water mark (lagging/failed-over provider,
    ///   deep reorg) prunes cached events above it and rewinds, rather than
    ///   reporting `reached_head` over orphaned state.
    /// - Scan at most `MAX_CHUNKS_PER_CALL` chunks per call (the request
    ///   budget); progress is cached so a follow-up call resumes rather than
    ///   restarts. When the budget is hit before head, `reached_head` is false.
    /// - Matches are recomputed from the reconciled cache after scanning, so a
    ///   reorged-away match is not reported from stale cache.
    pub async fn find_by_hashlock(&self, hashlock: [u8; 32]) -> Result<ScanOutcome, String> {
        let provider = self.contract.provider();
        let address = self.address();

        // [P2-3] Defer the head read into `run_scan` so it happens under the
        // scan lock, atomically with prune/reconcile/commit.
        let read_head = || async {
            provider
                .get_block_number()
                .await
                .map_err(|e| format!("eth_blockNumber failed: {e}"))
        };
        let fetch = |start: u64, end: u64| {
            let provider = provider.clone();
            async move {
                let filter = Filter::new()
                    .address(address)
                    .event_signature(EthHTLC::Locked::SIGNATURE_HASH)
                    .from_block(start)
                    .to_block(end);
                let logs = provider
                    .get_logs(&filter)
                    .await
                    .map_err(|e| format!("eth_getLogs({start}..{end}) failed: {e}"))?;

                let mut fresh = Vec::new();
                for log in logs {
                    // Positional metadata for reorg reconciliation / dedup. Present
                    // on any confirmed getLogs result; fall back defensively.
                    let block = log.block_number.unwrap_or(start);
                    let tx_hash = log.transaction_hash.map(|h| h.0).unwrap_or([0u8; 32]);
                    let log_index = log.log_index.unwrap_or(0);
                    if let Ok(decoded) = log.log_decode::<EthHTLC::Locked>() {
                        let event = &decoded.inner.data;
                        fresh.push(SeenLock {
                            swap_id: event.swapId.0,
                            hashlock: event.hashlock.0,
                            block,
                            tx_hash,
                            log_index,
                        });
                    }
                }
                Ok(fresh)
            }
        };

        let pass = run_scan(&self.cache, self.from_block, read_head, hashlock, fetch).await?;

        // Fetch current on-chain state for each matched swap id (outside the
        // scan lock — these are per-id eth_calls that no longer touch the cache).
        let mut found = Vec::with_capacity(pass.matched.len());
        for swap_id in pass.matched {
            let htlc = self.htlc_by_swap_id(swap_id).await?;
            found.push(FoundHtlc { swap_id, htlc });
        }

        Ok(ScanOutcome {
            found,
            coverage_from: self.from_block,
            coverage_to: pass.coverage_to,
            reached_head: pass.reached_head,
        })
    }
}

/// Human-readable swap state.
pub fn state_str(state: &EthHTLC::SwapState) -> &'static str {
    match state {
        EthHTLC::SwapState::EMPTY => "EMPTY",
        EthHTLC::SwapState::OPEN => "OPEN",
        EthHTLC::SwapState::CLAIMED => "CLAIMED",
        EthHTLC::SwapState::REFUNDED => "REFUNDED",
        _ => "UNKNOWN",
    }
}

/// Parse a 0x-optional 64-char hex string into 32 bytes.
pub fn parse_bytes32(s: &str) -> Result<[u8; 32], String> {
    let hex_str = s.trim().strip_prefix("0x").unwrap_or(s.trim());
    let bytes = hex::decode(hex_str).map_err(|e| format!("invalid hex: {e}"))?;
    bytes
        .try_into()
        .map_err(|_| "expected exactly 32 bytes (64 hex chars)".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_bytes32_accepts_with_and_without_prefix() {
        let h = "ecca22673f2bba423a5689cfaf6d3f6d34dc076a57a8747b21f64714e06cee21";
        let a = parse_bytes32(h).unwrap();
        let b = parse_bytes32(&format!("0x{h}")).unwrap();
        assert_eq!(a, b);
        assert_eq!(hex::encode(a), h);
    }

    #[test]
    fn parse_bytes32_rejects_bad_input() {
        assert!(parse_bytes32("0x1234").is_err());
        assert!(parse_bytes32("zz").is_err());
        assert!(parse_bytes32("").is_err());
    }

    fn lock(swap: u8, hash: u8, block: u64, tx: u8, idx: u64) -> SeenLock {
        SeenLock {
            swap_id: [swap; 32],
            hashlock: [hash; 32],
            block,
            tx_hash: [tx; 32],
            log_index: idx,
        }
    }

    #[test]
    fn rewind_start_clamps_to_from_block() {
        // Normal rewind by the window.
        assert_eq!(rewind_start(1000, 500, 64), 936);
        // Rewinding past the floor clamps to from_block.
        assert_eq!(rewind_start(520, 500, 64), 500);
        assert_eq!(rewind_start(500, 500, 64), 500);
        // Saturating: never underflows.
        assert_eq!(rewind_start(10, 0, 64), 0);
    }

    #[test]
    fn reconcile_drops_reorg_orphans_and_reads_canonical() {
        // Cache: a stable event below the rewound range, and one inside it that
        // a reorg has since orphaned (no longer on-chain).
        let mut cache = vec![
            lock(1, 1, 900, 0xAA, 0),  // stable, below the rescanned range
            lock(2, 2, 1000, 0xBB, 0), // orphaned by the reorg
        ];
        // Fresh reads for [950, 1010]: the canonical chain now has a different
        // event at 1000 (a block a reorg introduced / replaced).
        let fresh = vec![lock(3, 3, 1000, 0xCC, 0)];
        reconcile_range(&mut cache, 950, 1010, fresh);

        assert!(
            cache.iter().any(|l| l.block == 900 && l.tx_hash == [0xAA; 32]),
            "event below the rewound range is untouched"
        );
        assert!(
            !cache.iter().any(|l| l.tx_hash == [0xBB; 32]),
            "reorg orphan inside the rewound range is dropped"
        );
        assert!(
            cache.iter().any(|l| l.tx_hash == [0xCC; 32]),
            "canonical replacement is read in"
        );
        assert_eq!(cache.len(), 2);
    }

    #[test]
    fn reconcile_is_idempotent_and_dedupes_by_tx_and_index() {
        let mut cache = vec![lock(1, 1, 1000, 0xAA, 0)];
        let fresh = vec![lock(1, 1, 1000, 0xAA, 0)];
        // Re-scanning the same window twice must not duplicate the event.
        reconcile_range(&mut cache, 950, 1010, fresh.clone());
        reconcile_range(&mut cache, 950, 1010, fresh);
        assert_eq!(
            cache.len(),
            1,
            "same (tx_hash, log_index) must not be double-counted across re-scans"
        );

        // Two logs in the same tx are distinct by log_index.
        let mut multi = Vec::new();
        reconcile_range(
            &mut multi,
            950,
            1010,
            vec![lock(4, 4, 1000, 0xDD, 0), lock(5, 5, 1000, 0xDD, 1)],
        );
        assert_eq!(multi.len(), 2, "distinct log_index in one tx are separate events");
    }

    // ── [P2-6] head-regression pruning ──────────────────────────────────

    #[test]
    fn head_regression_prunes_orphans_and_rewinds() {
        let mut cache = ScanCache {
            next_block: 2001, // high-water mark 2000
            locked: vec![
                lock(1, 1, 1500, 0xAA, 0), // below the regressed head — kept
                lock(2, 2, 1990, 0xBB, 0), // above it — orphaned, must go
            ],
        };
        prune_head_regression(&mut cache, 1900, 100);
        assert_eq!(cache.next_block, 1901, "resume point pulled back to latest+1");
        assert_eq!(cache.locked.len(), 1, "events above the regressed head pruned");
        assert_eq!(cache.locked[0].block, 1500);
    }

    #[test]
    fn head_regression_noop_when_head_at_or_above_high_water() {
        let mut cache = ScanCache {
            next_block: 2001,
            locked: vec![lock(1, 1, 1990, 0xAA, 0)],
        };
        // Head equal to the high-water mark: nothing to prune.
        prune_head_regression(&mut cache, 2000, 100);
        assert_eq!(cache.next_block, 2001);
        assert_eq!(cache.locked.len(), 1);
        // Head above: also untouched.
        prune_head_regression(&mut cache, 5000, 100);
        assert_eq!(cache.next_block, 2001);
        assert_eq!(cache.locked.len(), 1);
    }

    #[test]
    fn head_regression_never_rewinds_below_from_block() {
        let mut cache = ScanCache {
            next_block: 600,
            locked: vec![lock(1, 1, 550, 0xAA, 0)],
        };
        // Head regressed below even the deployment floor.
        prune_head_regression(&mut cache, 400, 500);
        assert_eq!(cache.next_block, 500, "clamped to from_block");
        assert!(cache.locked.is_empty());
    }

    #[tokio::test]
    async fn run_scan_after_head_regression_rescans_and_drops_orphans() {
        // Pass 1: head 2000, one Locked event at block 1990.
        let cache = Mutex::new(ScanCache {
            next_block: 0,
            locked: Vec::new(),
        });
        let hashlock = [7u8; 32];
        let pass1 = run_scan(
            &cache,
            0,
            || async { Ok(2000) },
            hashlock,
            |start, end| async move {
                Ok(if (start..=end).contains(&1990) {
                    vec![lock(9, 7, 1990, 0xAA, 0)]
                } else {
                    vec![]
                })
            },
        )
        .await
        .unwrap();
        assert_eq!(pass1.matched.len(), 1);
        assert!(pass1.reached_head);

        // Pass 2: the provider's head has REGRESSED to 1900 and the canonical
        // chain no longer contains the event. The orphan above head must not be
        // reported, and reached_head must describe the regressed head honestly.
        let pass2 = run_scan(
            &cache,
            0,
            || async { Ok(1900) },
            hashlock,
            |_, _| async move { Ok(vec![]) },
        )
        .await
        .unwrap();
        assert!(
            pass2.matched.is_empty(),
            "orphaned event above the regressed head must not survive"
        );
        assert_eq!(pass2.coverage_to, 1900);
        assert!(pass2.reached_head);
    }

    // ── [P2-5] scan serialization ───────────────────────────────────────

    // Two concurrent scans over one cache must not run overlapping fetches:
    // the whole scan→reconcile→commit pass holds the cache's async lock, so a
    // stale older sweep can never reconcile over newer canonical state. The
    // slow mock fetch would overlap without the lock.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn concurrent_scans_serialize_and_share_the_cache() {
        use std::sync::Arc;
        use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
        use std::time::Duration;

        let latest = LOG_SCAN_CHUNK * 2; // a few chunks per pass
        let cache = Arc::new(Mutex::new(ScanCache {
            next_block: 0,
            locked: Vec::new(),
        }));
        let in_fetch = Arc::new(AtomicUsize::new(0));
        let overlapped = Arc::new(AtomicBool::new(false));
        let fetches = Arc::new(AtomicUsize::new(0));

        let mut handles = Vec::new();
        for _ in 0..2 {
            let cache = cache.clone();
            let in_fetch = in_fetch.clone();
            let overlapped = overlapped.clone();
            let fetches = fetches.clone();
            handles.push(tokio::spawn(async move {
                run_scan(&cache, 0, move || async move { Ok(latest) }, [1u8; 32], move |_start, _end| {
                    let in_fetch = in_fetch.clone();
                    let overlapped = overlapped.clone();
                    let fetches = fetches.clone();
                    async move {
                        if in_fetch.fetch_add(1, Ordering::SeqCst) > 0 {
                            overlapped.store(true, Ordering::SeqCst);
                        }
                        fetches.fetch_add(1, Ordering::SeqCst);
                        tokio::time::sleep(Duration::from_millis(10)).await;
                        in_fetch.fetch_sub(1, Ordering::SeqCst);
                        Ok(vec![])
                    }
                })
                .await
                .unwrap()
            }));
        }
        for h in handles {
            let pass = h.await.unwrap();
            assert!(pass.reached_head, "both callers see the fully-scanned cache");
            assert_eq!(pass.coverage_to, latest);
        }
        assert!(
            !overlapped.load(Ordering::SeqCst),
            "scan passes must serialize — no two getLogs sweeps in flight at once"
        );
        // The queued pass resumes from the refreshed cache: it only re-reads
        // the rewind tail (1 chunk), not the whole range again.
        assert!(
            fetches.load(Ordering::SeqCst) < 6,
            "second pass must resume from the shared cache, not rescan from zero \
             (saw {} fetches)",
            fetches.load(Ordering::SeqCst)
        );
        assert_eq!(
            cache.lock().await.next_block,
            latest + 1,
            "both passes committed into the one shared cache"
        );
    }

    // ── [P2-3] head read is serialized with the scan pass ───────────────
    //
    // The chain-head read must happen UNDER the scan lock. If it ran before
    // the lock, a queued second pass would read head while the first pass is
    // still mid-fetch (holding the lock) — exactly the interleaving that let a
    // stale head prune a newer cache. We instrument `read_head` to flag if any
    // fetch is in flight when it runs; with the read serialized under the lock
    // the flag must stay clear.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn head_read_runs_under_the_scan_lock() {
        use std::sync::Arc;
        use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
        use std::time::Duration;

        let latest = LOG_SCAN_CHUNK * 2;
        let cache = Arc::new(Mutex::new(ScanCache {
            next_block: 0,
            locked: Vec::new(),
        }));
        let in_fetch = Arc::new(AtomicUsize::new(0));
        let head_during_fetch = Arc::new(AtomicBool::new(false));

        let mut handles = Vec::new();
        for _ in 0..2 {
            let cache = cache.clone();
            let in_fetch = in_fetch.clone();
            let head_during_fetch = head_during_fetch.clone();
            handles.push(tokio::spawn(async move {
                let in_fetch_head = in_fetch.clone();
                let head_during_fetch = head_during_fetch.clone();
                let read_head = move || async move {
                    // A fetch in flight here means another pass holds the lock
                    // while we read head → the read escaped the lock.
                    if in_fetch_head.load(Ordering::SeqCst) > 0 {
                        head_during_fetch.store(true, Ordering::SeqCst);
                    }
                    Ok(latest)
                };
                let in_fetch = in_fetch.clone();
                run_scan(&cache, 0, read_head, [1u8; 32], move |_s, _e| {
                    let in_fetch = in_fetch.clone();
                    async move {
                        in_fetch.fetch_add(1, Ordering::SeqCst);
                        tokio::time::sleep(Duration::from_millis(10)).await;
                        in_fetch.fetch_sub(1, Ordering::SeqCst);
                        Ok(vec![])
                    }
                })
                .await
                .unwrap()
            }));
        }
        for h in handles {
            h.await.unwrap();
        }
        assert!(
            !head_during_fetch.load(Ordering::SeqCst),
            "chain-head read must run under the scan lock, never while another \
             pass is mid-fetch"
        );
    }
}
