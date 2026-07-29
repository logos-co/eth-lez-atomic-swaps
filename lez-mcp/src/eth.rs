//! Read-only Sepolia EthHTLC access. No ETH key required — the provider has
//! no wallet; only `eth_call` / `eth_getLogs` are used.

use std::sync::Mutex;

use alloy::{
    primitives::{Address, FixedBytes},
    providers::{DynProvider, Provider, ProviderBuilder, WsConnect},
    rpc::types::Filter,
    sol_types::SolEvent as _,
};
use swap_orchestrator::eth::client::EthHTLC;

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
    /// incremental, and reorg-safe:
    /// - Each call rewinds the resume point by [`REORG_SAFETY_WINDOW`] blocks
    ///   and re-reads that tail, replacing the cached events in the rewound
    ///   range with fresh reads (deduped by `(tx_hash, log_index)`). A reorg can
    ///   therefore neither strand an orphaned event in the cache nor permanently
    ///   skip a canonical block introduced by the reorg.
    /// - Scan at most `MAX_CHUNKS_PER_CALL` chunks per call (the request
    ///   budget); progress is cached so a follow-up call resumes rather than
    ///   restarts. When the budget is hit before head, `reached_head` is false.
    /// - Matches are recomputed from the reconciled cache after scanning, so a
    ///   reorged-away match is not reported from stale cache.
    pub async fn find_by_hashlock(&self, hashlock: [u8; 32]) -> Result<ScanOutcome, String> {
        let provider = self.contract.provider();
        let latest = provider
            .get_block_number()
            .await
            .map_err(|e| format!("eth_blockNumber failed: {e}"))?;

        // Rewind the resume point by the safety window to re-scan a possibly
        // reorged tail (clamped to the deployment/floor block).
        let mut start = {
            let cache = self.cache.lock().expect("scan cache poisoned");
            rewind_start(cache.next_block, self.from_block, REORG_SAFETY_WINDOW)
        };

        let mut chunks_used = 0u64;

        while start <= latest && chunks_used < MAX_CHUNKS_PER_CALL {
            let end = (start + LOG_SCAN_CHUNK - 1).min(latest);
            let filter = Filter::new()
                .address(self.address())
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

            // Replace this chunk's cached events with the fresh reads (drops
            // reorg orphans) and advance the resume point.
            {
                let mut cache = self.cache.lock().expect("scan cache poisoned");
                reconcile_range(&mut cache.locked, start, end, fresh);
                if end + 1 > cache.next_block {
                    cache.next_block = end + 1;
                }
            }

            chunks_used += 1;
            start = end + 1;
        }

        // Recompute matches from the reconciled cache (so a reorged-away match
        // is not resurrected) and the coverage high-water mark.
        let (matched_ids, coverage_to) = {
            let cache = self.cache.lock().expect("scan cache poisoned");
            let mut matched: Vec<[u8; 32]> = Vec::new();
            for l in &cache.locked {
                if l.hashlock == hashlock && !matched.contains(&l.swap_id) {
                    matched.push(l.swap_id);
                }
            }
            // next_block - 1 is the highest block fully scanned across all calls.
            (matched, cache.next_block.saturating_sub(1))
        };
        let reached_head = coverage_to >= latest;

        // Fetch current on-chain state for each matched swap id.
        let mut found = Vec::with_capacity(matched_ids.len());
        for swap_id in matched_ids {
            let htlc = self.htlc_by_swap_id(swap_id).await?;
            found.push(FoundHtlc { swap_id, htlc });
        }

        Ok(ScanOutcome {
            found,
            coverage_from: self.from_block,
            coverage_to,
            reached_head,
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
}
