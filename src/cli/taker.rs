use clap::Args;

use crate::config::SwapConfig;
use crate::error::{Result, SwapError};
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

    if !json {
        println!("Starting taker swap — generating preimage and locking ETH...");
    }

    let outcome = run_taker(config, &eth_client, &lez_client, override_preimage, None).await?;

    output::print_swap_outcome(&outcome, json);
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
