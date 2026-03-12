use serde::Serialize;
use tokio::sync::mpsc;

/// Progress events emitted during swap orchestration flows.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "step", content = "data")]
pub enum SwapProgress {
    // Taker steps (taker locks first, claims LEZ)
    PreimageGenerated { hashlock: String },
    LockingEth,
    EthLocked { swap_id: String },
    WaitingForLezLock,
    LezLockDetected,
    VerifyingLezEscrow,
    LezEscrowVerified,
    ClaimingLez,
    LezClaimed { tx_hash: String },

    // Maker steps (maker locks second, claims ETH)
    WaitingForEthLock,
    EthLockDetected { swap_id: String },
    LezLocking,
    LezLocked { tx_hash: String },
    WaitingForPreimage,
    PreimageRevealed { preimage: String },
    ClaimingEth,
    EthClaimed { tx_hash: String },

    // Shared
    TimelockExpired,
    Refunding,
    RefundComplete,
}

pub type ProgressSender = mpsc::UnboundedSender<SwapProgress>;

/// Send a progress event if the sender is present; no-op if `None`.
pub fn report(sender: &Option<ProgressSender>, progress: SwapProgress) {
    if let Some(tx) = sender {
        let _ = tx.send(progress);
    }
}
