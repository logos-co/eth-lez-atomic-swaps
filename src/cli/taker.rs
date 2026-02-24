use clap::Args;

use crate::config::SwapConfig;
use crate::error::{Result, SwapError};
use crate::swap::taker::run_taker;

use super::{create_clients, output};

#[derive(Args)]
pub struct TakerArgs {
    /// Hashlock from the maker (64-char hex)
    #[arg(long)]
    hashlock: String,
}

pub async fn cmd_taker(args: TakerArgs, config: &SwapConfig, json: bool) -> Result<()> {
    let (eth_client, lez_client) = create_clients(config).await?;

    let hashlock = parse_bytes32(&args.hashlock, "hashlock")?;

    let outcome = run_taker(config, &eth_client, &lez_client, hashlock).await?;

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
