#[cfg(feature = "demo")]
mod demo;
#[cfg(feature = "demo")]
mod infra;
mod maker;
mod output;
mod refund;
mod status;
mod taker;

use std::time::Duration;

use alloy::primitives::Address;
use clap::{Args, Parser, Subcommand};

use crate::config::{LezAuth, SwapConfig, eth_to_wei, parse_base58_account_id, parse_program_id};
use crate::error::{Result, SwapError};
use crate::eth::client::EthClient;
use crate::lez::client::LezClient;
use crate::swap::refund::now_unix;

#[derive(Parser)]
#[command(name = "swap-cli", about = "Atomic swap CLI (LEZ <-> ETH)")]
pub struct Cli {
    #[command(subcommand)]
    command: Commands,

    /// Output as JSON (for scripting)
    #[arg(long, global = true)]
    json: bool,

    /// Override .env file path (default: .env)
    #[arg(long, global = true)]
    env_file: Option<String>,

    #[command(flatten)]
    config: ConfigArgs,
}

/// Infrastructure config — typically set via .env, overridable via CLI.
#[derive(Args, Clone)]
pub struct ConfigArgs {
    /// Ethereum WebSocket RPC URL
    #[arg(long, env = "ETH_RPC_URL", hide_env_values = true)]
    eth_rpc_url: String,

    /// Ethereum private key (hex)
    #[arg(long, env = "ETH_PRIVATE_KEY", hide_env_values = true)]
    eth_private_key: String,

    /// Deployed EthHTLC contract address
    #[arg(long, env = "ETH_HTLC_ADDRESS")]
    eth_htlc_address: String,

    /// LEZ signing key (32-byte hex) — used when not using scaffold wallet.
    /// Mutually exclusive with LEZ_WALLET_HOME + LEZ_ACCOUNT_ID.
    #[arg(long, env = "LEZ_SIGNING_KEY", hide_env_values = true)]
    lez_signing_key: Option<String>,

    /// Scaffold wallet home directory (e.g. `.scaffold/wallet`).
    /// When set, the wallet manages keys — LEZ_SIGNING_KEY is not needed.
    #[arg(long, env = "LEZ_WALLET_HOME")]
    lez_wallet_home: Option<String>,

    /// LEZ account ID (base58) to sign with from the scaffold wallet.
    /// Required when LEZ_WALLET_HOME is set.
    #[arg(long, env = "LEZ_ACCOUNT_ID")]
    lez_account_id: Option<String>,

    /// LEZ sequencer URL
    #[arg(long, env = "LEZ_SEQUENCER_URL", default_value = "http://127.0.0.1:3040")]
    lez_sequencer_url: String,

    /// LEZ HTLC program ID (32-byte hex)
    #[arg(long, env = "LEZ_HTLC_PROGRAM_ID")]
    lez_htlc_program_id: String,

    /// Maker's ETH address (receives ETH from the swap)
    #[arg(long, env = "ETH_RECIPIENT_ADDRESS")]
    eth_recipient: String,

    /// Taker's LEZ account ID (base58)
    #[arg(long, env = "LEZ_TAKER_ACCOUNT_ID")]
    lez_taker_account: String,

    /// Amount of LEZ to swap
    #[arg(long, env = "LEZ_AMOUNT")]
    lez_amount: u128,

    /// Amount of ETH to swap (e.g. "0.001")
    #[arg(long, env = "ETH_AMOUNT")]
    eth_amount: String,

    /// LEZ timelock duration in minutes (from now). Shorter — maker locks second.
    #[arg(long, env = "LEZ_TIMELOCK_MINUTES", default_value = "5")]
    lez_timelock_minutes: u64,

    /// ETH timelock duration in minutes (from now). Longer — taker locks first.
    #[arg(long, env = "ETH_TIMELOCK_MINUTES", default_value = "10")]
    eth_timelock_minutes: u64,

    /// Polling interval in milliseconds
    #[arg(long, env = "POLL_INTERVAL_MS", default_value = "2000")]
    poll_interval_ms: u64,
}

impl ConfigArgs {
    fn into_swap_config(self) -> Result<SwapConfig> {
        let eth_htlc_address: Address = self
            .eth_htlc_address
            .parse()
            .map_err(|e| SwapError::InvalidConfig(format!("invalid eth-htlc-address: {e}")))?;

        let eth_recipient_address: Address = self
            .eth_recipient
            .parse()
            .map_err(|e| SwapError::InvalidConfig(format!("invalid eth-recipient: {e}")))?;

        let lez_htlc_program_id = parse_program_id(&self.lez_htlc_program_id)?;
        let lez_taker_account_id = parse_base58_account_id(&self.lez_taker_account)?;

        // Determine LEZ auth mode: wallet (scaffold) or raw key.
        let lez_auth = match (self.lez_wallet_home, self.lez_account_id, self.lez_signing_key) {
            (Some(home), Some(account_id_str), _) => {
                let account_id = parse_base58_account_id(&account_id_str)?;
                LezAuth::Wallet {
                    home: std::path::PathBuf::from(home),
                    account_id,
                }
            }
            (None, None, Some(key)) => LezAuth::RawKey(key),
            (Some(_), None, _) => {
                return Err(SwapError::InvalidConfig(
                    "LEZ_WALLET_HOME requires LEZ_ACCOUNT_ID".into(),
                ));
            }
            (None, Some(_), _) => {
                return Err(SwapError::InvalidConfig(
                    "LEZ_ACCOUNT_ID requires LEZ_WALLET_HOME".into(),
                ));
            }
            (None, None, None) => {
                return Err(SwapError::InvalidConfig(
                    "either LEZ_SIGNING_KEY or LEZ_WALLET_HOME + LEZ_ACCOUNT_ID must be set".into(),
                ));
            }
        };

        let now = now_unix();

        Ok(SwapConfig {
            eth_rpc_url: self.eth_rpc_url,
            eth_private_key: self.eth_private_key,
            eth_htlc_address,
            lez_sequencer_url: self.lez_sequencer_url,
            lez_auth,
            lez_htlc_program_id,
            lez_amount: self.lez_amount,
            eth_amount: eth_to_wei(&self.eth_amount)
                .map_err(|e| SwapError::InvalidConfig(e))?,
            lez_timelock: now + self.lez_timelock_minutes * 60,
            eth_timelock: now + self.eth_timelock_minutes * 60,
            eth_recipient_address,
            lez_taker_account_id,
            poll_interval: Duration::from_millis(self.poll_interval_ms),
        })
    }
}

#[derive(Subcommand)]
enum Commands {
    /// Run the maker flow: watch for ETH lock, lock LEZ, watch for preimage, claim ETH
    Maker(maker::MakerArgs),
    /// Run the taker flow: generate preimage, lock ETH, watch for LEZ lock, claim LEZ
    Taker(taker::TakerArgs),
    /// Refund expired HTLCs
    Refund(refund::RefundArgs),
    /// Inspect escrow/HTLC state on-chain
    Status(status::StatusArgs),
    /// Run a full demo: start local chains, deploy contracts, run both sides
    #[cfg(feature = "demo")]
    Demo,
    /// Start infrastructure (Anvil + LEZ sequencer + nwaku), write .env files, block until Ctrl-C
    #[cfg(feature = "demo")]
    Infra,
}

async fn create_clients(config: &SwapConfig) -> Result<(EthClient, LezClient)> {
    let eth_client = EthClient::new(config).await?;
    let lez_client = LezClient::new(config)?;
    Ok((eth_client, lez_client))
}

pub async fn run() -> Result<()> {
    // Short-circuit: the demo subcommand generates all config internally —
    // skip .env loading and ConfigArgs parsing entirely.
    #[cfg(feature = "demo")]
    {
        let args: Vec<String> = std::env::args().collect();
        if args.iter().any(|a| a == "demo") {
            return demo::cmd_demo().await;
        }
        if args.iter().any(|a| a == "infra") {
            return infra::cmd_infra().await;
        }
    }

    // Default tracing for non-infra/demo subcommands (infra/demo set up their own).
    tracing_subscriber::fmt::init();

    // Check for --env-file before full parse so dotenvy loads first.
    // This ensures env vars are available when clap reads `env = "..."` fallbacks.
    let env_file = std::env::args()
        .zip(std::env::args().skip(1))
        .find(|(k, _)| k == "--env-file")
        .map(|(_, v)| v);

    if let Some(path) = &env_file {
        dotenvy::from_filename(path).map_err(|e| {
            SwapError::InvalidConfig(format!("failed to load env file '{path}': {e}"))
        })?;
    } else {
        dotenvy::dotenv().ok();
    }

    let cli = Cli::parse();
    let config = cli.config.into_swap_config()?;

    match cli.command {
        Commands::Maker(args) => maker::cmd_maker(args, &config, cli.json).await,
        Commands::Taker(args) => taker::cmd_taker(args, &config, cli.json).await,
        Commands::Refund(args) => refund::cmd_refund(args, &config, cli.json).await,
        Commands::Status(args) => status::cmd_status(args, &config, cli.json).await,
        #[cfg(feature = "demo")]
        Commands::Demo => unreachable!("handled above"),
        #[cfg(feature = "demo")]
        Commands::Infra => unreachable!("handled above"),
    }
}
