use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use clap::Args;
use tokio::sync::mpsc;
use tracing::warn;

use crate::config::{SwapConfig, account_id_to_base58};
use crate::error::{Result, SwapError};
use crate::lez::client::LezClient;
use crate::swap::maker::{AutoAcceptConfig, run_maker, run_maker_loop};
use crate::swap::progress::SwapProgress;

use super::{bot, create_clients, output};

/// Default publisher script path, relative to the working directory
/// (repo checkout layout).
const DEFAULT_PUBLISHER_SCRIPT: &str = "web/offer-board/publish-offer.mjs";

#[derive(Args)]
pub struct MakerArgs {
    /// Accept a specific hashlock (64-char hex) instead of discovering via on-chain event
    #[arg(long)]
    hashlock: Option<String>,

    /// Run the auto-accept maker loop as a standing liquidity bot:
    /// startup guards, crash-recovery reconciliation, heartbeat offer
    /// republish, and continuous swap acceptance until Ctrl-C or out of funds.
    #[arg(long = "loop")]
    loop_mode: bool,

    /// Heartbeat interval (seconds) for republishing the offer over Waku
    /// lightpush. The fleet runs store=false, so late-joining board viewers
    /// only see live messages — keep this at 30-60s. 0 disables publishing.
    #[arg(long, env = "OFFER_HEARTBEAT_SECS", default_value_t = 45)]
    heartbeat_secs: u64,

    /// Node.js offer publisher script (long-lived @waku/sdk lightpush
    /// sidecar). Defaults to web/offer-board/publish-offer.mjs if present.
    #[arg(long, env = "OFFER_PUBLISHER_SCRIPT")]
    publisher_script: Option<String>,

    /// JSON journal of in-flight swaps for crash recovery.
    #[arg(long, env = "MAKER_STATE_FILE", default_value = ".maker-state.json")]
    state_file: String,

    /// Required safety margin (minutes) between the ETH (long) and LEZ
    /// (short) timelocks. Startup fails if ETH < LEZ + margin.
    #[arg(long, env = "TIMELOCK_MARGIN_MINUTES", default_value_t = 5)]
    timelock_margin_minutes: u64,

    /// Faucet sidecar: loop `wallet pinata claim` (150 LEZ each) until the
    /// maker LEZ balance reaches this target. Standalone (exits when reached)
    /// unless combined with --loop, where it tops up before the loop starts.
    #[arg(long, env = "FUND_TO_TARGET", value_name = "TARGET")]
    fund_to: Option<u128>,

    /// Path to the LEZ `wallet` binary used for pinata claims.
    #[arg(long, env = "LEZ_WALLET_BIN", default_value = "wallet")]
    wallet_bin: String,
}

pub async fn cmd_maker(
    args: MakerArgs,
    config: &SwapConfig,
    timelock_minutes: (u64, u64),
    json: bool,
) -> Result<()> {
    // Faucet-only sidecar mode: fund to target, then exit.
    if let Some(target) = args.fund_to
        && !args.loop_mode
    {
        let balance = bot::fund_to_target(config, &args.wallet_bin, target, json).await?;
        if json {
            println!("{}", serde_json::json!({ "balance": balance.to_string() }));
        }
        return Ok(());
    }

    if args.loop_mode {
        return cmd_maker_loop(args, config, timelock_minutes, json).await;
    }

    // Single-shot maker (original behaviour).
    let (eth_client, lez_client) = create_clients(config).await?;

    let hashlock = match args.hashlock {
        Some(hex_str) => {
            let bytes = hex::decode(&hex_str)
                .map_err(|e| SwapError::InvalidConfig(format!("invalid hashlock hex: {e}")))?;
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

    if !json {
        println!("Waiting for taker to lock ETH...");
    }

    let outcome = run_maker(config, &eth_client, &lez_client, hashlock, None, None).await?;

    output::print_swap_outcome(&outcome, json);
    Ok(())
}

/// The standing liquidity bot: `swap-cli maker --loop`.
async fn cmd_maker_loop(
    args: MakerArgs,
    config: &SwapConfig,
    (lez_minutes, eth_minutes): (u64, u64),
    json: bool,
) -> Result<()> {
    if args.hashlock.is_some() {
        return Err(SwapError::InvalidConfig(
            "--hashlock cannot be combined with --loop (the loop discovers hashlocks on-chain)"
                .into(),
        ));
    }

    // Startup guard 1: timelock safety invariant.
    bot::validate_timelocks(lez_minutes, eth_minutes, args.timelock_margin_minutes)?;

    let lez_client = LezClient::new(config)?;
    let maker_account = lez_client.account_id();

    // Optional pre-loop top-up.
    if let Some(target) = args.fund_to {
        bot::fund_to_target(config, &args.wallet_bin, target, json).await?;
    }

    // Startup guard 2: LEZ inventory.
    let balance = lez_client.get_balance(&maker_account).await?;
    if balance < config.lez_amount {
        return Err(SwapError::InvalidConfig(format!(
            "insufficient LEZ inventory: balance {balance} < offer amount {}; \
             top up with `swap-cli maker --fund-to <target>` or `wallet pinata claim`",
            config.lez_amount
        )));
    }

    // Crash recovery: reconcile journaled in-flight swaps.
    let store = Arc::new(bot::StateStore::load(&args.state_file)?);
    bot::reconcile(config, &store, json).await;

    let cancel = Arc::new(AtomicBool::new(false));

    // Ctrl-C → graceful stop between/within iterations.
    {
        let cancel = cancel.clone();
        tokio::spawn(async move {
            if tokio::signal::ctrl_c().await.is_ok() {
                eprintln!("Ctrl-C received — stopping after current wait...");
                cancel.store(true, Ordering::Relaxed);
            }
        });
    }

    // Heartbeat offer publisher (best-effort sidecar).
    let publisher_handle = if args.heartbeat_secs == 0 {
        warn!("offer heartbeat disabled (--heartbeat-secs 0) — board viewers will not see this offer");
        None
    } else {
        let script = args
            .publisher_script
            .clone()
            .map(PathBuf::from)
            .or_else(|| {
                let default = PathBuf::from(DEFAULT_PUBLISHER_SCRIPT);
                default.exists().then_some(default)
            });
        match script {
            Some(script) if script.exists() => {
                let mut env =
                    bot::OfferPublisherEnv::from_config(config, lez_minutes, eth_minutes);
                env.maker_lez_account = account_id_to_base58(&maker_account);
                Some(bot::spawn_offer_publisher(
                    script,
                    env,
                    args.heartbeat_secs,
                    cancel.clone(),
                ))
            }
            Some(script) => {
                warn!(
                    "offer publisher script not found at {} — offers will NOT be broadcast",
                    script.display()
                );
                None
            }
            None => {
                warn!(
                    "no offer publisher script (looked for {DEFAULT_PUBLISHER_SCRIPT}); \
                     offers will NOT be broadcast — set OFFER_PUBLISHER_SCRIPT"
                );
                None
            }
        }
    };

    if !json {
        println!(
            "Maker loop started: {} LEZ -> {} wei per swap, timelocks LEZ {}m / ETH {}m, \
             balance {balance} LEZ",
            config.lez_amount, config.eth_amount, lez_minutes, eth_minutes
        );
    }

    // Progress drain: prints events and journals in-flight swaps.
    let (tx, mut rx) = mpsc::unbounded_channel::<SwapProgress>();
    let drain_store = store.clone();
    let drain = tokio::spawn(async move {
        // The loop runs swaps sequentially, so at most one in-flight hashlock.
        let mut current: Option<String> = None;
        while let Some(event) = rx.recv().await {
            if json {
                if let Ok(line) = serde_json::to_string(&event) {
                    println!("{line}");
                }
            } else {
                println!("[maker-loop] {}", describe(&event));
            }
            match &event {
                SwapProgress::EthLockDetected { swap_id, hashlock } => {
                    current = Some(hashlock.clone());
                    drain_store.add(bot::InFlightSwap {
                        hashlock: hashlock.clone(),
                        swap_id: swap_id.clone(),
                        recorded_at: crate::swap::refund::now_unix(),
                    });
                }
                SwapProgress::AutoAcceptSwapCompleted { .. }
                | SwapProgress::AutoAcceptSwapFailed { .. }
                | SwapProgress::AutoAcceptStopped { .. } => {
                    if let Some(hashlock) = current.take() {
                        drain_store.remove(&hashlock);
                    }
                }
                _ => {}
            }
        }
    });

    let auto_config = AutoAcceptConfig {
        lez_timelock_minutes: lez_minutes,
        eth_timelock_minutes: eth_minutes,
    };
    let result = run_maker_loop(config, &auto_config, &cancel, Some(tx)).await;

    // tx was moved into the loop and dropped on return — drain ends naturally.
    let _ = drain.await;
    cancel.store(true, Ordering::Relaxed);
    if let Some(handle) = publisher_handle {
        handle.abort(); // kill_on_drop reaps the node child
    }

    if json {
        println!(
            "{}",
            serde_json::json!({
                "total_completed": result.total_completed,
                "total_failed": result.total_failed,
            })
        );
    } else {
        println!(
            "Maker loop stopped: {} completed, {} failed",
            result.total_completed, result.total_failed
        );
    }
    Ok(())
}

/// Human-readable one-liner for a progress event.
fn describe(event: &SwapProgress) -> String {
    match event {
        SwapProgress::WaitingForEthLock => "waiting for taker to lock ETH...".into(),
        SwapProgress::EthLockDetected { swap_id, hashlock } => {
            format!("ETH lock detected (swap {swap_id}, hashlock {hashlock})")
        }
        SwapProgress::LezLocking => "locking LEZ escrow...".into(),
        SwapProgress::LezLocked { tx_hash } => format!("LEZ locked ({tx_hash})"),
        SwapProgress::WaitingForPreimage => "waiting for taker to claim LEZ...".into(),
        SwapProgress::PreimageRevealed { .. } => "preimage revealed".into(),
        SwapProgress::ClaimingEth => "claiming ETH...".into(),
        SwapProgress::EthClaimed { tx_hash } => format!("ETH claimed ({tx_hash})"),
        SwapProgress::TimelockExpired => "timelock expired".into(),
        SwapProgress::Refunding => "refunding LEZ...".into(),
        SwapProgress::RefundComplete => "LEZ refund complete".into(),
        SwapProgress::AutoAcceptStarted => "auto-accept loop started".into(),
        SwapProgress::AutoAcceptIteration { iteration } => {
            format!("iteration {iteration}: publishing fresh offer, waiting for taker")
        }
        SwapProgress::AutoAcceptSwapCompleted { iteration, status } => {
            format!("iteration {iteration}: swap {status}")
        }
        SwapProgress::AutoAcceptSwapFailed { iteration, error } => {
            format!("iteration {iteration}: failed — {error}")
        }
        SwapProgress::AutoAcceptInsufficientFunds {
            lez_balance,
            lez_required,
        } => format!(
            "out of LEZ inventory ({lez_balance} < {lez_required}) — loop stopping; \
             top up with --fund-to or `wallet pinata claim`"
        ),
        SwapProgress::AutoAcceptStopped {
            total_completed,
            total_failed,
        } => format!("loop stopped ({total_completed} completed, {total_failed} failed)"),
        SwapProgress::AutoAcceptCancelled => "loop cancelled".into(),
        other => format!("{other:?}"),
    }
}
