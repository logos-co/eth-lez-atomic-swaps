use thiserror::Error;

#[derive(Debug, Error)]
pub enum SwapError {
    // --- Ethereum ---
    #[error("Ethereum RPC error: {0}")]
    EthRpc(String),

    #[error("Ethereum transaction reverted: {0}")]
    EthReverted(String),

    /// The deployed EthHTLC does not match the ABI this build compiles against.
    /// Raised at client construction (a single `eth_call` probe) so a mismatch
    /// fails LOUDLY at startup instead of silently: a topic0/selector change
    /// makes `Locked_filter().query()` return `Ok(vec![])` forever, which looks
    /// exactly like "no taker yet".
    #[error("EthHTLC ABI mismatch: {0}")]
    EthAbiMismatch(String),

    // --- LEZ ---
    #[error("LEZ sequencer error: {0}")]
    LezSequencer(String),

    #[error("LEZ transaction failed: {0}")]
    LezTransaction(String),

    #[error("failed to decode escrow data: {0}")]
    EscrowDecode(String),

    #[error("LEZ escrow state unknown (not a confirmed terminal state): {0}")]
    EscrowStateUnknown(String),

    #[error(
        "ETH claim unresolved after retries — taker holds the LEZ; stopping for supervised restart so reconcile retries: {0}"
    )]
    EthClaimUnresolved(String),

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

    // --- Cancellation ---
    #[error("swap cancelled by user")]
    Cancelled,
}

pub type Result<T> = std::result::Result<T, SwapError>;
