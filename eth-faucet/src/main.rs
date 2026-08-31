//! In-house Sepolia ETH drip faucet — proof of concept.
//!
//! Why this exists (see `data/lez-eth-faucet-scout/report.md`): the app's Setup
//! step can fund a new user's LEZ account but not their Sepolia gas, so it
//! sends them to an external browser faucet and every 0.4.3 tester's first swap
//! died on "insufficient funds for gas". This service is the smallest thing
//! that lets the app hand out that gas itself: one funded hot key, a
//! proof-of-work gate borrowed from the LEZ pinata faucet the repo already
//! ships, and rate limits that bound the worst case to a known number of ETH
//! per day.
//!
//! **This is a PoC.** It signs value transfers from a hot key. Run it with a
//! THROWAWAY key and a small float; see `README-poc.md` for the deployment
//! notes and for the phase-2 `FaucetVault` design that turns the key into a
//! mere relayer.

mod chain;
mod challenges;
mod config;
mod ledger;
mod routes;

use std::net::SocketAddr;
use std::sync::Arc;

use alloy::signers::local::PrivateKeySigner;

use crate::chain::{Chain, eth_for_logs};
use crate::challenges::Challenges;
use crate::config::{Config, format_wei_as_eth};
use crate::ledger::Ledger;
use crate::routes::{AppState, now_secs};

#[tokio::main]
async fn main() {
    if let Err(message) = run().await {
        eprintln!("eth-faucet: {message}");
        std::process::exit(1);
    }
}

async fn run() -> Result<(), String> {
    // `--genkey` before anything else: it is the one mode that must work with
    // no configuration at all, because its whole job is producing the
    // configuration (a throwaway key) you do not have yet.
    if std::env::args().any(|arg| arg == "--genkey") {
        return genkey();
    }

    // `--solve <seed-hex> <difficulty-bits>`: answer a challenge from the
    // command line, using the same crate the service verifies with and the app
    // solves with. That is the point — it makes the curl demo in README-poc.md
    // exercise the real scheme rather than a shell reimplementation of it, and
    // it gives an operator a way to reproduce a user's "it says my answer is
    // wrong" by hand.
    let args: Vec<String> = std::env::args().collect();
    if let Some(index) = args.iter().position(|arg| arg == "--solve") {
        return solve(args.get(index + 1), args.get(index + 2));
    }

    // A local .env is a convenience for `make faucet-poc-run`; in the container
    // the values come from `env_file: faucet.env`. Either way the key is only
    // ever read from the environment — never from a checked-in file, never
    // from an argument (argv is world-readable in /proc).
    let _ = dotenvy::dotenv();

    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info".into()),
        )
        .init();

    let config = Config::from_env()?;
    let chain = Chain::connect(&config.rpc_url, &config.private_key).await?;

    let ledger = match &config.state_file {
        Some(path) => Ledger::load(std::path::Path::new(path))?,
        None => {
            tracing::warn!(
                "FAUCET_STATE_FILE is unset — cooldowns live in memory only and a restart \
                 resets every one of them. Fine for a local demo; set it in deployment."
            );
            Ledger::default()
        }
    };

    let balance = chain.balance_wei(chain.address()).await?;
    tracing::info!(
        faucet_address = %format!("{:#x}", chain.address()),
        chain_id = chain.chain_id(),
        balance = %eth_for_logs(balance),
        drip = %format_wei_as_eth(config.policy.drip_wei),
        daily_budget = %format_wei_as_eth(config.policy.daily_budget_wei),
        pow_difficulty_bits = config.difficulty_bits,
        "faucet ready"
    );
    if balance < config.policy.daily_budget_wei {
        // Not fatal: a faucet with half a day of budget still serves people.
        // /health reports the same condition, which is what the container
        // healthcheck acts on.
        tracing::warn!(
            balance = %eth_for_logs(balance),
            "faucet balance is below one day's budget — refill it"
        );
    }
    if chain.chain_id() != 11155111 {
        tracing::warn!(
            chain_id = chain.chain_id(),
            "FAUCET_RPC_URL is not Sepolia (11155111) — check the endpoint before funding the key"
        );
    }

    let bind: SocketAddr = config
        .bind
        .parse()
        .map_err(|e| format!("FAUCET_BIND '{}' is not a host:port: {e}", config.bind))?;

    let state = Arc::new(AppState {
        config,
        chain,
        ledger: tokio::sync::Mutex::new(ledger),
        challenges: tokio::sync::Mutex::new(Challenges::default()),
        started_at: now_secs(),
    });

    let app = routes::router(state).layer(tower_http::trace::TraceLayer::new_for_http());

    let listener = tokio::net::TcpListener::bind(bind)
        .await
        .map_err(|e| format!("cannot bind {bind}: {e}"))?;
    tracing::info!(%bind, "listening");

    // ConnectInfo so the per-IP limit has a peer address to fall back on when
    // there is no reverse proxy setting X-Forwarded-For.
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(shutdown_signal())
    .await
    .map_err(|e| format!("server error: {e}"))
}

/// Print a fresh random key + its address, for a throwaway faucet wallet.
///
/// To stdout, never to a file: writing a key to disk is how keys end up
/// committed. The operator pastes it into a gitignored `deploy/faucet.env`.
fn genkey() -> Result<(), String> {
    let signer = PrivateKeySigner::random();
    println!("# THROWAWAY faucet key — paste into deploy/faucet.env (gitignored).");
    println!("# Fund the address below with a SMALL float; never reuse a personal key.");
    println!("FAUCET_PRIVATE_KEY={}", hex::encode(signer.to_bytes()));
    println!("# address: {:#x}", signer.address());
    Ok(())
}

/// Solve one challenge and print the answer. Bounded like every other solve,
/// so a typo'd difficulty ends in a message rather than a wedged terminal.
fn solve(seed_hex: Option<&String>, difficulty_bits: Option<&String>) -> Result<(), String> {
    let (Some(seed_hex), Some(bits)) = (seed_hex, difficulty_bits) else {
        return Err("usage: eth-faucet --solve <seed-hex> <difficulty-bits>".to_string());
    };
    let seed: [u8; 32] = hex::decode(seed_hex.trim().trim_start_matches("0x"))
        .map_err(|e| format!("seed is not hex: {e}"))?
        .as_slice()
        .try_into()
        .map_err(|_| "seed must be 32 bytes (64 hex chars)".to_string())?;
    let bits: u8 = bits
        .trim()
        .parse()
        .map_err(|_| format!("'{bits}' is not a difficulty in bits"))?;

    let started = std::time::Instant::now();
    let cancel = std::sync::atomic::AtomicBool::new(false);
    let solution = eth_faucet_pow::compute_solution_bounded(
        &seed,
        bits,
        eth_faucet_pow::MAX_POW_ITERATIONS,
        Some(started + std::time::Duration::from_secs(600)),
        &cancel,
    )?;
    // The solution alone on stdout, so `$(eth-faucet --solve ...)` is usable;
    // the timing goes to stderr where it cannot end up in a JSON body.
    eprintln!(
        "solved {bits} bits in {:.1}s",
        started.elapsed().as_secs_f64()
    );
    println!("{solution}");
    Ok(())
}

async fn shutdown_signal() {
    let _ = tokio::signal::ctrl_c().await;
    tracing::info!("shutting down");
}
