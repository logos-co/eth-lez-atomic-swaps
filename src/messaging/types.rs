use serde::{Deserialize, Serialize};

// ── Defaults ────────────────────────────────────────────────────────

pub const DEFAULT_NWAKU_URL: &str = "http://localhost:8645";

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

// ── Waku wire types (nwaku REST API) ────────────────────────────────

/// POST body for `/relay/v1/auto/messages`.
#[derive(Debug, Serialize)]
pub struct WakuRelayMessage {
    pub payload: String,
    #[serde(rename = "contentTopic")]
    pub content_topic: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timestamp: Option<i64>,
}

/// Single message from `GET /relay/v1/auto/messages/{topic}`.
#[derive(Debug, Deserialize)]
pub struct WakuRelayResponse {
    pub payload: String,
    #[serde(rename = "contentTopic")]
    pub content_topic: Option<String>,
    pub timestamp: Option<i64>,
}

/// Response from `GET /store/v3/messages`.
#[derive(Debug, Deserialize)]
pub struct StoreQueryResponse {
    #[serde(rename = "statusCode")]
    pub status_code: u32,
    pub messages: Vec<StoreMessageEntry>,
    #[serde(rename = "paginationCursor")]
    pub pagination_cursor: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct StoreMessageEntry {
    #[serde(rename = "messageHash")]
    pub message_hash: String,
    pub message: Option<StoreWakuMessage>,
}

#[derive(Debug, Deserialize)]
pub struct StoreWakuMessage {
    pub payload: String,
    #[serde(rename = "contentTopic")]
    pub content_topic: String,
    pub timestamp: Option<i64>,
}
