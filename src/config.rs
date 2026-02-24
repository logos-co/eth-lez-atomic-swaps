use std::{env, time::Duration};

use alloy::primitives::Address;
use nssa_core::{account::AccountId, program::ProgramId};

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
    pub lez_account_id: AccountId,
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

fn required_env(name: &str) -> Result<String> {
    env::var(name).map_err(|_| SwapError::MissingEnvVar(name.to_string()))
}

fn parse_account_id(hex_str: &str) -> Result<AccountId> {
    let bytes = hex::decode(hex_str)
        .map_err(|e| SwapError::InvalidConfig(format!("invalid account ID hex: {e}")))?;
    let arr: [u8; 32] = bytes
        .try_into()
        .map_err(|_| SwapError::InvalidConfig("account ID must be 32 bytes".into()))?;
    Ok(AccountId::new(arr))
}

fn parse_program_id(hex_str: &str) -> Result<ProgramId> {
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

impl SwapConfig {
    pub fn from_env() -> Result<Self> {
        dotenvy::dotenv().ok();
        let eth_htlc_address: Address = required_env("ETH_HTLC_ADDRESS")?
            .parse()
            .map_err(|e| SwapError::InvalidConfig(format!("invalid ETH_HTLC_ADDRESS: {e}")))?;

        let eth_recipient_address: Address = required_env("ETH_RECIPIENT_ADDRESS")?
            .parse()
            .map_err(|e| {
                SwapError::InvalidConfig(format!("invalid ETH_RECIPIENT_ADDRESS: {e}"))
            })?;

        let lez_account_id = parse_account_id(&required_env("LEZ_ACCOUNT_ID")?)?;
        let lez_htlc_program_id = parse_program_id(&required_env("LEZ_HTLC_PROGRAM_ID")?)?;
        let lez_taker_account_id =
            parse_account_id(&required_env("LEZ_TAKER_ACCOUNT_ID")?)?;

        let lez_amount: u128 = required_env("LEZ_AMOUNT")?
            .parse()
            .map_err(|e| SwapError::InvalidConfig(format!("invalid LEZ_AMOUNT: {e}")))?;
        let eth_amount: u128 = required_env("ETH_AMOUNT")?
            .parse()
            .map_err(|e| SwapError::InvalidConfig(format!("invalid ETH_AMOUNT: {e}")))?;
        let lez_timelock: u64 = required_env("LEZ_TIMELOCK")?
            .parse()
            .map_err(|e| SwapError::InvalidConfig(format!("invalid LEZ_TIMELOCK: {e}")))?;
        let eth_timelock: u64 = required_env("ETH_TIMELOCK")?
            .parse()
            .map_err(|e| SwapError::InvalidConfig(format!("invalid ETH_TIMELOCK: {e}")))?;

        let poll_interval_ms: u64 = env::var("POLL_INTERVAL_MS")
            .unwrap_or_else(|_| "2000".into())
            .parse()
            .map_err(|e| SwapError::InvalidConfig(format!("invalid POLL_INTERVAL_MS: {e}")))?;

        Ok(Self {
            eth_rpc_url: required_env("ETH_RPC_URL")?,
            eth_private_key: required_env("ETH_PRIVATE_KEY")?,
            eth_htlc_address,
            lez_sequencer_url: required_env("LEZ_SEQUENCER_URL")?,
            lez_signing_key: required_env("LEZ_SIGNING_KEY")?,
            lez_account_id,
            lez_htlc_program_id,
            lez_amount,
            eth_amount,
            lez_timelock,
            eth_timelock,
            eth_recipient_address,
            lez_taker_account_id,
            poll_interval: Duration::from_millis(poll_interval_ms),
        })
    }
}
