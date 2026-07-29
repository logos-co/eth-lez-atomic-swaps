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

/// A `Locked` event we have already seen, remembered across calls so repeated
/// scans never re-read the same range.
#[derive(Clone, Copy)]
struct SeenLock {
    swap_id: [u8; 32],
    hashlock: [u8; 32],
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

    /// Resolve a hashlock to its HTLC(s) by scanning `Locked` events. Bounded
    /// and incremental:
    /// - First serve any already-cached matches, then scan only the delta
    ///   `[next_block, head]` this process has not seen yet.
    /// - Scan at most `MAX_CHUNKS_PER_CALL` chunks per call (the request
    ///   budget); cache the progress so a follow-up call resumes rather than
    ///   restarts. When the budget is hit before head, `reached_head` is false.
    pub async fn find_by_hashlock(&self, hashlock: [u8; 32]) -> Result<ScanOutcome, String> {
        let provider = self.contract.provider();
        let latest = provider
            .get_block_number()
            .await
            .map_err(|e| format!("eth_blockNumber failed: {e}"))?;

        // Resume point + any already-known matching swap ids from prior scans.
        let (mut start, mut matched_ids) = {
            let cache = self.cache.lock().expect("scan cache poisoned");
            let matched: Vec<[u8; 32]> = cache
                .locked
                .iter()
                .filter(|l| l.hashlock == hashlock)
                .map(|l| l.swap_id)
                .collect();
            (cache.next_block, matched)
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
                if let Ok(decoded) = log.log_decode::<EthHTLC::Locked>() {
                    let event = &decoded.inner.data;
                    let seen = SeenLock {
                        swap_id: event.swapId.0,
                        hashlock: event.hashlock.0,
                    };
                    if seen.hashlock == hashlock && !matched_ids.contains(&seen.swap_id) {
                        matched_ids.push(seen.swap_id);
                    }
                    fresh.push(seen);
                }
            }

            // Commit this chunk's discoveries + advance the resume point.
            {
                let mut cache = self.cache.lock().expect("scan cache poisoned");
                if end + 1 > cache.next_block {
                    cache.locked.extend(fresh);
                    cache.next_block = end + 1;
                }
            }

            chunks_used += 1;
            start = end + 1;
        }

        let coverage_to = {
            // next_block - 1 is the highest block fully scanned across all calls.
            let cache = self.cache.lock().expect("scan cache poisoned");
            cache.next_block.saturating_sub(1)
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
}
