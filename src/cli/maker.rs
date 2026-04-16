use clap::Args;

use crate::config::{SwapConfig, account_id_to_base58};
use crate::error::{Result, SwapError};
use crate::messaging::client::MessagingClient;
use crate::messaging::node::MessagingNodeConfig;
use crate::messaging::topics::{self, OFFERS_TOPIC};
use crate::messaging::types::SwapOffer;
use crate::swap::maker::run_maker;

use super::{create_clients, output};

#[derive(Args)]
pub struct MakerArgs {
    /// Accept a specific hashlock (64-char hex) instead of discovering via on-chain event
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

            if !json {
                println!("Using hashlock: {hex_str}");
            }

            Some(arr)
        }
        None => None,
    };

    // If messaging is enabled, publish a standing offer so takers can discover us.
    let messaging = if let Some(msg_cfg) = &config.messaging {
        let messaging = MessagingClient::spawn(MessagingNodeConfig {
            listen_port: msg_cfg.listen_port,
            node_key_path: None,
            bootstrap_peers: vec![msg_cfg.bootstrap_multiaddr.clone()],
        })
        .await?;

        let mut content_topics = vec![OFFERS_TOPIC.to_string()];
        if let Some(hl) = &hashlock {
            content_topics.push(topics::swap_topic(hl));
        }
        let topic_refs: Vec<&str> = content_topics.iter().map(|s| s.as_str()).collect();
        messaging.subscribe(&topic_refs).await?;

        let offer = SwapOffer {
            hashlock: hashlock.map_or_else(String::new, |hl| hex::encode(hl)),
            lez_amount: config.lez_amount,
            eth_amount: config.eth_amount,
            maker_eth_address: format!("{}", config.eth_recipient_address),
            maker_lez_account: account_id_to_base58(&lez_client.account_id()),
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

        if !json {
            println!("Offer published to Logos Messaging. Waiting for taker to lock ETH...");
        }
        Some(messaging)
    } else {
        if !json {
            println!("Waiting for taker to lock ETH...");
        }
        None
    };

    let outcome = run_maker(config, &eth_client, &lez_client, hashlock, None, None).await?;

    // Explicit shutdown — WakuNodeHandle has no Drop. See delivery-dogfooding.md #3.
    if let Some(m) = messaging {
        let _ = m.shutdown().await;
    }

    output::print_swap_outcome(&outcome, json);
    Ok(())
}
