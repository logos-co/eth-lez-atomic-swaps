use serde::{Deserialize, Serialize};

// ── Application messages ────────────────────────────────────────────

/// Broadcast by maker to the offers content topic.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SwapOffer {
    pub hashlock: String,
    pub lez_amount: u128,
    pub eth_amount: u128,
    pub maker_eth_address: String,
    /// base58-encoded
    pub maker_lez_account: String,
    pub lez_timelock: u64,
    pub eth_timelock: u64,
    pub lez_htlc_program_id: String,
    pub eth_htlc_address: String,
}

/// Sent by taker on the per-swap content topic.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SwapAccept {
    pub hashlock: String,
    pub eth_swap_id: String,
    /// base58-encoded
    pub taker_lez_account: String,
    pub taker_eth_address: String,
}
