use std::path::PathBuf;
use std::time::Duration;

use alloy::primitives::Address;
use nssa_core::program::ProgramId;
use nssa_core::account::AccountId;

use crate::error::{SwapError, Result};

/// How the LEZ client authenticates transactions.
#[derive(Debug, Clone)]
pub enum LezAuth {
    /// Raw signing key (32-byte hex) — used in tests / legacy demo mode.
    RawKey(String),
    /// Scaffold-managed wallet — reads keys from wallet files on disk.
    Wallet {
        /// Path to the scaffold wallet home directory (e.g. `.scaffold/wallet`).
        home: PathBuf,
        /// The account ID to sign with (from `wallet list`).
        account_id: AccountId,
    },
}

#[derive(Debug, Clone)]
pub struct SwapConfig {
    // --- Ethereum ---
    pub eth_rpc_url: String,
    pub eth_private_key: String,
    pub eth_htlc_address: Address,

    // --- LEZ ---
    pub lez_sequencer_url: String,
    pub lez_auth: LezAuth,
    pub lez_htlc_program_id: ProgramId,

    // --- Swap parameters ---
    pub lez_amount: u128,
    pub eth_amount: u128,
    /// Absolute Unix timestamp — maker can refund LEZ after this.
    pub lez_timelock: u64,
    /// Absolute Unix timestamp — taker can refund ETH after this.
    pub eth_timelock: u64,

    // --- Counterparty ---
    pub eth_recipient_address: Address,
    pub lez_taker_account_id: AccountId,

    // --- Polling ---
    pub poll_interval: Duration,
}

impl SwapConfig {
    /// Create a copy with fresh absolute timelocks computed from the current time.
    pub fn with_fresh_timelocks(&self, lez_minutes: u64, eth_minutes: u64) -> Self {
        let now = crate::swap::refund::now_unix();
        let mut fresh = self.clone();
        fresh.lez_timelock = now + lez_minutes * 60;
        fresh.eth_timelock = now + eth_minutes * 60;
        fresh
    }
}

/// Parse an ETH amount string (e.g. "0.001") and convert to wei (u128).
pub fn eth_to_wei(s: &str) -> std::result::Result<u128, String> {
    let s = s.trim();
    // Split on decimal point
    let (int_part, frac_part) = match s.split_once('.') {
        Some((i, f)) => (i, f),
        None => (s, ""),
    };
    let int_val: u128 = if int_part.is_empty() {
        0
    } else {
        int_part.parse().map_err(|e| format!("invalid ETH amount: {e}"))?
    };
    // Pad or truncate fractional part to 18 digits
    let frac_padded = if frac_part.len() > 18 {
        &frac_part[..18]
    } else {
        frac_part
    };
    let frac_val: u128 = if frac_padded.is_empty() {
        0
    } else {
        let padded = format!("{:0<18}", frac_padded);
        padded.parse().map_err(|e| format!("invalid ETH amount fraction: {e}"))?
    };
    Ok(int_val * 1_000_000_000_000_000_000 + frac_val)
}

/// Convert a wei value back to an ETH string (e.g. 1000000000000000 -> "0.001").
pub fn wei_to_eth_string(wei: u128) -> String {
    let whole = wei / 1_000_000_000_000_000_000;
    let frac = wei % 1_000_000_000_000_000_000;
    if frac == 0 {
        return format!("{whole}");
    }
    let frac_str = format!("{:018}", frac);
    let trimmed = frac_str.trim_end_matches('0');
    format!("{whole}.{trimmed}")
}

/// Convert an AccountId to base58 (matches `logos-scaffold wallet list` format).
pub fn account_id_to_base58(id: &AccountId) -> String {
    base58::ToBase58::to_base58(id.value().as_slice())
}

/// Parse a base58 account ID string (as used by scaffold/wallet).
pub fn parse_base58_account_id(s: &str) -> Result<AccountId> {
    let bytes = base58::FromBase58::from_base58(s)
        .map_err(|e| SwapError::InvalidConfig(format!("invalid base58 account ID: {e:?}")))?;
    let arr: [u8; 32] = bytes
        .try_into()
        .map_err(|_| SwapError::InvalidConfig("base58 account ID must decode to 32 bytes".into()))?;
    Ok(AccountId::new(arr))
}

pub fn parse_program_id(hex_str: &str) -> Result<ProgramId> {
    let hex_str = hex_str.strip_prefix("0x").unwrap_or(hex_str);
    let bytes = hex::decode(hex_str)
        .map_err(|e| SwapError::InvalidConfig(format!("invalid program ID hex: {e}")))?;
    if bytes.len() != 32 {
        return Err(SwapError::InvalidConfig("program ID must be 32 bytes".into()));
    }
    let mut id = [0u32; 8];
    for (i, chunk) in bytes.chunks_exact(4).enumerate() {
        id[i] = u32::from_le_bytes(chunk.try_into().unwrap());
    }
    Ok(id)
}
