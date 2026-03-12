use std::time::Duration;

use clap::Args;
use tracing::{debug, info};

use crate::config::SwapConfig;
use crate::error::{Result, SwapError};
use crate::messaging::client::{MessagingClient, decode_waku_payload};
use crate::messaging::types::{SwapOffer, OFFERS_TOPIC};
use crate::swap::taker::run_taker;

use super::{create_clients, output};

#[derive(Args)]
pub struct TakerArgs {
    /// Use a specific preimage (64-char hex) instead of generating a random one
    #[arg(long)]
    preimage: Option<String>,
}

pub async fn cmd_taker(args: TakerArgs, config: &SwapConfig, json: bool) -> Result<()> {
    let (eth_client, lez_client) = create_clients(config).await?;

    let override_preimage = match &args.preimage {
        Some(hex_str) => {
            let bytes = hex::decode(hex_str).map_err(|e| {
                SwapError::InvalidConfig(format!("invalid preimage hex: {e}"))
            })?;
            let arr: [u8; 32] = bytes.try_into().map_err(|_| {
                SwapError::InvalidConfig("preimage must be 32 bytes (64 hex chars)".into())
            })?;
            Some(arr)
        }
        None => None,
    };

    // Discover offer via messaging if available.
    if let Some(nwaku_url) = &config.nwaku_url {
        discover_offer(nwaku_url, config, json).await?;
    }

    if !json {
        println!("Starting taker swap — generating preimage and locking ETH...");
    }

    let outcome = run_taker(config, &eth_client, &lez_client, override_preimage, None).await?;

    output::print_swap_outcome(&outcome, json);
    Ok(())
}

/// Discover a matching swap offer via Logos Messaging.
/// Returns once a valid offer is found (doesn't need to wait for escrow — taker locks first now).
async fn discover_offer(
    nwaku_url: &str,
    config: &SwapConfig,
    json: bool,
) -> Result<()> {
    let messaging = MessagingClient::new(nwaku_url);
    messaging.subscribe(&[OFFERS_TOPIC]).await?;

    if !json {
        println!("Listening for swap offers via Logos Messaging...");
    }

    // 1. Query store for recent offers (last 10 minutes).
    let now_ns = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos() as i64;
    let ten_min_ns: i64 = 10 * 60 * 1_000_000_000;
    let start_time_ns = now_ns - ten_min_ns;

    let offer = 'search: {
        // Try store first.
        let store_entries = messaging
            .store_query(&[OFFERS_TOPIC], Some(start_time_ns), Some(50))
            .await;

        if let Ok(entries) = store_entries {
            for entry in &entries {
                if let Some(ref waku_msg) = entry.message {
                    if let Ok(offer) = decode_waku_payload::<SwapOffer>(&waku_msg.payload) {
                        if offer.lez_amount == config.lez_amount
                            && offer.eth_amount == config.eth_amount
                        {
                            info!(hashlock = %offer.hashlock, "found matching offer in store");
                            break 'search offer;
                        }
                    }
                }
            }
        }

        debug!("no matching offer in store, polling relay...");

        // 2. Poll relay cache in a loop (5 min timeout).
        let deadline = tokio::time::Instant::now() + Duration::from_secs(300);
        loop {
            let offers: Vec<SwapOffer> = messaging.poll_messages(OFFERS_TOPIC).await?;
            for offer in offers {
                if offer.lez_amount == config.lez_amount
                    && offer.eth_amount == config.eth_amount
                {
                    info!(hashlock = %offer.hashlock, "found matching offer via relay");
                    break 'search offer;
                }
            }

            if tokio::time::Instant::now() >= deadline {
                return Err(SwapError::Timeout(
                    "no matching offer found within 5 minutes".into(),
                ));
            }

            tokio::time::sleep(config.poll_interval).await;
        }
    };

    if json {
        println!(
            "{}",
            serde_json::json!({
                "event": "offer_discovered",
                "lez_amount": offer.lez_amount,
                "eth_amount": offer.eth_amount,
            })
        );
    } else {
        println!(
            "Discovered offer — {} LEZ for {} wei",
            offer.lez_amount, offer.eth_amount,
        );
    }

    Ok(())
}

pub fn parse_bytes32(hex_str: &str, name: &str) -> Result<[u8; 32]> {
    let hex_str = hex_str.strip_prefix("0x").unwrap_or(hex_str);
    let bytes = hex::decode(hex_str)
        .map_err(|e| SwapError::InvalidConfig(format!("invalid {name} hex: {e}")))?;
    bytes
        .try_into()
        .map_err(|_| SwapError::InvalidConfig(format!("{name} must be 32 bytes (64 hex chars)")))
}
