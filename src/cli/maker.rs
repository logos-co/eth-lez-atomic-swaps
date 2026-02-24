use clap::Args;
use sha2::{Digest, Sha256};

use crate::config::SwapConfig;
use crate::error::{Result, SwapError};
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
        println!("Share this with the taker, then waiting for swap to complete...");
    }

    let outcome =
        run_maker(config, &eth_client, &lez_client, Some(preimage_for_display)).await?;

    output::print_swap_outcome(&outcome, json);
    Ok(())
}
