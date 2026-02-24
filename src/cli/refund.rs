use alloy::primitives::FixedBytes;
use clap::{Args, Subcommand};

use crate::config::SwapConfig;
use crate::error::Result;
use crate::swap::refund::{refund_eth, refund_lez};

use super::create_clients;
use super::taker::parse_bytes32;

#[derive(Args)]
pub struct RefundArgs {
    #[command(subcommand)]
    chain: RefundChain,
}

#[derive(Subcommand)]
pub enum RefundChain {
    /// Refund LEZ from an expired HTLC escrow
    Lez(RefundLezArgs),
    /// Refund ETH from an expired HTLC
    Eth(RefundEthArgs),
}

#[derive(Args)]
pub struct RefundLezArgs {
    /// Hashlock identifying the LEZ escrow (64-char hex)
    #[arg(long)]
    hashlock: String,
}

#[derive(Args)]
pub struct RefundEthArgs {
    /// Swap ID of the ETH HTLC (64-char hex)
    #[arg(long)]
    swap_id: String,
}

pub async fn cmd_refund(args: RefundArgs, config: &SwapConfig, json: bool) -> Result<()> {
    match args.chain {
        RefundChain::Lez(lez_args) => {
            let hashlock = parse_bytes32(&lez_args.hashlock, "hashlock")?;
            let (_eth_client, lez_client) = create_clients(config).await?;

            let tx_hash = refund_lez(&lez_client, &hashlock, config.lez_timelock).await?;

            if json {
                println!(
                    "{}",
                    serde_json::json!({
                        "chain": "LEZ",
                        "action": "refund",
                        "tx_hash": tx_hash,
                    })
                );
            } else {
                println!("LEZ refund submitted: {tx_hash}");
            }
        }
        RefundChain::Eth(eth_args) => {
            let swap_id_bytes = parse_bytes32(&eth_args.swap_id, "swap-id")?;
            let swap_id = FixedBytes::from(swap_id_bytes);
            let (eth_client, _lez_client) = create_clients(config).await?;

            let tx_hash = refund_eth(&eth_client, swap_id).await?;

            if json {
                println!(
                    "{}",
                    serde_json::json!({
                        "chain": "ETH",
                        "action": "refund",
                        "tx_hash": format!("{tx_hash}"),
                    })
                );
            } else {
                println!("ETH refund submitted: {tx_hash}");
            }
        }
    }

    Ok(())
}
