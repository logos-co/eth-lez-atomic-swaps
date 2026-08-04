use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use clap::Args;
use tokio::sync::mpsc;
use tracing::{error, warn};

use crate::config::{SwapConfig, account_id_to_base58};
use crate::error::{Result, SwapError};
use crate::lez::client::LezClient;
use crate::swap::maker::{AutoAcceptConfig, run_maker, run_maker_loop};
use crate::swap::progress::SwapProgress;

use super::{bot, create_clients, output};

/// Default publisher script path, relative to the working directory
/// (repo checkout layout).
const DEFAULT_PUBLISHER_SCRIPT: &str = "offer-publisher/publish-offer.mjs";

/// Boolish value parser for env/flag booleans (clap's default `bool` parser only
/// accepts `true`/`false`, but operators reasonably set `RESTRICT_COUNTERPARTY=1`).
/// Accepts `1`/`0`, `true`/`false`, `yes`/`no`, `on`/`off` (case-insensitive).
fn parse_boolish(s: &str) -> std::result::Result<bool, String> {
    match s.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "y" | "on" => Ok(true),
        "0" | "false" | "no" | "n" | "off" => Ok(false),
        other => Err(format!(
            "expected a boolean (1/0, true/false, yes/no, on/off), got '{other}'"
        )),
    }
}

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
    /// lightpush. The fleet runs store=false, so late-joining subscribers
    /// only see live messages — keep this at 30-60s. 0 disables publishing.
    #[arg(long, env = "OFFER_HEARTBEAT_SECS", default_value_t = 45)]
    heartbeat_secs: u64,

    /// Node.js offer publisher script (long-lived @waku/sdk lightpush
    /// sidecar). Defaults to offer-publisher/publish-offer.mjs if present.
    #[arg(long, env = "OFFER_PUBLISHER_SCRIPT")]
    publisher_script: Option<String>,

    /// JSON journal of in-flight swaps for crash recovery.
    #[arg(long, env = "MAKER_STATE_FILE", default_value = ".maker-state.json")]
    state_file: String,

    /// Required safety margin (minutes) between the ETH (long) and LEZ
    /// (short) timelocks. Startup fails if ETH < LEZ + margin.
    #[arg(long, env = "TIMELOCK_MARGIN_MINUTES", default_value_t = 5)]
    timelock_margin_minutes: u64,

    /// Restrict `--loop` to the single designated taker given by
    /// `--lez-taker-account` (env `LEZ_TAKER_ACCOUNT_ID`).
    ///
    /// This flag used to be MANDATORY, and its absence a hard startup failure:
    /// the LEZ HTLC gates `Claim` on `signer == taker_id`, and the loop had no
    /// way to learn a public taker's LEZ account per-swap, so every escrow was
    /// locked to one statically configured account and no other taker could
    /// claim. That is fixed — the taker now publishes its LEZ account in its own
    /// ETH lock (`Locked.takerLezAccount`) and the maker binds each escrow to
    /// that, so the default (flag unset) correctly serves anyone.
    ///
    /// Setting it turns `--lez-taker-account` into an allowlist for operators
    /// who deliberately want a private, one-counterparty maker.
    ///
    /// Accepts a boolish value from the env/flag (`1`/`0`, `true`/`false`,
    /// `yes`/`no`, `on`/`off`) so `RESTRICT_COUNTERPARTY=1` works, not just
    /// `=true`; a bare `--restrict-counterparty` (no value) means `true`.
    #[arg(
        long,
        env = "RESTRICT_COUNTERPARTY",
        value_parser = parse_boolish,
        num_args = 0..=1,
        default_value_t = false,
        default_missing_value = "true",
    )]
    restrict_counterparty: bool,

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
    timelock_minutes: (Option<u64>, Option<u64>),
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

    let outcome = run_maker(config, &eth_client, &lez_client, hashlock, None, None, None).await?;

    output::print_swap_outcome(&outcome, json);
    Ok(())
}

/// The standing liquidity bot: `swap-cli maker --loop`.
async fn cmd_maker_loop(
    args: MakerArgs,
    config: &SwapConfig,
    (lez_minutes_arg, eth_minutes_arg): (Option<u64>, Option<u64>),
    json: bool,
) -> Result<()> {
    if args.hashlock.is_some() {
        return Err(SwapError::InvalidConfig(
            "--hashlock cannot be combined with --loop (the loop discovers hashlocks on-chain)"
                .into(),
        ));
    }

    // Loop-mode timelock defaults are LONGER than single-shot (20/40 vs 5/10):
    // LEZ lock confirmation alone can take up to 300s on the public testnet, so
    // the short single-shot defaults would leave the standing bot almost no
    // margin. An explicit env/flag value (Some) always wins.
    let lez_minutes = lez_minutes_arg.unwrap_or(bot::LOOP_DEFAULT_LEZ_TIMELOCK_MINUTES);
    let eth_minutes = eth_minutes_arg.unwrap_or(bot::LOOP_DEFAULT_ETH_TIMELOCK_MINUTES);

    // Startup guard 1: timelock safety invariant.
    bot::validate_timelocks(lez_minutes, eth_minutes, args.timelock_margin_minutes)?;

    // Startup guard 2 (was P1-2): the loop no longer needs a designated
    // counterparty — the taker's LEZ account arrives on its own ETH lock and
    // each escrow is bound to it — so serving the public is now the default and
    // this is no longer a refusal. What remains is the inverse consistency
    // check: asking to restrict without naming anyone is a config error, and
    // silently serving everyone would be the opposite of what was asked.
    if args.restrict_counterparty && config.lez_taker_account_id.is_none() {
        return Err(SwapError::InvalidConfig(
            "--restrict-counterparty (RESTRICT_COUNTERPARTY) was set but no designated taker was \
             given: pass --lez-taker-account (LEZ_TAKER_ACCOUNT_ID) with the counterparty's base58 \
             LEZ account, or drop the flag to serve any taker (each taker now publishes its own \
             LEZ account in its ETH lock)."
                .into(),
        ));
    }
    if !args.restrict_counterparty && config.lez_taker_account_id.is_some() {
        warn!(
            "LEZ_TAKER_ACCOUNT_ID is set but --restrict-counterparty is not — it will be used as \
             an allowlist and locks from other takers will be rejected. Unset it to serve anyone."
        );
    }

    let lez_client = LezClient::new(config)?;
    let maker_account = lez_client.account_id();

    // Optional pre-loop top-up.
    if let Some(target) = args.fund_to {
        bot::fund_to_target(config, &args.wallet_bin, target, json).await?;
    }

    // Crash recovery FIRST (P1-3): reconcile journaled in-flight swaps before
    // the free-inventory guard, so an escrow that a previous crash left locked
    // is refunded/claimed (restoring balance) instead of wedging startup into a
    // restart-forever loop that never recovers the funds.
    let store = Arc::new(bot::StateStore::load(&args.state_file)?);
    let resume_handles = bot::reconcile(config, &store, json).await;

    // P1-A: fully RESOLVE all reconciled/resumed entries BEFORE the loop accepts
    // any new swap. Running resume watchers concurrently with the loop lets the
    // loop's 256-block replay rematch a still-live retained escrow's OPEN ETH
    // lock and double-fund it (LezClient::lock would see the existing PDA as its
    // own confirmation and transfer the amount a second time). Draining them
    // here makes startup sequential and closes that race structurally; the
    // journal-skip belt (run_maker) and lock() check-before-fund are backstops.
    let resumed = resume_handles.len();
    for handle in resume_handles {
        let _ = handle.await;
    }
    if resumed > 0 && !json {
        println!("Resolved {resumed} resumed in-flight swap(s) before accepting new swaps.");
    }

    // P1-4: quarantined entries are partial-lock wedges (a committed lock whose
    // funding never landed) that can never terminalize. They do NOT block startup
    // (they live in a separate section of the state file, excluded from
    // `snapshot()`), but each is permanently stranded LEZ whose hashlock/secret
    // must never be reused — surface them loudly on EVERY startup.
    let quarantined = store.quarantined_snapshot();
    if !quarantined.is_empty() {
        for q in &quarantined {
            error!(
                hashlock = %q.hashlock,
                swap_id = %q.swap_id,
                quarantined_at = q.quarantined_at,
                reason = %q.reason,
                "QUARANTINED partial-lock escrow — permanently unusable; do NOT reuse this secret"
            );
        }
        if !json {
            println!(
                "WARNING: {} quarantined partial-lock hashlock(s) in the state file — permanently \
                 unusable (funding never landed; see logs). Startup proceeds; these are never retried.",
                quarantined.len()
            );
        }
    }

    // If the journal STILL holds unresolved fund-bearing entries (reconcile
    // could not reach a terminal state — e.g. RPC/sequencer trouble), do NOT
    // start accepting new swaps: exit cleanly so the supervisor restarts and
    // reconciliation retries, rather than running the loop past unresolved funds
    // whose still-OPEN locks a fresh watcher could rematch.
    if !store.snapshot().is_empty() {
        warn!(
            "{} in-flight swap(s) still unresolved after reconciliation; stopping so the \
             supervisor restarts and retries (check RPC/sequencer connectivity)",
            store.snapshot().len()
        );
        return Ok(());
    }

    // Free-inventory guard: recovery is complete and the journal is empty, so a
    // shortfall now means we genuinely cannot fund a swap — refuse to start.
    let balance = lez_client.get_balance(&maker_account).await?;
    if balance < config.lez_amount {
        return Err(SwapError::InvalidConfig(format!(
            "insufficient LEZ inventory: balance {balance} < offer amount {}; \
             top up with `swap-cli maker --fund-to <target>` or `wallet pinata claim`",
            config.lez_amount
        )));
    }

    let cancel = Arc::new(AtomicBool::new(false));

    // Ctrl-C (SIGINT) or SIGTERM → graceful stop between/within iterations.
    // Handling SIGTERM matters for `systemctl stop` / container shutdown:
    // otherwise the Node offer-publisher sidecar is orphaned and keeps
    // advertising an offline maker (P2-1).
    {
        let cancel = cancel.clone();
        tokio::spawn(async move {
            wait_for_shutdown_signal().await;
            eprintln!("shutdown signal received — stopping after current wait...");
            cancel.store(true, Ordering::Relaxed);
        });
    }

    // Heartbeat offer publisher (best-effort sidecar).
    let publisher_handle = if args.heartbeat_secs == 0 {
        warn!(
            "offer heartbeat disabled (--heartbeat-secs 0) — subscribers will not see this offer"
        );
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
                let mut env = bot::OfferPublisherEnv::from_config(config, lez_minutes, eth_minutes);
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

    // Progress drain: prints events. Journaling is NOT done here — the swap flow
    // records each in-flight swap durably (fsync'd) BEFORE locking LEZ and clears
    // it only on a confirmed terminal state, via the SwapJournal handle passed
    // into run_maker_loop (P1-4/P1-5). A print-only drain cannot race the lock.
    let (tx, mut rx) = mpsc::unbounded_channel::<SwapProgress>();
    let drain = tokio::spawn(async move {
        while let Some(event) = rx.recv().await {
            if json {
                if let Ok(line) = serde_json::to_string(&event) {
                    println!("{line}");
                }
            } else {
                println!("[maker-loop] {}", describe(&event));
            }
        }
    });

    let auto_config = AutoAcceptConfig {
        lez_timelock_minutes: lez_minutes,
        eth_timelock_minutes: eth_minutes,
    };
    let timelock_margin_secs = args.timelock_margin_minutes * 60;
    let result = run_maker_loop(
        config,
        &auto_config,
        &cancel,
        Some(tx),
        store.as_ref(),
        timelock_margin_secs,
    )
    .await;

    // tx was moved into the loop and dropped on return — drain ends naturally.
    let _ = drain.await;
    cancel.store(true, Ordering::Relaxed);
    if let Some(handle) = publisher_handle {
        handle.abort(); // kill_on_drop reaps the node child
    }

    // (Resumed in-flight swaps were already drained before the loop started, so
    // there are no background recovery tasks left to await here — P1-A.)

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

/// Resolve when the process receives a graceful-shutdown signal. On Unix this
/// is either SIGINT (Ctrl-C) or SIGTERM (`systemctl stop`, `docker stop`,
/// orchestrator termination); elsewhere it falls back to Ctrl-C only. Handling
/// SIGTERM ensures the Node offer-publisher sidecar is reaped instead of being
/// orphaned to keep advertising an offline maker (P2-1).
async fn wait_for_shutdown_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        let mut sigterm = match signal(SignalKind::terminate()) {
            Ok(s) => s,
            Err(e) => {
                warn!("failed to install SIGTERM handler ({e}); Ctrl-C only");
                let _ = tokio::signal::ctrl_c().await;
                return;
            }
        };
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {}
            _ = sigterm.recv() => {}
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}

/// Human-readable one-liner for a progress event.
fn describe(event: &SwapProgress) -> String {
    match event {
        SwapProgress::WaitingForEthLock => "waiting for taker to lock ETH...".into(),
        SwapProgress::EthLockDetected {
            swap_id,
            hashlock,
            taker_lez_account,
            tx_hash,
            chain_id,
        } => {
            format!(
                "ETH lock detected (swap {swap_id}, hashlock {hashlock}, tx {tx_hash}, chain \
                 {chain_id}); LEZ escrow will be claimable only by taker account \
                 {taker_lez_account}"
            )
        }
        SwapProgress::EthLockRejectedTakerAccount {
            swap_id,
            taker_lez_account,
            reason,
        } => format!(
            "ETH lock rejected (swap {swap_id}): takerLezAccount {taker_lez_account} — {reason}; \
             still waiting"
        ),
        SwapProgress::EthLockRejected {
            swap_id,
            eth_expiry_secs,
            required_expiry_secs,
        } => format!(
            "ETH lock rejected (swap {swap_id}): expiry {eth_expiry_secs} < required \
             {required_expiry_secs} (fresh LEZ expiry + margin); still waiting — the taker \
             must re-lock with a longer ETH timelock"
        ),
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

#[cfg(test)]
mod tests {
    use super::parse_boolish;
    use clap::Parser;

    // P2-3: the docs advertise RESTRICT_COUNTERPARTY=1, but clap's default bool
    // parser only accepts true/false. The boolish parser accepts 1/0, true/false,
    // yes/no, on/off (case-insensitive) and rejects anything else.
    #[test]
    fn boolish_parses_env_style_values() {
        for truthy in ["1", "true", "TRUE", "yes", "Y", "on", " true "] {
            assert_eq!(parse_boolish(truthy), Ok(true), "{truthy:?} should be true");
        }
        for falsy in ["0", "false", "FALSE", "no", "N", "off"] {
            assert_eq!(parse_boolish(falsy), Ok(false), "{falsy:?} should be false");
        }
        for bad in ["", "2", "maybe", "enable"] {
            assert!(parse_boolish(bad).is_err(), "{bad:?} should be rejected");
        }
    }

    // The clap wiring: `RESTRICT_COUNTERPARTY=1` parses to `true`, a bare
    // `--restrict-counterparty` flag (no value) is `true`, and the default is
    // `false`. Uses a tiny throwaway parser mirroring the real arg attributes so
    // the test does not depend on the full MakerArgs surface (env/other fields).
    #[derive(Parser)]
    struct BoolishProbe {
        #[arg(
            long,
            value_parser = parse_boolish,
            num_args = 0..=1,
            default_value_t = false,
            default_missing_value = "true",
        )]
        flag: bool,
    }

    #[test]
    fn boolish_clap_flag_and_value_forms() {
        assert!(!BoolishProbe::parse_from(["x"]).flag, "default is false");
        assert!(
            BoolishProbe::parse_from(["x", "--flag"]).flag,
            "bare flag means true"
        );
        assert!(
            BoolishProbe::parse_from(["x", "--flag", "1"]).flag,
            "=1 parses to true"
        );
        assert!(
            !BoolishProbe::parse_from(["x", "--flag", "0"]).flag,
            "=0 parses to false"
        );
    }
}
