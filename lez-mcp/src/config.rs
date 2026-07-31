//! Environment-driven configuration for the MCP server.
//!
//! Key material policy: the LEZ signing key enters ONLY via
//! `LEZ_SIGNING_KEY` (hex in env), `LEZ_SIGNING_KEY_FILE` (path to a file
//! containing the hex key), or a scaffold wallet on disk
//! (`LEZ_WALLET_HOME` + `LEZ_ACCOUNT_ID`). Keys are never accepted as tool
//! arguments and never logged.

use std::path::PathBuf;

use swap_orchestrator::config::{LezAuth, parse_base58_account_id};

/// Default public LEZ testnet sequencer.
pub const DEFAULT_SEQUENCER_URL: &str = "https://testnet.lez.logos.co";
/// Default Sepolia WebSocket RPC (the app requires WebSocket).
pub const DEFAULT_ETH_RPC_URL: &str = "wss://ethereum-sepolia-rpc.publicnode.com";
/// Canonical shared EthHTLC deployment on Sepolia (2026-07-21).
pub const DEFAULT_ETH_HTLC_ADDRESS: &str = "0x8636Fe66DFee166589a913140f14d5F57394834A";
/// Block the canonical EthHTLC was deployed in — lower bound for Locked-event
/// scans when resolving a hashlock to a swap id.
pub const DEFAULT_ETH_HTLC_FROM_BLOCK: u64 = 11_316_985;
/// Canonical LEZ HTLC guest program id (hex, LE-per-word) for the v0.2.0 pin.
pub const DEFAULT_LEZ_HTLC_PROGRAM_ID: &str =
    "27720b5b0345135d8e684eb172c27f5fb237548cc891a3ec889d0ed340504070";

#[derive(Debug, Clone)]
pub struct McpConfig {
    pub sequencer_url: String,
    /// Signing configuration; `None` = read-only server (no transfer tool).
    pub lez_auth: Option<LezAuth>,
    pub eth_rpc_url: String,
    pub eth_htlc_address: String,
    pub eth_htlc_from_block: u64,
    pub lez_htlc_program_id: String,
}

impl McpConfig {
    /// Read configuration from the environment. Never logs key material.
    pub fn from_env() -> Result<Self, String> {
        let sequencer_url = std::env::var("LEZ_SEQUENCER_URL")
            .ok()
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(|| DEFAULT_SEQUENCER_URL.to_string());

        let lez_auth = Self::auth_from_env()?;

        let eth_rpc_url = std::env::var("ETH_RPC_URL")
            .ok()
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(|| DEFAULT_ETH_RPC_URL.to_string());
        let eth_htlc_address = std::env::var("ETH_HTLC_ADDRESS")
            .ok()
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(|| DEFAULT_ETH_HTLC_ADDRESS.to_string());
        let eth_htlc_from_block = match std::env::var("ETH_HTLC_FROM_BLOCK") {
            Ok(s) if !s.trim().is_empty() => s
                .trim()
                .parse()
                .map_err(|e| format!("invalid ETH_HTLC_FROM_BLOCK: {e}"))?,
            _ => DEFAULT_ETH_HTLC_FROM_BLOCK,
        };
        let lez_htlc_program_id = std::env::var("LEZ_HTLC_PROGRAM_ID")
            .ok()
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(|| DEFAULT_LEZ_HTLC_PROGRAM_ID.to_string());

        Ok(Self {
            sequencer_url,
            lez_auth,
            eth_rpc_url,
            eth_htlc_address,
            eth_htlc_from_block,
            lez_htlc_program_id,
        })
    }

    /// Resolve signing configuration, in priority order:
    /// 1. `LEZ_SIGNING_KEY` (raw hex in env)
    /// 2. `LEZ_SIGNING_KEY_FILE` (path to a file containing the hex key)
    /// 3. `LEZ_WALLET_HOME` + `LEZ_ACCOUNT_ID` (scaffold wallet on disk)
    ///
    /// Returns `Ok(None)` when none are set — the server then runs read-only
    /// (plus faucet, which needs no signature).
    fn auth_from_env() -> Result<Option<LezAuth>, String> {
        if let Ok(key) = std::env::var("LEZ_SIGNING_KEY")
            && !key.trim().is_empty()
        {
            return Ok(Some(LezAuth::RawKey(key.trim().to_string())));
        }

        if let Ok(path) = std::env::var("LEZ_SIGNING_KEY_FILE")
            && !path.trim().is_empty()
        {
            let key = std::fs::read_to_string(path.trim())
                .map_err(|e| format!("cannot read LEZ_SIGNING_KEY_FILE: {e}"))?;
            let key = key.trim().to_string();
            if key.is_empty() {
                return Err("LEZ_SIGNING_KEY_FILE is empty".into());
            }
            return Ok(Some(LezAuth::RawKey(key)));
        }

        let home = std::env::var("LEZ_WALLET_HOME").ok().filter(|s| !s.is_empty());
        let account = std::env::var("LEZ_ACCOUNT_ID").ok().filter(|s| !s.is_empty());
        match (home, account) {
            (Some(home), Some(account)) => {
                let account_id = parse_base58_account_id(&account)
                    .map_err(|e| format!("invalid LEZ_ACCOUNT_ID: {e}"))?;
                Ok(Some(LezAuth::Wallet {
                    home: PathBuf::from(home),
                    account_id,
                }))
            }
            (None, None) => Ok(None),
            _ => Err(
                "LEZ_WALLET_HOME and LEZ_ACCOUNT_ID must be set together (or use LEZ_SIGNING_KEY)"
                    .into(),
            ),
        }
    }
}
