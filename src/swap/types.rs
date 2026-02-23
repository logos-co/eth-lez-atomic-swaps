use alloy::primitives::{Address, FixedBytes};
use nssa_core::account::AccountId;

/// Parameters for a cross-chain atomic swap.
pub struct SwapParams {
    pub hashlock: [u8; 32],
    pub lez_amount: u128,
    pub eth_amount: u128,
    /// Absolute Unix timestamp — maker can refund LEZ after this.
    pub lez_timelock: u64,
    /// Absolute Unix timestamp — taker can refund ETH after this.
    pub eth_timelock: u64,
    pub maker_lez_account_id: AccountId,
    pub taker_lez_account_id: AccountId,
    pub maker_eth_address: Address,
    pub taker_eth_address: Address,
}

/// Outcome of a swap orchestration flow.
pub enum SwapOutcome {
    Completed {
        preimage: [u8; 32],
        eth_claim_tx: FixedBytes<32>,
        lez_claim_tx: String,
    },
    Refunded {
        eth_refund_tx: Option<FixedBytes<32>>,
        lez_refund_tx: Option<String>,
    },
}
