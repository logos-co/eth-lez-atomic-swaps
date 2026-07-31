//! lez-mcp library surface — see `main.rs` for the server entrypoint and the
//! crate README for tool documentation. Exposed as a lib so integration
//! tests can exercise the tool internals against the public testnet.

pub mod config;
pub mod eth;
pub mod faucet;
pub mod fingerprint;
pub mod server;

use std::time::Duration;

use swap_orchestrator::config::{LezAuth, SwapConfig};

use crate::config::McpConfig;

/// Fabricate the app's SwapConfig for LezClient construction. Only the LEZ
/// fields matter to LezClient; ETH/swap fields are placeholders.
pub fn swap_config_for(cfg: &McpConfig, lez_auth: LezAuth) -> SwapConfig {
    SwapConfig {
        eth_rpc_url: cfg.eth_rpc_url.clone(),
        eth_private_key: String::new(),
        eth_htlc_address: alloy::primitives::Address::ZERO,
        lez_sequencer_url: cfg.sequencer_url.clone(),
        lez_sequencer_url_explicit: true,
        lez_auth,
        lez_htlc_program_id: swap_orchestrator::config::parse_program_id(&cfg.lez_htlc_program_id)
            .unwrap_or([0u32; 8]),
        lez_amount: 0,
        eth_amount: 0,
        lez_timelock: 0,
        eth_timelock: 0,
        eth_recipient_address: alloy::primitives::Address::ZERO,
        lez_taker_account_id: lee::AccountId::new([0u8; 32]),
        poll_interval: Duration::from_secs(2),
    }
}
