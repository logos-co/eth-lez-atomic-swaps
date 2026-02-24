use clap::Args;
use sha2::{Digest, Sha256};

use crate::config::SwapConfig;
use crate::error::{Result, SwapError};
use crate::messaging::client::MessagingClient;
use crate::messaging::types::{self, SwapOffer, OFFERS_TOPIC};
use crate::swap::maker::run_maker;

use super::{create_clients, output};

#[derive(Args)]
pub struct MakerArgs {
    /// Use a specific preimage (64-char hex) instead of generating a random one
    #[arg(long)]
    preimage: Option<String>,
}

pub async fn cmd_maker(args: MakerArgs, config: &SwapConfig, json: bool) -> Result<()> {
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

    // Compute and display hashlock so the taker can use it.
    let preimage_for_display: [u8; 32] = override_preimage.unwrap_or_else(rand::random);
    let hashlock: [u8; 32] = Sha256::digest(preimage_for_display).into();

    if json {
        println!(
            "{}",
            serde_json::json!({
                "event": "hashlock_generated",
                "hashlock": hex::encode(hashlock),
            })
        );
    } else {
        println!("Hashlock: {}", hex::encode(hashlock));
    }

    // If messaging is enabled, publish the offer before starting the swap.
    // The taker discovers it and waits for the LEZ escrow to appear.
    if let Some(nwaku_url) = &config.nwaku_url {
        let messaging = MessagingClient::new(nwaku_url);
        let swap_topic = types::swap_topic(&hashlock);
        messaging.subscribe(&[OFFERS_TOPIC, &swap_topic]).await?;

        let offer = SwapOffer {
            hashlock: hex::encode(hashlock),
            lez_amount: config.lez_amount,
            eth_amount: config.eth_amount,
            maker_eth_address: format!("{}", config.eth_recipient_address),
            maker_lez_account: hex::encode(lez_client.account_id().value()),
            lez_timelock: config.lez_timelock,
            eth_timelock: config.eth_timelock,
            lez_htlc_program_id: hex::encode(
                config.lez_htlc_program_id.iter()
                    .flat_map(|w| w.to_le_bytes())
                    .collect::<Vec<u8>>(),
            ),
            eth_htlc_address: format!("{}", config.eth_htlc_address),
        };

        messaging.publish(OFFERS_TOPIC, &offer).await?;

        if json {
            println!(
                "{}",
                serde_json::json!({
                    "event": "offer_published",
                    "topic": OFFERS_TOPIC,
                })
            );
        } else {
            println!("Offer published to Logos Messaging. Waiting for swap...");
        }
    } else {
        if !json {
            println!("Share this with the taker, then waiting for swap to complete...");
        }
    }

    let outcome =
        run_maker(config, &eth_client, &lez_client, Some(preimage_for_display), None).await?;

    output::print_swap_outcome(&outcome, json);
    Ok(())
}
