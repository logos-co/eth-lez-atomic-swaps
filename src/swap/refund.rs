use std::time::{SystemTime, UNIX_EPOCH};

use alloy::primitives::FixedBytes;

use crate::{
    error::{Result, SwapError},
    eth::client::EthClient,
    lez::client::LezClient,
};

/// Refund LEZ from the HTLC escrow after the timelock has expired.
///
/// LEZ has no on-chain timelock enforcement, so we check off-chain before
/// submitting the refund instruction.
pub async fn refund_lez(
    lez_client: &LezClient,
    hashlock: &[u8; 32],
    lez_timelock: u64,
) -> Result<String> {
    let now = now_unix();
    if now < lez_timelock {
        return Err(SwapError::TimelockNotExpired(lez_timelock - now));
    }

    lez_client.refund(hashlock).await
}

/// Refund ETH from the HTLC contract after the timelock has expired.
///
/// The on-chain contract enforces the timelock, so we simply delegate.
pub async fn refund_eth(
    eth_client: &EthClient,
    swap_id: FixedBytes<32>,
) -> Result<FixedBytes<32>> {
    eth_client.refund(swap_id).await
}

/// Current Unix timestamp in seconds.
pub fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock before UNIX epoch")
        .as_secs()
}
