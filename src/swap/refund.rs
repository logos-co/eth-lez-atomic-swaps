use std::time::{SystemTime, UNIX_EPOCH};

use alloy::primitives::FixedBytes;

use crate::{
    error::{Result, SwapError},
    eth::client::EthClient,
    lez::client::LezClient,
};

/// Check that the LEZ timelock has expired. Pure function, no I/O.
pub fn check_lez_timelock(lez_timelock: u64) -> Result<()> {
    let now = now_unix();
    if now < lez_timelock {
        return Err(SwapError::TimelockNotExpired(lez_timelock - now));
    }
    Ok(())
}

/// Refund LEZ from the HTLC escrow after the timelock has expired.
///
/// LEZ has no on-chain timelock enforcement, so we check off-chain before
/// submitting the refund instruction.
pub async fn refund_lez(
    lez_client: &LezClient,
    hashlock: &[u8; 32],
    lez_timelock: u64,
) -> Result<String> {
    check_lez_timelock(lez_timelock)?;
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn now_unix_returns_reasonable_value() {
        let now = now_unix();
        assert!(now > 1_700_000_000, "should be past 2023");
        assert!(now < 2_000_000_000, "should be before 2033");
    }

    #[test]
    fn check_lez_timelock_rejects_unexpired() {
        let future = now_unix() + 3600;
        let err = check_lez_timelock(future).unwrap_err();
        assert!(matches!(err, SwapError::TimelockNotExpired(_)));
    }

    #[test]
    fn check_lez_timelock_allows_expired() {
        let past = now_unix() - 1;
        check_lez_timelock(past).unwrap();
    }
}
