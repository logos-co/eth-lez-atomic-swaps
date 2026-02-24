use alloy::primitives::FixedBytes;
use clap::Args;

use crate::config::SwapConfig;
use crate::error::{Result, SwapError};
use crate::eth::client::EthHTLC::SwapState;

use super::{create_clients, output};
use super::taker::parse_bytes32;

#[derive(Args)]
pub struct StatusArgs {
    /// Hashlock to look up LEZ escrow state (64-char hex)
    #[arg(long)]
    hashlock: Option<String>,

    /// Swap ID to look up ETH HTLC state (64-char hex)
    #[arg(long)]
    swap_id: Option<String>,
}

pub async fn cmd_status(args: StatusArgs, config: &SwapConfig, json: bool) -> Result<()> {
    if args.hashlock.is_none() && args.swap_id.is_none() {
        return Err(SwapError::InvalidConfig(
            "at least one of --hashlock or --swap-id is required".into(),
        ));
    }

    let (eth_client, lez_client) = create_clients(config).await?;

    if let Some(hashlock_hex) = &args.hashlock {
        let hashlock = parse_bytes32(hashlock_hex, "hashlock")?;
        match lez_client.get_escrow(&hashlock).await? {
            Some(escrow) => output::print_escrow(&escrow, json),
            None => {
                if json {
                    println!(
                        "{}",
                        serde_json::json!({
                            "chain": "LEZ",
                            "state": "not_found",
                            "hashlock": hex::encode(hashlock),
                        })
                    );
                } else {
                    println!("LEZ Escrow: not found for hashlock {}", hex::encode(hashlock));
                }
            }
        }
    }

    if let Some(swap_id_hex) = &args.swap_id {
        let swap_id_bytes = parse_bytes32(swap_id_hex, "swap-id")?;
        let swap_id = FixedBytes::from(swap_id_bytes);
        let htlc = eth_client.get_htlc(swap_id).await?;

        if matches!(htlc.state, SwapState::EMPTY) {
            if json {
                println!(
                    "{}",
                    serde_json::json!({
                        "chain": "ETH",
                        "state": "not_found",
                        "swap_id": format!("{swap_id}"),
                    })
                );
            } else {
                println!("ETH HTLC: not found for swap ID {swap_id}");
            }
        } else {
            output::print_htlc(&htlc, swap_id, json);
        }
    }

    Ok(())
}
