//! One-off provisioning helper: generate a fresh LEZ signing key, initialize
//! it on-chain, and pinata-fund it to a target balance — using the native
//! onboarding path (`src/lez/onboard.rs`), no scaffold / `wallet` binary
//! required.
//!
//! NOT part of the maker container image (see `Dockerfile`, which only
//! builds the `swap-cli` bin) — this is a one-time setup step run during
//! `deploy/README.md` step 2, typically on the same host that will run the
//! bot: the pinata proof-of-work solve is CPU-bound at difficulty 3, and a
//! constrained sandbox/VM can time out on it (a 4-core VPS should not).
//!
//! Usage:
//!   cargo run --release --example onboard_maker_account -- \
//!     --sequencer-url https://testnet.lez.logos.co --target 150
//!
//! Prints the new account's base58 ID and hex signing key to stdout — the
//! only time the key is ever displayed. Copy it straight into
//! `deploy/maker.env` (`LEZ_SIGNING_KEY`) and clear your shell history.

use clap::Parser;
use sequencer_service_rpc::SequencerClientBuilder;
use swap_orchestrator::config::account_id_to_base58;
use swap_orchestrator::lez::onboard::{FundingProgress, Signer, claim_to_target};
use tokio::sync::mpsc;
use url::Url;

#[derive(Parser)]
struct Args {
    /// LEZ sequencer RPC URL.
    #[arg(long, default_value = "https://testnet.lez.logos.co")]
    sequencer_url: String,

    /// Target LEZ balance to reach via pinata claims (150 LEZ per claim).
    #[arg(long, default_value_t = 150)]
    target: u128,

    /// Only generate a keypair + account ID — skip on-chain init/funding
    /// entirely (no network call at all). Useful for the placeholder
    /// LEZ_TAKER_ACCOUNT_ID a restricted-mode maker deployment needs before
    /// a real public taker exists: it just has to be a well-formed account
    /// ID, not an initialized/funded one, since restricted mode never
    /// actually pays out to it.
    #[arg(long)]
    generate_only: bool,
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt::init();
    let args = Args::parse();

    let signer = Signer::generate().expect("LEZ key generation failed");
    let account_b58 = account_id_to_base58(&signer.account_id);

    eprintln!("=== new LEZ account ===");
    eprintln!("account_id (base58): {account_b58}");
    eprintln!("signing_key (hex):   {}", signer.signing_key);
    eprintln!(
        "Copy the signing_key above into deploy/maker.env as LEZ_SIGNING_KEY — it is not \
         printed again."
    );

    if args.generate_only {
        println!(
            "{}",
            serde_json::json!({ "account_id": account_b58, "generated_only": true })
        );
        return;
    }

    let url = Url::parse(&args.sequencer_url).expect("invalid --sequencer-url");
    let sequencer = SequencerClientBuilder::default()
        .build(url)
        .expect("failed to build sequencer client");

    eprintln!("initializing + funding to target {} LEZ...", args.target);

    let (tx, mut rx) = mpsc::unbounded_channel::<FundingProgress>();
    let progress_task = tokio::spawn(async move {
        while let Some(event) = rx.recv().await {
            eprintln!("[onboard] {event:?}");
        }
    });

    let final_balance = claim_to_target(&sequencer, &signer, args.target, Some(tx))
        .await
        .expect("funding loop failed");
    let _ = progress_task.await;

    eprintln!("=== done ===");
    eprintln!("account_id (base58): {account_b58}");
    eprintln!("final balance:       {final_balance} LEZ");
    // Also on stdout, machine-parseable, in case this is scripted:
    println!(
        "{}",
        serde_json::json!({
            "account_id": account_b58,
            "balance": final_balance.to_string(),
        })
    );
}
