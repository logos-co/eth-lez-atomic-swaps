use std::time::Duration;

use alloy::primitives::Address;
use nssa_core::program::ProgramId;
use nssa_core::account::AccountId;

use crate::error::{SwapError, Result};

#[derive(Debug, Clone)]
pub struct SwapConfig {
    // --- Ethereum ---
    pub eth_rpc_url: String,
    pub eth_private_key: String,
    pub eth_htlc_address: Address,

    // --- LEZ ---
    pub lez_sequencer_url: String,
    pub lez_signing_key: String,
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

    // --- Messaging ---
    /// Nwaku REST API URL. None = messaging disabled, out-of-band coordination.
    pub nwaku_url: Option<String>,
}

pub fn parse_account_id(hex_str: &str) -> Result<AccountId> {
    let hex_str = hex_str.strip_prefix("0x").unwrap_or(hex_str);
    let bytes = hex::decode(hex_str)
        .map_err(|e| SwapError::InvalidConfig(format!("invalid account ID hex: {e}")))?;
    let arr: [u8; 32] = bytes
        .try_into()
        .map_err(|_| SwapError::InvalidConfig("account ID must be 32 bytes".into()))?;
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
