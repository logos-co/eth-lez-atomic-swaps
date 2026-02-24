use std::time::Duration;

use clap::Args;
use tracing::{debug, info};

use crate::config::SwapConfig;
use crate::error::{Result, SwapError};
use crate::lez::client::LezClient;
use crate::messaging::client::{MessagingClient, decode_waku_payload};
use crate::messaging::types::{self, SwapAccept, SwapOffer, OFFERS_TOPIC};
use crate::swap::taker::run_taker;

use super::{create_clients, output};

#[derive(Args)]
pub struct TakerArgs {
    /// Hashlock from the maker (64-char hex). Optional when --nwaku-url is set.
    #[arg(long)]
    hashlock: Option<String>,
}

pub async fn cmd_taker(args: TakerArgs, config: &SwapConfig, json: bool) -> Result<()> {
    let (eth_client, lez_client) = create_clients(config).await?;

    let hashlock = match args.hashlock {
        // Explicit hashlock provided — use it directly (existing behavior).
        Some(hex_str) => parse_bytes32(&hex_str, "hashlock")?,

        // No hashlock — discover via messaging.
        None => {
            let nwaku_url = config.nwaku_url.as_deref().ok_or_else(|| {
                SwapError::InvalidConfig(
                    "either --hashlock or --nwaku-url is required".into(),
                )
            })?;

            discover_offer(nwaku_url, config, &lez_client, json).await?
        }
    };

    let outcome = run_taker(config, &eth_client, &lez_client, hashlock, None).await?;

    output::print_swap_outcome(&outcome, json);
    Ok(())
}

/// Discover a matching swap offer via Logos Messaging, wait for the LEZ
/// escrow to appear, publish a `SwapAccept`, and return the hashlock.
async fn discover_offer(
    nwaku_url: &str,
    config: &SwapConfig,
    lez_client: &LezClient,
    json: bool,
) -> Result<[u8; 32]> {
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

    let hashlock = parse_bytes32(&offer.hashlock, "offer hashlock")?;

    if json {
        println!(
            "{}",
            serde_json::json!({
                "event": "offer_discovered",
                "hashlock": offer.hashlock,
                "lez_amount": offer.lez_amount,
                "eth_amount": offer.eth_amount,
            })
        );
    } else {
        println!("Discovered offer — hashlock: {}", offer.hashlock);
    }

    // 3. Wait for LEZ escrow to appear (maker may still be locking).
    if !json {
        println!("Waiting for LEZ escrow to be funded...");
    }

    let escrow_deadline = tokio::time::Instant::now() + Duration::from_secs(300);
    loop {
        match lez_client.get_escrow(&hashlock).await? {
            Some(escrow) if escrow.amount >= config.lez_amount => {
                info!("LEZ escrow is funded, proceeding");
                break;
            }
            _ => {}
        }

        if tokio::time::Instant::now() >= escrow_deadline {
            return Err(SwapError::Timeout(
                "LEZ escrow did not appear within 5 minutes".into(),
            ));
        }

        tokio::time::sleep(config.poll_interval).await;
    }

    // 4. Subscribe to swap-specific topic and publish accept (informational).
    let swap_topic = types::swap_topic(&hashlock);
    messaging.subscribe(&[&swap_topic]).await?;

    let accept = SwapAccept {
        hashlock: hex::encode(hashlock),
        eth_swap_id: String::new(), // filled after ETH lock, informational only
        taker_lez_account: hex::encode(lez_client.account_id().value()),
        taker_eth_address: format!("{}", config.eth_recipient_address),
    };

    // Best-effort publish; don't fail the swap if messaging hiccups.
    if let Err(e) = messaging.publish(&swap_topic, &accept).await {
        tracing::warn!("failed to publish SwapAccept: {e}");
    }

    Ok(hashlock)
}

pub fn parse_bytes32(hex_str: &str, name: &str) -> Result<[u8; 32]> {
    let hex_str = hex_str.strip_prefix("0x").unwrap_or(hex_str);
    let bytes = hex::decode(hex_str)
        .map_err(|e| SwapError::InvalidConfig(format!("invalid {name} hex: {e}")))?;
    bytes
        .try_into()
        .map_err(|_| SwapError::InvalidConfig(format!("{name} must be 32 bytes (64 hex chars)")))
}
