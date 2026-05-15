use std::time::Duration;

use sha2::{Digest, Sha256};

use crate::demo::DemoEnv;
use crate::error::Result;
use crate::eth::client::EthClient;
use crate::lez::client::LezClient;
use crate::scaffold;
use crate::swap::maker::run_maker;
use crate::swap::taker::run_taker;
use crate::swap::types::SwapOutcome;

pub async fn cmd_demo() -> Result<()> {
    let _ = tracing_subscriber::fmt::try_init();

    println!();
    println!("=== Atomic Swap Demo (LEZ + Ethereum) ===");
    println!();

    eprint!("  Starting scaffold localnet...");
    scaffold::localnet_start().await?;
    eprintln!(" \x1b[32m\u{2713}\x1b[0m");

    let result = run_demo().await;
    scaffold::localnet_stop().await;
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

    let maker_handle = {
        let config = maker_config.clone();
        tokio::spawn(async move {
            let eth = EthClient::new(&config).await.unwrap();
            let lez = LezClient::new(&config).unwrap();

            eprintln!("  [maker] Waiting for ETH lock");
            run_maker(&config, &eth, &lez, Some(hashlock), None, None).await
        })
    };

    let taker_handle = {
        let config = taker_config.clone();
        tokio::spawn(async move {
            let eth = EthClient::new(&config).await.unwrap();
            let lez = LezClient::new(&config).unwrap();

            tokio::time::sleep(Duration::from_secs(3)).await;
            eprintln!("  [taker] Locking ETH");
            run_taker(&config, &eth, &lez, Some(preimage), None).await
        })
    };

    let (maker_result, taker_result) = tokio::join!(maker_handle, taker_handle);

    let maker_outcome = maker_result.unwrap()?;
    let taker_outcome = taker_result.unwrap()?;

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
