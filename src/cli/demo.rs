use std::sync::Arc;
use std::time::Duration;

use sha2::{Digest, Sha256};

use crate::config::account_id_to_base58;
use crate::demo::DemoEnv;
use crate::error::Result;
use crate::eth::client::EthClient;
use crate::lez::client::LezClient;
use crate::messaging::client::MessagingClient;
use crate::messaging::node::MessagingNodeConfig;
use crate::messaging::topics::OFFERS_TOPIC;
use crate::messaging::types::SwapOffer;
use crate::scaffold;
use crate::swap::maker::run_maker;
use crate::swap::taker::run_taker;
use crate::swap::types::SwapOutcome;

pub async fn cmd_demo() -> Result<()> {
    let _ = tracing_subscriber::fmt::try_init();

    println!();
    println!("=== Atomic Swap Demo (LEZ + Ethereum + Logos Messaging) ===");
    println!();

    // Ensure scaffold localnet is running (start if needed).
    eprint!("  Starting scaffold localnet...");
    scaffold::localnet_start().await?;
    eprintln!(" \x1b[32m\u{2713}\x1b[0m");

    // Run the demo, stopping the localnet on exit (success or failure).
    let result = run_demo().await;
    scaffold::localnet_stop().await;
    result
}

async fn run_demo() -> Result<()> {
    // Spawn two embedded waku nodes — one for the maker task, one for the taker.
    // Both bind to OS-assigned ports (avoids leaks if a panic skips shutdown).
    // The taker dials the maker's listen address so they form a 2-peer mesh.
    eprint!("  Spawning embedded waku nodes...");
    let maker_msg = Arc::new(
        MessagingClient::spawn(MessagingNodeConfig {
            listen_port: 0,
            node_key_path: None,
            bootstrap_peers: vec![],
        })
        .await?,
    );
    let maker_addrs = maker_msg.listen_addresses().await?;
    let maker_addr = maker_addrs
        .into_iter()
        .next()
        .ok_or_else(|| crate::error::SwapError::Messaging("maker node has no listen addr".into()))?;

    let taker_msg = Arc::new(
        MessagingClient::spawn(MessagingNodeConfig {
            listen_port: 0,
            node_key_path: None,
            bootstrap_peers: vec![maker_addr.to_string()],
        })
        .await?,
    );
    eprintln!(" \x1b[32m\u{2713}\x1b[0m");

    // Subscribe both nodes BEFORE either side publishes — gossipsub doesn't
    // replay history, and we don't have a store-server (see dogfooding #12).
    maker_msg.subscribe(&[OFFERS_TOPIC]).await?;
    taker_msg.subscribe(&[OFFERS_TOPIC]).await?;
    // Brief pause for the gossipsub mesh to form.
    tokio::time::sleep(Duration::from_secs(1)).await;

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
    println!("  Messaging:   embedded (in-process, 2 nodes)");
    println!();
    println!("--- Running Swap ---");
    println!();

    // Generate a deterministic preimage for the demo (passed to taker).
    let preimage: [u8; 32] = rand::random();
    let hashlock: [u8; 32] = Sha256::digest(preimage).into();

    // Spawn maker: publish standing offer, wait for ETH lock, lock LEZ,
    // watch for preimage on LEZ, claim ETH.
    let maker_handle = {
        let config = maker_config.clone();
        let messaging = maker_msg.clone();
        tokio::spawn(async move {
            let eth = EthClient::new(&config).await.unwrap();
            let lez = LezClient::new(&config).unwrap();

            let offer = SwapOffer {
                hashlock: hex::encode(hashlock),
                lez_amount: config.lez_amount,
                eth_amount: config.eth_amount,
                maker_eth_address: format!("{}", config.eth_recipient_address),
                maker_lez_account: account_id_to_base58(&lez.account_id()),
                lez_timelock: config.lez_timelock,
                eth_timelock: config.eth_timelock,
                lez_htlc_program_id: hex::encode(
                    config
                        .lez_htlc_program_id
                        .iter()
                        .flat_map(|w| w.to_le_bytes())
                        .collect::<Vec<u8>>(),
                ),
                eth_htlc_address: format!("{}", config.eth_htlc_address),
            };
            messaging.publish(OFFERS_TOPIC, &offer).await.unwrap();
            eprintln!("  [maker] \x1b[34mPublished offer via Logos Messaging\x1b[0m");

            // run_maker waits for ETH lock, locks LEZ, watches for preimage, claims ETH.
            run_maker(&config, &eth, &lez, Some(hashlock), None, None).await
        })
    };

    // Spawn taker: discover offer, lock ETH, wait for LEZ lock, claim LEZ.
    let taker_handle = {
        let config = taker_config.clone();
        let messaging = taker_msg.clone();
        tokio::spawn(async move {
            let eth = EthClient::new(&config).await.unwrap();
            let lez = LezClient::new(&config).unwrap();

            // Discover offer via Logos Messaging.
            eprintln!("  [taker] Listening for offers via Logos Messaging...");
            discover_offer_demo(&messaging, &config).await;
            eprintln!("  [taker] \x1b[34mDiscovered offer via Logos Messaging\x1b[0m");

            // Brief pause so maker's ETH event watcher is ready before we lock.
            tokio::time::sleep(Duration::from_secs(3)).await;

            // run_taker generates preimage, locks ETH, waits for LEZ lock, claims LEZ.
            run_taker(&config, &eth, &lez, Some(preimage), None).await
        })
    };

    let (maker_result, taker_result) = tokio::join!(maker_handle, taker_handle);

    let maker_outcome = maker_result.unwrap()?;
    let taker_outcome = taker_result.unwrap()?;

    // Explicit shutdown — WakuNodeHandle has no Drop. See dogfooding #3.
    if let Ok(m) = Arc::try_unwrap(maker_msg) {
        let _ = m.shutdown().await;
    }
    if let Ok(t) = Arc::try_unwrap(taker_msg) {
        let _ = t.shutdown().await;
    }

    println!();
    println!("--- Results ---");
    println!();
    print_outcome("Maker", &maker_outcome);
    print_outcome("Taker", &taker_outcome);
    println!();

    Ok(())
}

/// Poll the embedded messaging client until a matching offer arrives.
async fn discover_offer_demo(messaging: &MessagingClient, config: &crate::config::SwapConfig) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(120);
    loop {
        let offers: Vec<SwapOffer> =
            messaging.poll_messages(OFFERS_TOPIC).await.unwrap_or_default();
        for offer in offers {
            if offer.lez_amount == config.lez_amount && offer.eth_amount == config.eth_amount {
                return;
            }
        }

        if tokio::time::Instant::now() >= deadline {
            panic!("taker: no matching offer found via messaging");
        }

        tokio::time::sleep(Duration::from_millis(500)).await;
    }
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
