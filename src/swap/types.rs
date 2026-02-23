use alloy::primitives::FixedBytes;

/// Outcome of a swap orchestration flow.
pub enum SwapOutcome {
    Completed {
        preimage: [u8; 32],
        eth_tx: FixedBytes<32>,  // maker: ETH claim tx, taker: ETH lock swap_id
        lez_tx: String,          // maker: LEZ lock tx, taker: LEZ claim tx
    },
    Refunded {
        eth_refund_tx: Option<FixedBytes<32>>,
        lez_refund_tx: Option<String>,
    },
}
