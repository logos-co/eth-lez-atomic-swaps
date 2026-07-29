//! Read-only Sepolia EthHTLC access. No ETH key required — the provider has
//! no wallet; only `eth_call` / `eth_getLogs` are used.

use alloy::{
    primitives::{Address, FixedBytes},
    providers::{DynProvider, Provider, ProviderBuilder, WsConnect},
    rpc::types::Filter,
    sol_types::SolEvent as _,
};
use swap_orchestrator::eth::client::EthHTLC;

/// Max block span per `eth_getLogs` call (public providers cap ranges).
const LOG_SCAN_CHUNK: u64 = 40_000;

pub struct EthReader {
    contract: EthHTLC::EthHTLCInstance<DynProvider>,
    from_block: u64,
}

pub struct FoundHtlc {
    pub swap_id: [u8; 32],
    pub htlc: EthHTLC::HTLC,
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

    /// Scan `Locked` events from the deployment block for HTLCs whose
    /// hashlock matches. Returns the matching swap ids with their current
    /// state, plus the scanned block range.
    pub async fn find_by_hashlock(
        &self,
        hashlock: [u8; 32],
    ) -> Result<(Vec<FoundHtlc>, u64, u64), String> {
        let provider = self.contract.provider();
        let latest = provider
            .get_block_number()
            .await
            .map_err(|e| format!("eth_blockNumber failed: {e}"))?;

        let mut found = Vec::new();
        let mut start = self.from_block;
        while start <= latest {
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

            for log in logs {
                if let Ok(decoded) = log.log_decode::<EthHTLC::Locked>() {
                    let event = &decoded.inner.data;
                    if event.hashlock.0 == hashlock {
                        let swap_id = event.swapId.0;
                        let htlc = self.htlc_by_swap_id(swap_id).await?;
                        found.push(FoundHtlc { swap_id, htlc });
                    }
                }
            }
            start = end + 1;
        }

        Ok((found, self.from_block, latest))
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
