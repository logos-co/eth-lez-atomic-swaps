use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use clap::Args;
use tokio::sync::mpsc;
use tracing::{error, warn};

use crate::config::{SwapConfig, account_id_to_base58};
use crate::error::{Result, SwapError};
use crate::lez::client::LezClient;
use crate::ops::OpsLedger;
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

    /// Fallback (reliable-baseline) republish interval in seconds. The
    /// offer-publisher uses `FALLBACK_HEARTBEAT_SECS || OFFER_HEARTBEAT_SECS ||
    /// 30` as its actual cadence (RFQ is the fast path; this is the slow
    /// baseline). The health check must threshold on that SAME effective
    /// interval — otherwise a 30s fallback cadence gets flagged unhealthy by a
    /// smaller OFFER_HEARTBEAT_SECS (the exact regression the RFQ rollout hit).
    #[arg(long, env = "FALLBACK_HEARTBEAT_SECS")]
    fallback_heartbeat_secs: Option<u64>,

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

    /// Faucet sidecar: natively claim from the pinata faucet (150 LEZ each,
    /// no `wallet` binary) until the maker LEZ balance reaches this target.
    /// Standalone (exits when reached) unless combined with --loop, where it
    /// tops up before the loop starts.
    #[arg(long, env = "FUND_TO_TARGET", value_name = "TARGET")]
    fund_to: Option<u128>,

    /// Read `--status-file` and assert on it, then exit 0 (healthy) or
    /// non-zero with a reason (unhealthy) — this is the docker healthcheck
    /// entry point (issue #93). Deliberately NOT a liveness proxy: a process
    /// that is merely alive but whose last confirmed offer publish is stale,
    /// or whose offer-publisher sidecar is dead, is reported unhealthy. Takes
    /// priority over every other mode.
    #[arg(long)]
    status: bool,

    /// Where the standing loop writes its periodic health snapshot (rewritten
    /// every ~15s) and where `--status` reads it from. Defaults to
    /// `maker-status.json` next to `--state-file`.
    #[arg(long, env = "MAKER_STATUS_FILE")]
    status_file: Option<String>,

    /// Background low-water top-up interval (seconds) for the standing loop:
    /// every this many seconds, check the LEZ balance and — if below
    /// `--fund-low-water` — claim from the pinata faucet up to that mark.
    /// Runs independently of the swap-accept loop, so running out of
    /// inventory mid-campaign no longer requires an operator to notice and
    /// restart the process by hand (issue #93 point 3).
    #[arg(long, env = "FUND_CHECK_SECS", default_value_t = 300)]
    fund_check_secs: u64,

    /// Low-water mark (LEZ) that triggers the background top-up above.
    /// Defaults to 3x the offer's `--lez-amount` when unset — enough runway
    /// for a few more swaps before the maker would otherwise refuse a new one.
    #[arg(long, env = "FUND_LOW_WATER")]
    fund_low_water: Option<u128>,

    /// Durable, privacy-safe operations ledger (issue #98): one append-only
    /// record per accepted swap plus its terminal outcome. Defaults to
    /// `maker-ops.jsonl` next to `--state-file`. Written by the standing
    /// loop; read by `--ops-report`.
    #[arg(long, env = "MAKER_OPS_FILE")]
    ops_file: Option<String>,

    /// Print unique accepted/completed/refunded/failed counts from the
    /// durable ops ledger, then exit — the operator command issue #98 asks
    /// for ("how many people actually tried, and how did it go"). Read-only;
    /// does not require the loop to be running. Takes priority over every
    /// mode except `--status`.
    #[arg(long)]
    ops_report: bool,
}

pub async fn cmd_maker(
    args: MakerArgs,
    config: &SwapConfig,
    timelock_minutes: (Option<u64>, Option<u64>),
    json: bool,
) -> Result<()> {
    // Healthcheck entry point (issue #93): read-only, no network clients, no
    // config validation beyond what already happened while parsing `config`.
    // Takes priority over every other mode.
    if args.status {
        return cmd_maker_status(&args);
    }

    // Operator report entry point (issue #98): read-only, no network clients.
    if args.ops_report {
        return cmd_maker_ops_report(&args, json);
    }

    // Faucet-only sidecar mode: fund to target, then exit.
    if let Some(target) = args.fund_to
        && !args.loop_mode
    {
        let balance = bot::fund_to_target(config, target, json).await?;
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

/// `swap-cli maker --status`: read `--status-file` and assert on it. This is
/// the docker healthcheck's `test` command (see `deploy/docker-compose.yml`).
/// Exits 0 (via `Ok`) when healthy, non-zero (via `Err`, printed by `main`)
/// with an actionable reason otherwise. See issue #93 for why this exists —
/// the healthcheck it replaces asserted against a status file that was never
/// written, so it was permanently red regardless of whether the bot was
/// actually working.
fn cmd_maker_status(args: &MakerArgs) -> Result<()> {
    let path = args
        .status_file
        .clone()
        .map(PathBuf::from)
        .unwrap_or_else(|| bot::default_status_file(&args.state_file));

    let status = bot::read_status_file(&path).map_err(|e| {
        SwapError::InvalidConfig(format!(
            "unhealthy: {e} (no status file yet — the loop hasn't completed its first ~15s \
             write cycle, or the writer is dead)"
        ))
    })?;

    let now_ms = crate::swap::refund::now_unix() * 1000;
    let now_secs = now_ms / 1000;

    // The writer itself must be alive: a wedged/crashed process leaves a
    // status file that simply stops updating, independent of anything it last
    // recorded.
    let writer_age_secs = now_secs.saturating_sub(status.updated_at);
    let max_writer_age_secs = 3 * bot::STATUS_WRITE_INTERVAL_SECS;
    if writer_age_secs > max_writer_age_secs {
        return Err(SwapError::InvalidConfig(format!(
            "unhealthy: status file last updated {writer_age_secs}s ago (max {max_writer_age_secs}s) \
             — the maker loop is likely wedged or dead"
        )));
    }

    // Offer-publish freshness — only meaningful when publishing is enabled
    // (`--heartbeat-secs 0` disables it deliberately, e.g. for a restricted
    // test deployment with no board presence). Threshold on the SAME effective
    // interval the publisher uses (fallback preferred over the legacy heartbeat,
    // mirroring FALLBACK_HEARTBEAT_SECS || OFFER_HEARTBEAT_SECS), so a healthy
    // 30s fallback cadence is not flagged by a smaller OFFER_HEARTBEAT_SECS.
    let effective_heartbeat_secs = args
        .fallback_heartbeat_secs
        .unwrap_or(args.heartbeat_secs);
    if effective_heartbeat_secs > 0 {
        if !status.publisher_alive {
            return Err(SwapError::InvalidConfig(
                "unhealthy: offer-publisher sidecar is not alive — offers are not being \
                 broadcast to the fleet"
                    .into(),
            ));
        }
        let max_publish_age_ms = 3 * effective_heartbeat_secs * 1000;
        match status.last_offer_publish_ms {
            Some(last) => {
                let age_ms = now_ms.saturating_sub(last);
                if age_ms > max_publish_age_ms {
                    return Err(SwapError::InvalidConfig(format!(
                        "unhealthy: last confirmed offer publish was {}s ago (max {}s = 3x the \
                         {}s offer-publish interval) — the fleet connection is likely wedged",
                        age_ms / 1000,
                        max_publish_age_ms / 1000,
                        effective_heartbeat_secs
                    )));
                }
            }
            None => {
                let since_start_ms = now_ms.saturating_sub(status.started_at * 1000);
                if since_start_ms > max_publish_age_ms {
                    return Err(SwapError::InvalidConfig(format!(
                        "unhealthy: no confirmed offer publish {}s after startup (max {}s)",
                        since_start_ms / 1000,
                        max_publish_age_ms / 1000
                    )));
                }
            }
        }
    }

    if matches!(status.loop_state.as_str(), "stopped" | "cancelled") {
        return Err(SwapError::InvalidConfig(format!(
            "unhealthy: maker loop state is '{}' — the standing loop has exited",
            status.loop_state
        )));
    }

    let summary = serde_json::json!({
        "healthy": true,
        "loop_state": status.loop_state,
        "completed": status.completed,
        "failed": status.failed,
        "transient_errors": status.transient_errors,
        "in_flight": status.in_flight,
        "quarantined": status.quarantined,
        "lez_balance": status.lez_balance,
        "eth_balance_wei": status.eth_balance_wei,
        "last_offer_publish_ms": status.last_offer_publish_ms,
        "publisher_alive": status.publisher_alive,
    });
    println!("{summary}");
    Ok(())
}

/// Resolve the effective ops-ledger path: `--ops-file`/`MAKER_OPS_FILE` if
/// set, else the sibling of `--state-file` (mirrors `default_status_file`).
fn resolve_ops_file(args: &MakerArgs) -> PathBuf {
    args.ops_file
        .clone()
        .map(PathBuf::from)
        .unwrap_or_else(|| crate::ops::default_ledger_file(Path::new(&args.state_file)))
}

/// `swap-cli maker --ops-report`: the operator command issue #98 asks for —
/// unique accepted/completed/refunded/failed counts from the durable ops
/// ledger, read-only, no network. See `docs/counting-the-campaign.md` for
/// what each number does and does not mean.
fn cmd_maker_ops_report(args: &MakerArgs, json: bool) -> Result<()> {
    let path = resolve_ops_file(args);
    let ledger = OpsLedger::load(&path)?;
    let report = ledger.report();

    if json {
        println!("{}", serde_json::to_string(&report).map_err(|e| {
            SwapError::InvalidConfig(format!("failed to serialize ops report: {e}"))
        })?);
    } else {
        println!("Ops ledger: {}", path.display());
        println!("  accepted (distinct swaps ever reserved): {}", report.accepted_total);
        println!("  completed:                               {}", report.completed);
        println!("  refunded:                                 {}", report.refunded);
        println!("  failed:                                   {}", report.failed);
        for (code, count) in &report.failed_by_code {
            println!("    - {code:?}: {count}");
        }
        println!("  in-flight (accepted, no terminal state yet): {}", report.in_flight);
    }
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
        bot::fund_to_target(config, target, json).await?;
    }

    // Crash recovery FIRST (P1-3): reconcile journaled in-flight swaps before
    // the free-inventory guard, so an escrow that a previous crash left locked
    // is refunded/claimed (restoring balance) instead of wedging startup into a
    // restart-forever loop that never recovers the funds.
    let store = Arc::new(bot::StateStore::load(&args.state_file)?);
    // issue #98: durable, privacy-safe ops ledger — loaded alongside the
    // fund-safety journal so a restart's reconciliation can record the
    // terminal outcome of a swap that was in-flight when the process died
    // (see `docs/counting-the-campaign.md`).
    let ops_ledger = Arc::new(OpsLedger::load(resolve_ops_file(&args))?);
    let resume_handles = bot::reconcile(config, &store, json, &ops_ledger).await;

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
    // Bounded-retried on the READ itself (issue #93 point 2): a transient
    // sequencer error (e.g. a request timeout) is not the same thing as
    // "reachable and genuinely under-funded" — only the latter is a hard
    // startup failure below. Without this retry a single sequencer blip
    // crash-looped the whole process under `restart: unless-stopped`.
    let balance = lez_client
        .get_balance_with_retry(&maker_account, |_| {})
        .await?;
    if balance < config.lez_amount {
        return Err(SwapError::InvalidConfig(format!(
            "insufficient LEZ inventory: balance {balance} < offer amount {}; \
             top up with `swap-cli maker --fund-to <target>`",
            config.lez_amount
        )));
    }

    let cancel = Arc::new(AtomicBool::new(false));

    // Status file (issue #93 point 1): resolved once, written every ~15s by
    // `spawn_status_writer`, read and asserted on by `swap-cli maker --status`
    // (the docker healthcheck).
    let status_file_path = args
        .status_file
        .clone()
        .map(PathBuf::from)
        .unwrap_or_else(|| bot::default_status_file(&args.state_file));
    let status = Arc::new(bot::StatusState::new(crate::swap::refund::now_unix()));

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
                    status.clone(),
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

    // Periodic status-file writer (issue #93 point 1) and background LEZ
    // low-water top-up (issue #93 point 3) — both independent of the
    // swap-accept loop below, so a wedge in one never masks the other.
    let fund_low_water = args
        .fund_low_water
        .unwrap_or_else(|| config.lez_amount.saturating_mul(3));
    let status_writer_handle = bot::spawn_status_writer(
        config.clone(),
        status.clone(),
        store.clone(),
        status_file_path.clone(),
        maker_account,
        cancel.clone(),
    );
    let fund_topper_handle = bot::spawn_fund_topper(
        config.clone(),
        args.fund_check_secs,
        fund_low_water,
        cancel.clone(),
        status.clone(),
    );

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
    let drain = {
        let status = status.clone();
        tokio::spawn(async move {
            while let Some(event) = rx.recv().await {
                note_status_from_progress(&status, &event);
                if json {
                    if let Ok(line) = serde_json::to_string(&event) {
                        println!("{line}");
                    }
                } else {
                    println!("[maker-loop] {}", describe(&event));
                }
            }
        })
    };

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
        ops_ledger.as_ref(),
    )
    .await;

    // tx was moved into the loop and dropped on return — drain ends naturally.
    let _ = drain.await;
    cancel.store(true, Ordering::Relaxed);
    if let Some(handle) = publisher_handle {
        handle.abort(); // kill_on_drop reaps the node child
    }
    status_writer_handle.abort();
    fund_topper_handle.abort();
    // Write a final snapshot immediately rather than leaving the last
    // periodic write (up to STATUS_WRITE_INTERVAL_SECS stale) to imply the
    // loop is still running — `--status` treats `stopped`/`cancelled` as
    // unhealthy, and that should be visible the moment the process actually
    // stops, not up to 15s later.
    status.set_loop_state("stopped");
    let final_snapshot = status.snapshot(store.snapshot().len(), store.quarantined_snapshot().len());
    if let Err(e) = bot::write_status_file(&status_file_path, &final_snapshot) {
        warn!(
            "failed to write final status snapshot to {}: {e}",
            status_file_path.display()
        );
    }

    // (Resumed in-flight swaps were already drained before the loop started, so
    // there are no background recovery tasks left to await here — P1-A.)

    // issue #98: the durable ops-ledger totals (across ALL restarts of this
    // maker, not just this process's own uptime) versus `result`'s in-memory
    // counters (this process only) are deliberately different numbers — see
    // `docs/counting-the-campaign.md` for why both are printed.
    let ops_report = ops_ledger.report();
    if json {
        println!(
            "{}",
            serde_json::json!({
                "total_completed": result.total_completed,
                "total_failed": result.total_failed,
                "total_transient_errors": result.total_transient_errors,
                "ops_ledger": ops_report,
            })
        );
    } else {
        println!(
            "Maker loop stopped: {} completed, {} failed, {} transient (sequencer) errors \
             (this process's uptime)",
            result.total_completed, result.total_failed, result.total_transient_errors
        );
        println!(
            "Durable ops ledger (all-time, {}): accepted={} completed={} refunded={} failed={} in_flight={}",
            resolve_ops_file(&args).display(),
            ops_report.accepted_total,
            ops_report.completed,
            ops_report.refunded,
            ops_report.failed,
            ops_report.in_flight,
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

/// Feed the standing loop's progress events into the shared
/// [`bot::StatusState`] so the periodic status-file writer (and, downstream,
/// `swap-cli maker --status`) reflect what the loop is actually doing, not
/// just that the process is alive (issue #93).
fn note_status_from_progress(status: &bot::StatusState, event: &SwapProgress) {
    match event {
        SwapProgress::AutoAcceptStarted => status.set_loop_state("starting"),
        SwapProgress::AutoAcceptIteration { .. } => status.set_loop_state("waiting_for_taker"),
        SwapProgress::EthLockDetected { .. } | SwapProgress::LezLocking => {
            status.set_loop_state("locking_lez")
        }
        SwapProgress::LezLocked { .. } | SwapProgress::WaitingForPreimage => {
            status.set_loop_state("waiting_for_claim")
        }
        SwapProgress::PreimageRevealed { .. } | SwapProgress::ClaimingEth => {
            status.set_loop_state("claiming_eth")
        }
        SwapProgress::AutoAcceptSwapCompleted { .. } => {
            status.incr_completed();
            status.set_loop_state("waiting_for_taker");
        }
        SwapProgress::AutoAcceptSwapFailed { .. } => {
            status.incr_failed();
            status.set_loop_state("waiting_for_taker");
        }
        SwapProgress::AutoAcceptTransientError { .. } => {
            // Deliberately NOT incr_failed(): a retried/retried-out sequencer
            // timeout is infrastructure flakiness, not a swap failure — see
            // MakerStatus::transient_errors.
            status.incr_transient();
            status.set_loop_state("waiting_for_taker");
        }
        SwapProgress::AutoAcceptInsufficientFunds { .. } => {
            status.set_loop_state("insufficient_funds_waiting")
        }
        SwapProgress::AutoAcceptCancelled => status.set_loop_state("cancelled"),
        SwapProgress::AutoAcceptStopped { .. } => status.set_loop_state("stopped"),
        _ => {}
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
        SwapProgress::AutoAcceptTransientError { iteration, error } => {
            format!(
                "iteration {iteration}: transient sequencer error (retried, not counted as a \
                 swap failure) — {error}"
            )
        }
        SwapProgress::AutoAcceptInsufficientFunds {
            lez_balance,
            lez_required,
        } => format!(
            "out of LEZ inventory ({lez_balance} < {lez_required}) — waiting for the background \
             fund-topper (--fund-check-secs / --fund-low-water) instead of stopping"
        ),
        SwapProgress::AutoAcceptStopped {
            total_completed,
            total_failed,
            total_transient_errors,
        } => format!(
            "loop stopped ({total_completed} completed, {total_failed} failed, \
             {total_transient_errors} transient)"
        ),
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
