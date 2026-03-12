use clap::Args;

use crate::config::SwapConfig;
use crate::error::{Result, SwapError};
use crate::messaging::client::MessagingClient;
use crate::messaging::types::{self, SwapOffer, OFFERS_TOPIC};
use crate::swap::maker::run_maker;

use super::{create_clients, output};

#[derive(Args)]
pub struct MakerArgs {
    /// Accept a specific hashlock (64-char hex) instead of discovering via messaging
    #[arg(long)]
    hashlock: Option<String>,
}

pub async fn cmd_maker(args: MakerArgs, config: &SwapConfig, json: bool) -> Result<()> {
    let (eth_client, lez_client) = create_clients(config).await?;

    let hashlock = match args.hashlock {
        Some(hex_str) => {
            let bytes = hex::decode(&hex_str).map_err(|e| {
                SwapError::InvalidConfig(format!("invalid hashlock hex: {e}"))
            })?;
            let arr: [u8; 32] = bytes.try_into().map_err(|_| {
                SwapError::InvalidConfig("hashlock must be 32 bytes (64 hex chars)".into())
            })?;

            if json {
                println!(
                    "{}",
                    serde_json::json!({
                        "event": "hashlock_provided",
                        "hashlock": hex_str,
                    })
                );
            } else {
                println!("Using hashlock: {hex_str}");
            }

            arr
        }
        None => {
            // Publish standing offer and wait for taker via on-chain ETH lock detection.
            // For now, require --hashlock when messaging is not fully wired for taker-locks-first.
            return Err(SwapError::InvalidConfig(
                "--hashlock is required (messaging-based discovery coming soon)".into(),
            ));
        }
    };

    // If messaging is enabled, publish a standing offer.
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
            println!("Offer published to Logos Messaging. Waiting for taker to lock ETH...");
        }
    } else if !json {
        println!("Waiting for taker to lock ETH...");
    }

    let outcome = run_maker(config, &eth_client, &lez_client, Some(hashlock), None).await?;

    output::print_swap_outcome(&outcome, json);
    Ok(())
}
