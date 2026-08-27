use std::time::Duration;

use clap::Args;
use sha2::{Digest, Sha256};
use tokio::sync::mpsc;

use crate::demo::DemoEnv;
use crate::error::Result;
use crate::eth::client::EthClient;
use crate::lez::client::LezClient;
use crate::scaffold;
use crate::swap::maker::run_maker;
use crate::swap::progress::SwapProgress;
use crate::swap::taker::run_taker;
use crate::swap::types::SwapOutcome;

/// Drain a progress channel, printing each event as the same JSON wire format
/// swap-ffi forwards to the UI (`serde_json::to_string(&progress)`), prefixed
/// with the role so maker/taker output is distinguishable in interleaved logs.
/// This is what makes the new `tx_hash`/`chain_id` fields on `EthLocked` /
/// `EthLockDetected` visible and verifiable from `make demo` output.
fn spawn_progress_printer(
    role: &'static str,
    mut rx: mpsc::UnboundedReceiver<SwapProgress>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        while let Some(event) = rx.recv().await {
            if let Ok(json) = serde_json::to_string(&event) {
                println!("[{role}-progress] {json}");
            }
        }
    })
}

#[derive(Args, Clone, Debug, Default)]
pub struct DemoArgs {
    /// Reuse an already-running scaffold localnet instead of starting/stopping it.
    #[arg(long)]
    pub no_localnet: bool,
}

pub async fn cmd_demo(args: DemoArgs) -> Result<()> {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .try_init();

    println!();
    println!("=== Atomic Swap Demo (LEZ + Ethereum) ===");
    println!();

    if !args.no_localnet {
        eprint!("  Starting scaffold localnet...");
        scaffold::localnet_start().await?;
        eprintln!(" \x1b[32m\u{2713}\x1b[0m");
    }

    let result = run_demo().await;
    if !args.no_localnet {
        scaffold::localnet_stop().await;
    }
    result
}

async fn run_demo() -> Result<()> {
    let env = DemoEnv::start(Some(Box::new(|step, label, detail| {
        if detail.is_empty() {
            eprint!("  [{step}/5] {label}...");
        } else {
            eprintln!("  \x1b[32m\u{2713}\x1b[0m {detail}");
        }
    })))
    .await?;

    let maker_config = env.maker_config.clone();
    let taker_config = env.taker_config.clone();

    println!();
    println!("--- Configuration ---");
    println!("  LEZ amount:  {} LEZ", maker_config.lez_amount);
    println!("  ETH amount:  {} wei", maker_config.eth_amount);
    println!("  ETH HTLC:    {}", maker_config.eth_htlc_address);
    println!("  Sequencer:   {}", maker_config.lez_sequencer_url);
    println!();
    println!("--- Running Swap ---");
    println!();

    let preimage: [u8; 32] = rand::random();
    let hashlock: [u8; 32] = Sha256::digest(preimage).into();

    let (maker_progress_tx, maker_progress_rx) = mpsc::unbounded_channel::<SwapProgress>();
    let maker_progress_printer = spawn_progress_printer("maker", maker_progress_rx);

    let (taker_progress_tx, taker_progress_rx) = mpsc::unbounded_channel::<SwapProgress>();
    let taker_progress_printer = spawn_progress_printer("taker", taker_progress_rx);

    let maker_handle = {
        let config = maker_config.clone();
        tokio::spawn(async move {
            let eth = EthClient::new(&config).await.unwrap();
            let lez = LezClient::new(&config).unwrap();

            eprintln!("  [maker] Waiting for ETH lock");
            run_maker(
                &config,
                &eth,
                &lez,
                Some(hashlock),
                None,
                Some(maker_progress_tx),
                None,
            )
            .await
        })
    };

    let taker_handle = {
        let config = taker_config.clone();
        tokio::spawn(async move {
            let eth = EthClient::new(&config).await.unwrap();
            let lez = LezClient::new(&config).unwrap();

            tokio::time::sleep(Duration::from_secs(3)).await;
            eprintln!("  [taker] Locking ETH");
            run_taker(&config, &eth, &lez, Some(preimage), Some(taker_progress_tx)).await
        })
    };

    let (maker_result, taker_result) = tokio::join!(maker_handle, taker_handle);

    let maker_outcome = maker_result.unwrap()?;
    let taker_outcome = taker_result.unwrap()?;

    // The progress senders were moved into the completed maker/taker tasks and
    // dropped on return, so the printer tasks drain the remaining buffered
    // events and exit on their own; await them so all progress lines are
    // flushed before the results print below.
    let _ = maker_progress_printer.await;
    let _ = taker_progress_printer.await;

    println!();
    println!("--- Results ---");
    println!();
    print_outcome("Maker", &maker_outcome);
    print_outcome("Taker", &taker_outcome);
    println!();

    Ok(())
}

fn print_outcome(role: &str, outcome: &SwapOutcome) {
    match outcome {
        SwapOutcome::Completed {
            preimage,
            eth_tx,
            lez_tx,
        } => {
            println!("  \x1b[32m{role} completed\x1b[0m");
            println!("    preimage: {}", hex::encode(preimage));
            println!("    ETH tx:   {eth_tx}");
            println!("    LEZ tx:   {lez_tx}");
        }
        SwapOutcome::Refunded {
            eth_refund_tx,
            lez_refund_tx,
        } => {
            println!("  \x1b[33m{role} refunded\x1b[0m");
            println!("    ETH refund: {eth_refund_tx:?}");
            println!("    LEZ refund: {lez_refund_tx:?}");
        }
    }
}
