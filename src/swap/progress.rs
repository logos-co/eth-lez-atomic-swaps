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
        /// The taker's LEZ AccountId (hex) carried on the ETH `Locked` event —
        /// the account the maker will name as the sole claimant of its LEZ
        /// escrow. Surfaced so an operator can see WHO a swap is bound to.
        taker_lez_account: String,
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
    /// A matched ETH lock was turned away because of the LEZ account it named:
    /// either one the maker can never lock to (zero, or the maker's own — the
    /// LEZ HTLC requires `maker != taker`), or, for a designated-counterparty
    /// maker, a taker it does not serve. Kept separate from
    /// [`SwapProgress::EthLockRejected`] so the timelock story stays honest;
    /// like it, purely informational — the maker records the rejection BEFORE
    /// any journal write and keeps waiting.
    EthLockRejectedTakerAccount {
        swap_id: String,
        /// The rejected `takerLezAccount` (hex) from the `Locked` event.
        taker_lez_account: String,
        reason: String,
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
