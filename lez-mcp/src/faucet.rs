//! Native pinata (faucet) claims.
//!
//! The implementation (PoW solve + unsigned claim submission) lives in the
//! app crate at `swap_orchestrator::lez::faucet` so the MCP server and the
//! CLI/bot/GUI share exactly one copy — see that module's docs for the full
//! protocol writeup and its test suite for the PoW/cancellation coverage.
pub use swap_orchestrator::lez::faucet::*;
