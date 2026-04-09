use serde::{Deserialize, Serialize};

// ── Content topics ──────────────────────────────────────────────────

pub const OFFERS_TOPIC: &str = "/atomic-swaps/1/offers/json";

pub fn swap_topic(hashlock: &[u8; 32]) -> String {
    format!("/atomic-swaps/1/swap-{}/json", hex::encode(hashlock))
}

// ── Application messages ────────────────────────────────────────────

/// Broadcast by maker to `OFFERS_TOPIC`.
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

/// Sent by taker on `swap_topic(hashlock)`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SwapAccept {
    pub hashlock: String,
    pub eth_swap_id: String,
    /// base58-encoded
    pub taker_lez_account: String,
    pub taker_eth_address: String,
}
