use thiserror::Error;

#[derive(Debug, Error)]
pub enum SwapError {
    // --- Ethereum ---
    #[error("Ethereum RPC error: {0}")]
    EthRpc(String),

    #[error("Ethereum transaction reverted: {0}")]
    EthReverted(String),

    // --- LEZ ---
    #[error("LEZ sequencer error: {0}")]
    LezSequencer(String),

    #[error("LEZ transaction failed: {0}")]
    LezTransaction(String),

    #[error("failed to decode escrow data: {0}")]
    EscrowDecode(String),

    // --- Swap logic ---
    #[error("invalid swap state: expected {expected}, got {actual}")]
    InvalidState { expected: String, actual: String },

    #[error("timeout waiting for {0}")]
    Timeout(String),

    #[error("invalid preimage")]
    InvalidPreimage,

    #[error("timelock not expired: {0} seconds remaining")]
    TimelockNotExpired(u64),

    // --- Scaffold ---
    #[error("scaffold error: {0}")]
    Scaffold(String),

    // --- Config ---
    #[error("missing environment variable: {0}")]
    MissingEnvVar(String),

    #[error("invalid configuration: {0}")]
    InvalidConfig(String),

    // --- Messaging ---
    #[error("messaging error: {0}")]
    Messaging(String),

    // --- Cancellation ---
    #[error("swap cancelled by user")]
    Cancelled,
}

pub type Result<T> = std::result::Result<T, SwapError>;
