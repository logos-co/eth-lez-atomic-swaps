use std::time::Duration;

use crate::demo::DemoEnv;
use crate::error::Result;
use crate::eth::client::EthClient;
use crate::lez::client::LezClient;
use crate::swap::maker::run_maker;
use crate::swap::taker::run_taker;
use crate::swap::types::SwapOutcome;

pub async fn cmd_demo() -> Result<()> {
    tracing_subscriber::fmt::init();

    println!();
    println!("=== Atomic Swap Demo ===");
    println!();

    let env = DemoEnv::start(Some(Box::new(|step, label, detail| {
        if detail.is_empty() {
            eprint!("  [{step}/6] {label}...");
        } else {
            eprintln!("  \x1b[32m\u{2713}\x1b[0m {detail}");
        }
    })))
    .await;

    println!();
    println!("--- Configuration ---");
    println!("  LEZ amount:  {} LEZ", env.maker_config.lez_amount);
    println!("  ETH amount:  {} wei", env.maker_config.eth_amount);
    println!("  Hashlock:    {}", hex::encode(env.hashlock));
    println!("  ETH HTLC:    {}", env.maker_config.eth_htlc_address);
    println!("  Sequencer:   {}", env.maker_config.lez_sequencer_url);
    println!();
    println!("--- Running Swap ---");
    println!("  Starting maker, taker will join in ~10s...");
    println!();

    let maker_config = env.maker_config.clone();
    let preimage = env.preimage;
    let hashlock = env.hashlock;
    let taker_config = env.taker_config.clone();

    // Spawn maker.
    let maker_handle = tokio::spawn(async move {
        let eth = EthClient::new(&maker_config).await.unwrap();
        let lez = LezClient::new(&maker_config).unwrap();
        run_maker(&maker_config, &eth, &lez, Some(preimage)).await
    });

    // Delay taker to let maker lock LEZ first.
    tokio::time::sleep(Duration::from_secs(10)).await;

    let taker_handle = tokio::spawn(async move {
        let eth = EthClient::new(&taker_config).await.unwrap();
        let lez = LezClient::new(&taker_config).unwrap();
        run_taker(&taker_config, &eth, &lez, hashlock).await
    });

    let (maker_result, taker_result) = tokio::join!(maker_handle, taker_handle);

    let maker_outcome = maker_result.unwrap()?;
    let taker_outcome = taker_result.unwrap()?;

    println!();
    println!("--- Results ---");
    println!();
    print_outcome("Maker", &maker_outcome);
    print_outcome("Taker", &taker_outcome);
    println!();

    // DemoEnv dropped here — services shut down.
    Ok(())
}

fn print_outcome(role: &str, outcome: &SwapOutcome) {
    match outcome {
        SwapOutcome::Completed {
            preimage,
            eth_tx,
            lez_tx,
        } => {
            println!("  \x1b[32m{role}: Swap completed!\x1b[0m");
            println!("    preimage: {}", hex::encode(preimage));
            println!("    ETH tx:   {eth_tx}");
            println!("    LEZ tx:   {lez_tx}");
        }
        SwapOutcome::Refunded {
            eth_refund_tx,
            lez_refund_tx,
        } => {
            println!("  \x1b[31m{role}: Swap refunded\x1b[0m");
            if let Some(tx) = eth_refund_tx {
                println!("    ETH refund tx: {tx}");
            }
            if let Some(tx) = lez_refund_tx {
                println!("    LEZ refund tx: {tx}");
            }
        }
    }
}
