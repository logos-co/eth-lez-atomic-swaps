use serde::Serialize;
use tokio::sync::mpsc;

/// Progress events emitted during swap orchestration flows.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "step", content = "data")]
pub enum SwapProgress {
    // Taker steps (taker locks first, claims LEZ)
    PreimageGenerated {
        hashlock: String,
    },
    LockingEth,
    EthLocked {
        swap_id: String,
        /// The ETH lock transaction hash, so a consumer can build a block
        /// explorer link.
        tx_hash: String,
        /// The chain ID of the ETH endpoint the lock was submitted to (e.g.
        /// 31337 for Anvil, 11155111 for Sepolia), so a hash isn't ambiguous.
        chain_id: u64,
    },
    WaitingForLezLock,
    LezLockDetected,
    VerifyingLezEscrow,
    LezEscrowVerified,
    ClaimingLez,
    LezClaimed {
        tx_hash: String,
    },

    // Maker steps (maker locks second, claims ETH)
    WaitingForEthLock,
    EthLockDetected {
        swap_id: String,
        hashlock: String,
        /// The taker's ETH lock transaction hash, so a consumer can build a
        /// block explorer link — this is the maker's only path to the
        /// taker's lock tx.
        tx_hash: String,
        /// The chain ID of the ETH endpoint the lock was observed on.
        chain_id: u64,
    },
    /// A matched ETH lock was rejected by the loop-mode timelock-margin gate
    /// (P1-E): its absolute on-chain expiry does not clear the maker's fresh
    /// LEZ expiry plus the safety margin. Purely informational — the maker
    /// records the rejection and keeps waiting — but without it the UI sits at
    /// WaitingForEthLock with no hint of why nothing is happening.
    EthLockRejected {
        swap_id: String,
        /// The taker's absolute ETH expiry (unix seconds) from the contract.
        eth_expiry_secs: u64,
        /// The minimum acceptable expiry: fresh LEZ expiry + margin.
        required_expiry_secs: u64,
    },
    LezLocking,
    LezLocked {
        tx_hash: String,
    },
    WaitingForPreimage,
    PreimageRevealed {
        preimage: String,
    },
    ClaimingEth,
    EthClaimed {
        tx_hash: String,
    },

    // Shared
    TimelockExpired,
    Refunding,
    RefundComplete,

    // Auto-accept loop events
    AutoAcceptStarted,
    AutoAcceptIteration {
        iteration: u32,
    },
    AutoAcceptSwapCompleted {
        iteration: u32,
        status: String,
    },
    AutoAcceptSwapFailed {
        iteration: u32,
        error: String,
    },
    AutoAcceptInsufficientFunds {
        lez_balance: String,
        lez_required: String,
    },
    AutoAcceptStopped {
        total_completed: u32,
        total_failed: u32,
    },
    AutoAcceptCancelled,
}

pub type ProgressSender = mpsc::UnboundedSender<SwapProgress>;

/// Send a progress event if the sender is present; no-op if `None`.
pub fn report(sender: &Option<ProgressSender>, progress: SwapProgress) {
    if let Some(tx) = sender {
        let _ = tx.send(progress);
    }
}
