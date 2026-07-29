//! lez-mcp — MCP server (stdio) exposing the LEZ public testnet and the
//! Sepolia EthHTLC as typed agent tools.
//!
//! Startup sequence:
//! 1. Read env config (sequencer URL, optional signing key, ETH RPC).
//! 2. Fingerprint the configured sequencer (`getProgramIds` vs the embedded
//!    v0.2.0 builtin ImageIDs). Match → writes enabled. Mismatch or RPC
//!    failure → read tools stay live, write tools are hard-refused with the
//!    structured diff (a skewed client's transactions are silently dropped,
//!    so refusing loudly is the only safe behaviour).
//! 3. Serve MCP over stdio. All logging goes to stderr (stdout is the
//!    protocol channel). Key material is never logged.

use std::sync::Arc;

use lez_mcp::{
    config::McpConfig,
    fingerprint::run_fingerprint,
    server::{Ctx, Gate, LezMcpServer, Signer},
    swap_config_for,
};
use rmcp::{ServiceExt as _, transport::stdio};
use sequencer_service_rpc::SequencerClientBuilder;
use swap_orchestrator::lez::client::LezClient;
use tracing::{info, warn};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // stdout is the MCP channel — log to stderr only.
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info".into()),
        )
        .init();

    let cfg = McpConfig::from_env().map_err(|e| format!("config error: {e}"))?;
    info!(sequencer = %cfg.sequencer_url, "starting lez-mcp");

    let sequencer_url = url::Url::parse(&cfg.sequencer_url)
        .map_err(|e| format!("invalid LEZ_SEQUENCER_URL: {e}"))?;
    let sequencer = SequencerClientBuilder::default()
        .build(sequencer_url)
        .map_err(|e| format!("failed to create sequencer client: {e}"))?;

    // Signing identity (optional — server is read-only+faucet without it).
    let (signer, lez) = match &cfg.lez_auth {
        Some(auth) => {
            let signer = Signer::from_auth(auth).map_err(|e| format!("signer setup: {e}"))?;
            let lez = LezClient::new(&swap_config_for(&cfg, auth.clone()))
                .map_err(|e| format!("LezClient setup: {e}"))?;
            info!(
                account = %swap_orchestrator::config::account_id_to_base58(&signer.account_id),
                "signing key configured (key material is never logged)"
            );
            (Some(signer), Some(lez))
        }
        None => {
            info!("no signing key configured — lez_transfer disabled, reads + faucet available");
            (None, None)
        }
    };

    // Startup safety: fingerprint the configured sequencer before serving.
    let gate = match run_fingerprint(&sequencer, &cfg.sequencer_url).await {
        Ok(report) if report.matched => {
            info!("startup fingerprint MATCH — writes enabled");
            Gate::Verified(report)
        }
        Ok(report) => {
            warn!(
                mismatches = ?report.mismatches,
                "startup fingerprint MISMATCH — write tools will be refused"
            );
            Gate::Mismatch(report)
        }
        Err(e) => {
            warn!(error = %e, "startup fingerprint could not run — write tools will be refused \
                               until lez_fingerprint succeeds");
            Gate::Unverified { error: e }
        }
    };

    let ctx = Arc::new(Ctx::new(cfg, sequencer, lez, signer, gate));
    let service = LezMcpServer::new(ctx)
        .serve(stdio())
        .await
        .map_err(|e| format!("MCP serve failed: {e}"))?;
    service.waiting().await?;
    Ok(())
}
