use std::time::Duration;

use sha2::{Digest, Sha256};

use crate::demo::DemoEnv;
use crate::error::Result;
use crate::eth::client::EthClient;
use crate::lez::client::LezClient;
use crate::messaging::client::{MessagingClient, decode_waku_payload};
use crate::messaging::types::{SwapOffer, OFFERS_TOPIC};
use crate::swap::maker::run_maker;
use crate::swap::taker::run_taker;
use crate::swap::types::SwapOutcome;

const NWAKU_URL: &str = "http://localhost:8645";

pub async fn cmd_demo() -> Result<()> {
    let _ = tracing_subscriber::fmt::try_init();

    println!();
    println!("=== Atomic Swap Demo (LEZ + Ethereum + Logos Messaging) ===");
    println!();

    // Check if nwaku is reachable — messaging is required for the demo.
    let messaging = MessagingClient::new(NWAKU_URL);
    check_nwaku(&messaging).await?;
    println!("  \x1b[32m\u{2713}\x1b[0m Logos Messaging (nwaku) at {NWAKU_URL}");

    let env = DemoEnv::start(Some(Box::new(|step, label, detail| {
        if detail.is_empty() {
            eprint!("  [{step}/6] {label}...");
        } else {
            eprintln!("  \x1b[32m\u{2713}\x1b[0m {detail}");
        }
    })))
    .await;

    let mut maker_config = env.maker_config.clone();
    maker_config.nwaku_url = Some(NWAKU_URL.to_string());
    let mut taker_config = env.taker_config.clone();
    taker_config.nwaku_url = Some(NWAKU_URL.to_string());

    println!();
    println!("--- Configuration ---");
    println!("  LEZ amount:  {} LEZ", maker_config.lez_amount);
    println!("  ETH amount:  {} wei", maker_config.eth_amount);
    println!("  ETH HTLC:    {}", maker_config.eth_htlc_address);
    println!("  Sequencer:   {}", maker_config.lez_sequencer_url);
    println!("  Messaging:   {NWAKU_URL}");
    println!();
    println!("--- Running Swap ---");
    println!();

    let preimage = env.preimage;

    // Spawn maker: publish offer, lock LEZ, wait for ETH, claim ETH.
    let maker_handle = {
        let config = maker_config.clone();
        tokio::spawn(async move {
            let eth = EthClient::new(&config).await.unwrap();
            let lez = LezClient::new(&config).unwrap();
            let hashlock: [u8; 32] = Sha256::digest(preimage).into();

            // Publish offer via Logos Messaging.
            let messaging = MessagingClient::new(NWAKU_URL);
            let swap_topic = crate::messaging::types::swap_topic(&hashlock);
            messaging.subscribe(&[OFFERS_TOPIC, &swap_topic]).await.unwrap();

            let offer = SwapOffer {
                hashlock: hex::encode(hashlock),
                lez_amount: config.lez_amount,
                eth_amount: config.eth_amount,
                maker_eth_address: format!("{}", config.eth_recipient_address),
                maker_lez_account: hex::encode(lez.account_id().value()),
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
            eprintln!("          hashlock: {}", hex::encode(hashlock));

            // run_maker locks LEZ, waits for ETH lock event, claims ETH.
            run_maker(&config, &eth, &lez, Some(preimage), None).await
        })
    };

    // Spawn taker: discover offer via messaging, wait for escrow, lock ETH, claim LEZ.
    let taker_handle = {
        let config = taker_config.clone();
        tokio::spawn(async move {
            let eth = EthClient::new(&config).await.unwrap();
            let lez = LezClient::new(&config).unwrap();

            // Discover offer via Logos Messaging.
            eprintln!("  [taker] Listening for offers via Logos Messaging...");
            let hashlock = discover_offer_demo(&config).await;
            eprintln!("  [taker] \x1b[34mDiscovered offer via Logos Messaging\x1b[0m");
            eprintln!("          hashlock: {}", hex::encode(hashlock));

            // Wait for LEZ escrow to appear (maker may still be locking).
            eprintln!("  [taker] Waiting for LEZ escrow...");
            let deadline = tokio::time::Instant::now() + Duration::from_secs(120);
            loop {
                match lez.get_escrow(&hashlock).await {
                    Ok(Some(escrow)) if escrow.amount >= config.lez_amount => break,
                    _ => {}
                }
                if tokio::time::Instant::now() >= deadline {
                    panic!("taker: LEZ escrow did not appear in time");
                }
                tokio::time::sleep(Duration::from_millis(500)).await;
            }
            eprintln!("  [taker] LEZ escrow verified");

            // Brief pause so maker's ETH event watcher is ready before we lock.
            tokio::time::sleep(Duration::from_secs(3)).await;

            // run_taker verifies escrow, locks ETH, waits for preimage, claims LEZ.
            run_taker(&config, &eth, &lez, hashlock, None).await
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

/// Poll messaging until a matching offer is found. Returns the hashlock.
async fn discover_offer_demo(config: &crate::config::SwapConfig) -> [u8; 32] {
    let messaging = MessagingClient::new(NWAKU_URL);
    messaging.subscribe(&[OFFERS_TOPIC]).await.unwrap();

    let deadline = tokio::time::Instant::now() + Duration::from_secs(120);
    loop {
        // Try store first (in case offer was published before we subscribed).
        let now_ns = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos() as i64;
        if let Ok(entries) = messaging
            .store_query(&[OFFERS_TOPIC], Some(now_ns - 120_000_000_000), Some(20))
            .await
        {
            for entry in &entries {
                if let Some(ref msg) = entry.message {
                    if let Ok(offer) = decode_waku_payload::<SwapOffer>(&msg.payload) {
                        if offer.lez_amount == config.lez_amount
                            && offer.eth_amount == config.eth_amount
                        {
                            return parse_hashlock(&offer.hashlock);
                        }
                    }
                }
            }
        }

        // Poll relay cache.
        let offers: Vec<SwapOffer> = messaging.poll_messages(OFFERS_TOPIC).await.unwrap_or_default();
        for offer in offers {
            if offer.lez_amount == config.lez_amount && offer.eth_amount == config.eth_amount {
                return parse_hashlock(&offer.hashlock);
            }
        }

        if tokio::time::Instant::now() >= deadline {
            panic!("taker: no matching offer found via messaging");
        }

        tokio::time::sleep(Duration::from_millis(500)).await;
    }
}

fn parse_hashlock(hex_str: &str) -> [u8; 32] {
    let bytes = hex::decode(hex_str).expect("invalid hashlock hex in offer");
    bytes.try_into().expect("hashlock must be 32 bytes")
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

async fn check_nwaku(client: &MessagingClient) -> Result<()> {
    client.subscribe(&[OFFERS_TOPIC]).await.map_err(|_| {
        crate::error::SwapError::Messaging(format!(
            "cannot reach nwaku at {NWAKU_URL} — run `make nwaku` first"
        ))
    })
}
