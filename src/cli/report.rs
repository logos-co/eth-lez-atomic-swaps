//! `swap-cli chain-report` — the read-only, on-chain count of public-trial
//! swap activity. See [`crate::eth::report`] for why the chain is the primary
//! source and `docs/counting-the-campaign.md` for what each number means.
//!
//! Read-only in the same sense as `maker --ops-report`: no state is written, no
//! transaction is sent, and nothing depends on a maker loop running. Unlike
//! `--ops-report` it also needs no operator access to any maker container —
//! anyone with a Sepolia RPC URL can reproduce the numbers.

use alloy::primitives::Address;
use alloy::providers::{Provider, ProviderBuilder};
use clap::{Args, Parser, Subcommand};
use serde::Serialize;
use tracing::warn;

use crate::error::{Result, SwapError};
use crate::eth::client::verify_interface_version;
use crate::eth::report::{
    self, DEFAULT_CHUNK_BLOCKS, PINNED_DEPLOY_BLOCK, PINNED_ETH_HTLC_ADDRESS, ReportWindow,
    SUPERSEDED_ETH_HTLC_ADDRESS, SwapCounts,
};

/// The subcommand token. `run()` matches on it to short-circuit the global
/// [`crate::cli::ConfigArgs`] — see [`run_standalone`].
pub const COMMAND_NAME: &str = "chain-report";

/// How many times the whole scan is retried on a **fresh connection** when the
/// endpoint could not corroborate it.
///
/// A fresh [`connect`] builds a new provider with its own reqwest connection
/// pool, so its requests open a new TCP connection rather than reusing the
/// keep-alive one the previous attempt was pinned to. A pool that load-balances
/// per connection therefore routes the retry to a different backend — which is
/// the only thing that can get past one whose log index has been pruned, since
/// retrying over the established connection just asks the same node again.
const MAX_SCAN_ATTEMPTS: usize = 5;

#[derive(Args, Debug)]
pub struct ChainReportArgs {
    /// Ethereum RPC URL — the same `ETH_RPC_URL` the maker and taker already
    /// use (`wss://` or `https://`). No new endpoint, no new provider.
    #[arg(long, env = "ETH_RPC_URL", hide_env_values = true)]
    eth_rpc_url: String,

    /// EthHTLC venue to measure. Defaults to the pinned Sepolia deployment;
    /// whichever address is used is named in the output.
    #[arg(long, env = "ETH_HTLC_ADDRESS", default_value = PINNED_ETH_HTLC_ADDRESS)]
    eth_htlc_address: String,

    /// First block to count locks from. Defaults to the pinned venue's
    /// deployment block when measuring the pinned venue — i.e. its whole
    /// history — and is required for any other address.
    #[arg(long, conflicts_with = "since")]
    from_block: Option<u64>,

    /// Last block to count locks in (default: the chain head).
    #[arg(long, conflicts_with = "until")]
    to_block: Option<u64>,

    /// Count only locks at or after this UTC time — a Unix timestamp,
    /// `YYYY-MM-DD`, or `YYYY-MM-DDTHH:MM:SS`. Resolved to a block number,
    /// which the report prints.
    #[arg(long, value_parser = report::parse_time_arg)]
    since: Option<u64>,

    /// Count only locks at or before this UTC time (same formats as --since).
    #[arg(long, value_parser = report::parse_time_arg)]
    until: Option<u64>,

    /// Blocks per `eth_getLogs` request. Halved automatically on a range
    /// rejection; raise it only if your endpoint is known to allow more.
    #[arg(long, default_value_t = DEFAULT_CHUNK_BLOCKS)]
    chunk_blocks: u64,

    /// Publish a count whose canary probes were ANSWERED at one or both ends of
    /// the scan and reported no logs — your assertion that those blocks really
    /// are log-free. Without it such a scan is REFUSED, because an endpoint
    /// that has pruned this era's logs answers with an empty list exactly like
    /// a quiet chain does, and the number it would print is a zero
    /// indistinguishable from a quiet trial. It has nothing to bear on a walk
    /// the endpoint never answered at all — every probe errored, or was
    /// throttled, or any mix — because nothing about the chain was observed
    /// there; that is refused either way.
    #[arg(long)]
    allow_uncorroborated: bool,
}

/// Everything a run measured, plus what it measured it against. The provenance
/// fields are not decoration: a bare count is unverifiable, and "which contract,
/// which blocks" is what lets anyone else reproduce it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ChainReport {
    /// The EthHTLC this run read.
    pub contract_address: String,
    /// Whether that is the currently pinned venue. `false` means the numbers
    /// describe some other deployment and are not the public-trial count.
    pub is_pinned_venue: bool,
    pub chain_id: u64,
    /// `INTERFACE_VERSION` as reported by the contract (the handshake that
    /// makes a wrong-era address fail loudly instead of reading zero).
    pub interface_version: u64,
    /// The attempt window: locks in these blocks were counted.
    pub from_block: u64,
    pub to_block: u64,
    /// How far `Claimed`/`Refunded` were followed. Equal to `to_block` for a
    /// default run; higher when `--to-block`/`--until` bound the attempt window,
    /// so a swap locked at the end of it is not miscounted as still-open.
    pub outcomes_to_block: u64,
    /// Whether every log response in this run was corroborated against a
    /// canary (see `eth::report::Canary`) — what rules out an under-count from
    /// a load-balanced endpoint whose backends have pruned their log index. A
    /// run that could not anchor a canary is refused outright, so `false` only
    /// ever appears when the operator passed `--allow-uncorroborated` to
    /// accept an unproven count. A window that selects no blocks queried
    /// nothing, so nothing went unproven and this stays `true`.
    pub corroborated: bool,
    #[serde(flatten)]
    pub counts: SwapCounts,
}

// ---------------------------------------------------------------------------
// Standalone parsing
// ---------------------------------------------------------------------------
//
// Every other subcommand hangs off `Cli`, which flattens `ConfigArgs` — a
// private key, LEZ signing material, amounts, timelocks and a counterparty.
// This command reads PUBLIC chain data only, so demanding any of that would be
// wrong twice over: it would stop the operator running a report from a machine
// with no keys on it, and it would stop an outside observer verifying the trial
// numbers at all, which is the whole point of preferring the chain to the ops
// ledger. So `chain-report` is short-circuited before `Cli::parse()` with its
// own parser — the same shape the `demo`/`infra` subcommands already use, but
// with real clap parsing rather than argv string matching, so `--help`,
// `--json` and value validation all behave normally.

#[derive(Parser)]
#[command(name = "swap-cli", about = "Atomic swap CLI (LEZ <-> ETH)")]
struct ChainReportCli {
    #[command(subcommand)]
    command: ChainReportCommand,

    /// Output as JSON (for scripting)
    #[arg(long, global = true)]
    json: bool,

    /// Override .env file path (default: .env)
    #[arg(long, global = true)]
    env_file: Option<String>,
}

#[derive(Subcommand)]
enum ChainReportCommand {
    /// Read-only, on-chain report of public-trial swap activity
    ChainReport(ChainReportArgs),
}

/// Entry point for the short-circuit in [`crate::cli::run`].
///
/// Installs its own subscriber writing to **stderr**, unlike the stdout default
/// every other subcommand gets. This command's stdout carries the report — and
/// under `--json`, nothing but the report: the reconnect notice below fires on
/// exactly the runs a script is most likely to be piping through `jq`, and a
/// WARN line ahead of the JSON object would break it.
///
/// `RUST_LOG` still applies, defaulting to INFO. The `debug!` trail in
/// [`crate::eth::report`] — which end missed, what each canary scored, why a
/// chunk was retried — exists to diagnose the load-balanced-endpoint flakiness
/// this whole command is built around, and is worthless if it cannot be turned
/// on.
pub async fn run_standalone() -> Result<()> {
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info".into()),
        )
        .try_init()
        .ok();
    let cli = ChainReportCli::parse();
    let ChainReportCommand::ChainReport(args) = cli.command;
    cmd_chain_report(args, cli.json).await
}

// ---------------------------------------------------------------------------
// The command
// ---------------------------------------------------------------------------

/// Whether an opening-handshake failure is worth redrawing a backend for.
///
/// Only a transport fault is. A version skew ([`SwapError::EthAbiMismatch`]) is
/// a verdict the endpoint already delivered — the deployment will still be the
/// wrong era on the next connection, and retrying it five times would blunt the
/// loud, immediate startup refusal `eth::client` is built around. Same for a
/// config error: nothing about it changes on a reconnect.
fn is_retryable_handshake_failure(err: &SwapError) -> bool {
    matches!(err, SwapError::EthRpc(_))
}

/// How long to wait before retrying `err`, for every request this command
/// makes outside `eth::report`'s own scan loop.
///
/// A 429 is the endpoint asking for FEWER requests (see
/// [`report::RpcFailure`]), so retrying immediately fires the next request
/// within milliseconds — the same burst shape that provoked the throttle. Every
/// other transport fault is answered by drawing a different backend, and
/// waiting buys nothing, so this returns zero and the caller decides.
///
/// A zero duration therefore reads as "not a throttle". Both callers rely on
/// that, which is what keeps this the single rate-limit policy on the path
/// rather than one per call site. The schedule is the one `scan_range` already
/// applies to a throttled chunk.
fn throttle_backoff(err: &SwapError, attempt: usize) -> std::time::Duration {
    match err {
        SwapError::EthRpc(msg)
            if report::classify_rpc_failure(msg) == report::RpcFailure::RateLimited =>
        {
            report::RATE_LIMIT_BACKOFF * 2u32.pow((attempt as u32).saturating_sub(1).min(6))
        }
        _ => std::time::Duration::ZERO,
    }
}

/// Fetch one block's timestamp, waiting out a throttle rather than dying on it.
///
/// The `--since`/`--until` binary searches issue up to ~17 serial
/// `eth_getBlockByNumber` calls per bound over the pinned venue's history —
/// exactly the burst the endpoint answers with a 429. Retrying the SAME block
/// is right here: unlike a scan, there is no other backend to prefer and
/// nothing to re-anchor, only a request the endpoint asked us to slow down.
/// Anything that is not a throttle propagates on the first failure.
///
/// Generic over the fetch for the same reason the searches above it are: a
/// throttled endpoint is the one thing a unit test cannot get from a real one.
async fn block_timestamp_paced<F, Fut>(block: u64, fetch: F) -> Result<u64>
where
    F: Fn() -> Fut,
    Fut: std::future::Future<Output = Result<u64>>,
{
    let mut attempt = 1usize;
    loop {
        let err = match fetch().await {
            Ok(timestamp) => return Ok(timestamp),
            Err(e) => e,
        };
        let backoff = throttle_backoff(&err, attempt);
        if backoff.is_zero() || attempt as u32 > report::MAX_RATE_LIMIT_RETRIES {
            return Err(err);
        }
        warn!(
            block,
            attempt,
            ?backoff,
            %err,
            "chain-report: endpoint throttled the window search, backing off before retrying"
        );
        tokio::time::sleep(backoff).await;
        attempt += 1;
    }
}

/// The three RPCs a run needs before it can even name its window: the era
/// handshake, the chain id, and the head.
///
/// The handshake is the same one every client runs, and for the same reason: a
/// wrong-era contract does not error on a log query, it matches nothing. A
/// report that silently prints zeroes is worse than one that refuses to run.
async fn opening_handshake(
    provider: &alloy::providers::DynProvider,
    address: Address,
) -> Result<(u64, u64, u64)> {
    let interface_version = verify_interface_version(provider, address).await?;
    let chain_id = provider
        .get_chain_id()
        .await
        .map_err(|e| SwapError::EthRpc(format!("eth_chainId failed: {e}")))?;
    let head = provider
        .get_block_number()
        .await
        .map_err(|e| SwapError::EthRpc(format!("eth_blockNumber failed: {e}")))?;
    Ok((interface_version, chain_id, head))
}

/// The oldest block a window may start at, for the venue actually being
/// measured.
///
/// [`PINNED_DEPLOY_BLOCK`] is a fact about ONE deployment — earlier blocks
/// cannot hold its events, which is what makes it a safe floor and a cheap
/// one. It says nothing about any other address. Applying it to a non-pinned
/// venue is the same era inheritance the guard in [`cmd_chain_report`]
/// refuses, and through `--since` it does not even fail loudly: it silently
/// truncates that deployment's history, down to an empty window on a localnet
/// whose head is below the pinned venue's deployment block.
fn search_floor(is_pinned_venue: bool, from_block: Option<u64>) -> u64 {
    match from_block {
        Some(explicit) => explicit,
        None if is_pinned_venue => PINNED_DEPLOY_BLOCK,
        None => 0,
    }
}

pub async fn cmd_chain_report(args: ChainReportArgs, json: bool) -> Result<()> {
    let address: Address = args
        .eth_htlc_address
        .parse()
        .map_err(|e| SwapError::InvalidConfig(format!("invalid eth-htlc-address: {e}")))?;
    let pinned: Address = PINNED_ETH_HTLC_ADDRESS
        .parse()
        .expect("pinned venue address is a compile-time constant");
    let is_pinned_venue = address == pinned;

    if args.chunk_blocks == 0 {
        return Err(SwapError::InvalidConfig(
            "--chunk-blocks must be at least 1".into(),
        ));
    }

    // A non-pinned address cannot inherit the pinned venue's deployment block —
    // that is precisely how two deployment eras get silently mixed into one
    // number. Make the operator state the era they mean.
    if !is_pinned_venue && args.from_block.is_none() && args.since.is_none() {
        let superseded = SUPERSEDED_ETH_HTLC_ADDRESS
            .parse::<Address>()
            .ok()
            .is_some_and(|a| a == address);
        let hint = if superseded {
            format!(
                " That address is the SUPERSEDED v1 deployment recorded in docs/testnet.md; its \
                 Locked topic0 differs from the pinned venue's, so its swaps are a different era \
                 and must never be summed with {PINNED_ETH_HTLC_ADDRESS}'s."
            )
        } else {
            String::new()
        };
        return Err(SwapError::InvalidConfig(format!(
            "--eth-htlc-address {address} is not the pinned venue ({PINNED_ETH_HTLC_ADDRESS}), so \
             the pinned deployment block ({PINNED_DEPLOY_BLOCK}) does not apply to it. Pass an \
             explicit --from-block (or --since) saying where that deployment's history starts.{hint}"
        )));
    }

    // The opening RPCs get the same fresh-connection retry the scan does. They
    // run against the same load-balanced endpoint, and a 429 on the very first
    // eth_call is documented expected behaviour there — dying on it while an
    // uncorroborated scan gets five backends would be arbitrary.
    let mut attempt = 1usize;
    let (provider, interface_version, chain_id, head) = loop {
        let provider = connect(&args.eth_rpc_url).await?;
        match opening_handshake(&provider, address).await {
            Ok((version, chain_id, head)) => break (provider, version, chain_id, head),
            Err(e) if !is_retryable_handshake_failure(&e) => return Err(e),
            Err(e) if attempt < MAX_SCAN_ATTEMPTS => {
                let backoff = throttle_backoff(&e, attempt);
                let doing = if backoff.is_zero() {
                    "chain-report: reconnecting to reach a different RPC backend"
                } else {
                    "chain-report: endpoint throttled the opening handshake, backing off before \
                     retrying"
                };
                warn!(attempt, ?backoff, %e, "{doing}");
                tokio::time::sleep(backoff).await;
                attempt += 1;
            }
            Err(SwapError::EthRpc(msg)) => {
                return Err(SwapError::EthRpc(exhausted_message(&msg)));
            }
            Err(e) => return Err(e),
        }
    };

    let rpc = &provider;
    let default_from = search_floor(is_pinned_venue, args.from_block);
    let from_block = match args.since {
        Some(ts) => {
            match report::first_block_at_or_after(default_from, head, ts, |b| {
                block_timestamp_paced(b, move || report::block_timestamp(rpc, b))
            })
            .await?
            {
                Some(b) => b,
                // Nothing on chain is that new yet: an empty window, not an error.
                None => head.saturating_add(1),
            }
        }
        None => default_from,
    };

    let to_block = match args.until {
        Some(ts) => {
            match report::last_block_at_or_before(from_block.min(head), head, ts, |b| {
                block_timestamp_paced(b, move || report::block_timestamp(rpc, b))
            })
            .await?
            {
                Some(b) => b,
                // Everything on chain is newer than --until: an empty window.
                None => from_block.saturating_sub(1),
            }
        }
        None => args.to_block.unwrap_or(head).min(head),
    };

    // Terminal events are followed to the head even when the attempt window
    // ends earlier: a lock in the last block of the window that was claimed one
    // block later is completed, not still-open. An empty window is the one
    // exception — there are no locks to follow outcomes for, so scanning to the
    // head would be a long query with nothing to learn from it.
    let window = ReportWindow {
        locks_from: from_block,
        locks_to: to_block,
        outcomes_to: if to_block < from_block {
            to_block
        } else {
            head.max(to_block)
        },
    };

    // A public endpoint load-balances across nodes with different log
    // retention, and a connection tends to stay on the node it was routed to:
    // once it lands on one whose index does not reach back to `locks_from`,
    // retrying on that connection cannot help. A fresh connection is a fresh
    // draw from the pool, so an uncorroborated scan is retried on a new one
    // rather than published or abandoned.
    let mut scan = None;
    let mut last_uncorroborated = String::new();
    for attempt in 1..=MAX_SCAN_ATTEMPTS {
        let provider = if attempt == 1 {
            provider.clone()
        } else {
            connect(&args.eth_rpc_url).await?
        };
        match report::collect_tally(
            &provider,
            address,
            &window,
            args.chunk_blocks,
            args.allow_uncorroborated,
        )
        .await
        {
            Ok(s) => {
                scan = Some(s);
                break;
            }
            Err(SwapError::EthLogsUncorroborated(msg)) => {
                warn!(
                    attempt,
                    %msg,
                    "chain-report: reconnecting to reach a different RPC backend"
                );
                last_uncorroborated = msg;
            }
            Err(e) => return Err(e),
        }
    }
    let Some(scan) = scan else {
        return Err(SwapError::EthLogsUncorroborated(exhausted_message(
            &last_uncorroborated,
        )));
    };

    let report = ChainReport {
        contract_address: address.to_string(),
        is_pinned_venue,
        chain_id,
        interface_version,
        from_block: window.locks_from,
        to_block: window.locks_to,
        outcomes_to_block: window.outcomes_to,
        corroborated: scan.corroborated,
        counts: scan.tally.counts(),
    };

    print_report(&report, json)?;
    Ok(())
}

/// What terminates a run whose every reconnect ended the same way: the
/// refusal that was reached, plus how many times it was reached.
///
/// Deliberately says nothing about WHY. The remedy differs by what the probes
/// actually observed — a throttled endpoint and one that never answered want
/// opposite advice — and `eth::report::refusal_message` has already branched on
/// that. Appending a fixed diagnosis here would contradict it in the same
/// breath, on the one sentence that ends every failed run.
fn exhausted_message(refusal: &str) -> String {
    format!(
        "{refusal} This was retried {MAX_SCAN_ATTEMPTS} times, on a fresh connection each time so \
         that a load-balanced endpoint had a chance to route it to a different backend, and every \
         attempt ended the same way. No count is published."
    )
}

/// Rewrite a `wss://`/`ws://` endpoint to `https://`/`http://`, same host.
/// Anything else is passed through unchanged, for [`connect`] to accept or
/// refuse. Schemes are matched case-insensitively: URL schemes are
/// case-insensitive by spec, and alloy lowercases before dispatching on them,
/// so a `WSS://` this rewrite skipped would still reach the WebSocket
/// transport. See [`connect`] for why that matters.
fn to_http_url(url: &str) -> String {
    match url.split_once("://") {
        Some((scheme, rest)) if scheme.eq_ignore_ascii_case("wss") => format!("https://{rest}"),
        Some((scheme, rest)) if scheme.eq_ignore_ascii_case("ws") => format!("http://{rest}"),
        _ => url.to_string(),
    }
}

/// The lowercased scheme of `url`, or `None` when it carries none.
fn url_scheme(url: &str) -> Option<String> {
    url.split_once("://")
        .map(|(scheme, _)| scheme.to_ascii_lowercase())
}

/// Transports whose JSON-RPC batch reaches one backend as one request.
const BATCHING_SCHEMES: [&str; 2] = ["http", "https"];

/// Connect read-only over HTTP, reusing the configured `ETH_RPC_URL` — the same
/// host, switched from `wss://` to `https://`.
///
/// This is not a preference, it is a correctness requirement. The report proves
/// each answer by batching it with canary queries (see `eth::report::Canary`),
/// and that proof only holds if the batch reaches ONE backend. Alloy's
/// WebSocket transport does not send a JSON-RPC batch at all — it splits the
/// packet and sends each request separately (`alloy-pubsub`'s frontend maps a
/// `RequestPacket::Batch` over individual sends) — so over `wss://` the canary
/// and the query it vouches for can be answered by different nodes, and a
/// pruned node's empty answer sails through. Over HTTP the batch is one
/// request and one response, which is what makes the corroboration mean
/// anything.
///
/// Nothing is lost by it: this command only ever issues log/block queries, and
/// needs no subscription. `src/eth/watcher.rs` already polls `eth_getLogs`
/// rather than subscribing, for its own reasons.
async fn connect(url: &str) -> Result<alloy::providers::DynProvider> {
    let http = to_http_url(url);
    let scheme = url_scheme(&http);
    if !scheme
        .as_deref()
        .is_some_and(|s| BATCHING_SCHEMES.contains(&s))
    {
        let got = match &scheme {
            Some(s) => format!("scheme '{s}'"),
            None => "no scheme".to_string(),
        };
        return Err(SwapError::InvalidConfig(format!(
            "--eth-rpc-url must be an http:// or https:// endpoint, but this one has {got}. Every \
             count this report publishes is proven by batching each eth_getLogs with canary \
             queries, and that proof only holds when the JSON-RPC batch reaches ONE backend as \
             ONE request — which only the HTTP transport does. A wss:// or ws:// URL is rewritten \
             to the same host over https:// or http:// automatically; any other transport would \
             let the canary and the query it vouches for be answered by different nodes, so it is \
             refused rather than counted as proven."
        )));
    }
    Ok(ProviderBuilder::new()
        .connect(&http)
        .await
        .map_err(|e| SwapError::EthRpc(format!("RPC connect failed: {e}")))?
        .erased())
}

fn print_report(report: &ChainReport, json: bool) -> Result<()> {
    if json {
        println!(
            "{}",
            serde_json::to_string(report).map_err(|e| SwapError::InvalidConfig(format!(
                "failed to serialize chain report: {e}"
            )))?
        );
        return Ok(());
    }

    let venue = if report.is_pinned_venue {
        "pinned public-trial venue"
    } else {
        "NOT the pinned venue — these numbers are not the public-trial count"
    };
    println!(
        "On-chain swap report — EthHTLC {} ({venue})",
        report.contract_address
    );
    println!(
        "  chain id {}, INTERFACE_VERSION {}",
        report.chain_id, report.interface_version
    );
    let empty_window = report.to_block < report.from_block;
    if empty_window {
        // A window that selects no blocks — e.g. `--since` in the future.
        // Nothing was queried, so there is nothing to say about outcomes or
        // corroboration either.
        println!("  locks counted:    (empty window — no blocks selected, nothing queried)");
    } else {
        println!(
            "  locks counted:    blocks {}–{} (inclusive)",
            report.from_block, report.to_block
        );
        println!("  outcomes through: block {}", report.outcomes_to_block);
        if !report.corroborated {
            println!(
                "  NOTE: --allow-uncorroborated was passed and this scan could not be anchored \
                 at both ends of blocks {}-{}, so these counts are UNPROVEN: an empty log \
                 response could not be told apart from an RPC backend that does not hold these \
                 blocks' logs.",
                report.from_block, report.outcomes_to_block
            );
        }
    }
    println!();

    let c = &report.counts;
    println!("  attempted (ETH locked):  {}", c.attempted);
    println!("  completed (claimed):     {}", c.completed);
    println!("  refunded:                {}", c.refunded);
    println!("  still open:              {}", c.still_open);
    println!("  distinct taker wallets:  {}", c.distinct_taker_wallets);

    println!();
    if c.by_maker.is_empty() {
        println!("  by maker: (no locks in this window)");
    } else {
        println!("  by maker (Locked.recipient):");
        for m in &c.by_maker {
            println!(
                "    {}  attempted {}  completed {}  refunded {}  open {}",
                m.recipient, m.attempted, m.completed, m.refunded, m.still_open
            );
        }
    }

    println!();
    println!("  Notes: 'attempted' counts every taker who locked ETH, including ones no maker");
    println!("  ever answered. 'distinct taker wallets' is a WALLET count, not a person count.");
    println!("  Per-maker rows do not dedupe a taker who used more than one maker, so their");
    println!("  attempt counts sum to the total but their takers cannot be added up.");
    println!(
        "  For WHY a swap failed, see `swap-cli maker --ops-report` (docs/counting-the-campaign.md)."
    );

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory as _;

    fn parse(argv: &[&str]) -> std::result::Result<ChainReportArgs, clap::Error> {
        let cli = ChainReportCli::try_parse_from(argv)?;
        let ChainReportCommand::ChainReport(args) = cli.command;
        Ok(args)
    }

    #[test]
    fn cli_definition_is_valid() {
        ChainReportCli::command().debug_assert();
    }

    // The venue default is the whole point of "only the current venue": a bare
    // run must measure the pinned contract, not whatever happens to be handy.
    #[test]
    fn address_defaults_to_the_pinned_venue() {
        // `env` on the arg means a stray ETH_HTLC_ADDRESS in the ambient
        // environment would win; assert on the declared default instead, which
        // is what an operator with no such variable gets.
        let default = ChainReportArgs::augment_args(clap::Command::new("t"))
            .get_arguments()
            .find(|a| a.get_id() == "eth_htlc_address")
            .and_then(|a| a.get_default_values().first().cloned())
            .expect("eth-htlc-address has a default");
        assert_eq!(default.to_str().unwrap(), PINNED_ETH_HTLC_ADDRESS);
    }

    // A block window and a time window are two ways to say the same thing;
    // accepting both at once would silently pick one.
    #[test]
    fn block_and_time_bounds_are_mutually_exclusive() {
        assert!(
            parse(&[
                "swap-cli",
                "chain-report",
                "--eth-rpc-url",
                "wss://example",
                "--from-block",
                "1",
                "--since",
                "2026-08-21",
            ])
            .is_err()
        );
        assert!(
            parse(&[
                "swap-cli",
                "chain-report",
                "--eth-rpc-url",
                "wss://example",
                "--to-block",
                "9",
                "--until",
                "2026-08-21",
            ])
            .is_err()
        );
    }

    #[test]
    fn time_bounds_are_parsed_at_the_cli_boundary() {
        let args = parse(&[
            "swap-cli",
            "chain-report",
            "--eth-rpc-url",
            "wss://example",
            "--since",
            "2026-08-21",
            "--until",
            "1787356800",
        ])
        .unwrap();
        assert_eq!(args.since, Some(1_787_270_400));
        assert_eq!(args.until, Some(1_787_356_800));

        assert!(
            parse(&[
                "swap-cli",
                "chain-report",
                "--eth-rpc-url",
                "wss://example",
                "--since",
                "last tuesday",
            ])
            .is_err()
        );
    }

    // `--json` is documented as a global flag on this binary; it must work on
    // either side of the subcommand, the way it does for every other command.
    #[test]
    fn json_flag_is_accepted_before_and_after_the_subcommand() {
        for argv in [
            vec![
                "swap-cli",
                "--json",
                "chain-report",
                "--eth-rpc-url",
                "wss://example",
            ],
            vec![
                "swap-cli",
                "chain-report",
                "--json",
                "--eth-rpc-url",
                "wss://example",
            ],
        ] {
            let cli = ChainReportCli::try_parse_from(argv).unwrap();
            assert!(cli.json);
        }
    }

    // The provenance fields are the difference between a number and a claim.
    #[test]
    fn json_report_names_the_contract_and_the_block_range() {
        let report = ChainReport {
            contract_address: PINNED_ETH_HTLC_ADDRESS.to_string(),
            is_pinned_venue: true,
            chain_id: 11_155_111,
            interface_version: 2,
            from_block: PINNED_DEPLOY_BLOCK,
            to_block: PINNED_DEPLOY_BLOCK + 100,
            outcomes_to_block: PINNED_DEPLOY_BLOCK + 200,
            corroborated: true,
            counts: report::SwapTally::new().counts(),
        };
        let v: serde_json::Value =
            serde_json::from_str(&serde_json::to_string(&report).unwrap()).unwrap();

        assert_eq!(v["contract_address"], PINNED_ETH_HTLC_ADDRESS);
        assert_eq!(v["is_pinned_venue"], true);
        assert_eq!(v["from_block"], PINNED_DEPLOY_BLOCK);
        assert_eq!(v["to_block"], PINNED_DEPLOY_BLOCK + 100);
        assert_eq!(v["outcomes_to_block"], PINNED_DEPLOY_BLOCK + 200);
        // Counts are flattened alongside the provenance, not nested.
        assert_eq!(v["attempted"], 0);
        assert_eq!(v["distinct_taker_wallets"], 0);
    }

    // A count no canary could prove is refused by default, so the flag that
    // downgrades that refusal must be opt-in, never implied.
    #[test]
    fn uncorroborated_counts_are_opt_in() {
        let default = parse(&["swap-cli", "chain-report", "--eth-rpc-url", "wss://example"])
            .expect("a bare run parses");
        assert!(!default.allow_uncorroborated);

        let opted_in = parse(&[
            "swap-cli",
            "chain-report",
            "--eth-rpc-url",
            "wss://example",
            "--allow-uncorroborated",
        ])
        .expect("the opt-in parses");
        assert!(opted_in.allow_uncorroborated);
    }

    // The pinned deployment block is a fact about the pinned venue only.
    // Inheriting it elsewhere silently truncates that venue's history — and on
    // a localnet, whose head is far below it, truncates it to nothing.
    #[test]
    fn only_the_pinned_venue_inherits_the_pinned_deploy_block() {
        assert_eq!(search_floor(true, None), PINNED_DEPLOY_BLOCK);
        assert_eq!(
            search_floor(false, None),
            0,
            "another deployment's history does not start where the pinned one's does"
        );
        // An explicit --from-block always wins, on either venue.
        assert_eq!(search_floor(true, Some(42)), 42);
        assert_eq!(search_floor(false, Some(42)), 42);
    }

    fn args_for(address: &str, from_block: Option<u64>, since: Option<u64>) -> ChainReportArgs {
        ChainReportArgs {
            // Unroutable on purpose: every assertion below is about a check
            // that must happen BEFORE the command opens a connection.
            eth_rpc_url: "http://127.0.0.1:1".into(),
            eth_htlc_address: address.into(),
            from_block,
            to_block: None,
            since,
            until: None,
            chunk_blocks: DEFAULT_CHUNK_BLOCKS,
            allow_uncorroborated: false,
        }
    }

    // The primary defence against summing two deployment eras: a non-pinned
    // address may not inherit the pinned venue's deployment block, so it is
    // refused until the caller says where that deployment's history starts.
    #[tokio::test]
    async fn a_non_pinned_venue_without_a_window_is_refused_before_connecting() {
        let err = cmd_chain_report(args_for(SUPERSEDED_ETH_HTLC_ADDRESS, None, None), false)
            .await
            .expect_err("the superseded v1 venue must not inherit the pinned deployment block");

        let SwapError::InvalidConfig(msg) = err else {
            panic!("expected an InvalidConfig refusal, got: {err:?}");
        };
        assert!(msg.contains(PINNED_ETH_HTLC_ADDRESS));
        assert!(msg.contains(&PINNED_DEPLOY_BLOCK.to_string()));
        assert!(
            msg.contains("SUPERSEDED"),
            "the v1 address is named for what it is: {msg}"
        );
    }

    // The same guard must not fire once the caller HAS stated the era, or
    // `--since` would be unusable on any venue but the pinned one. The command
    // then proceeds to the RPC (which fails, because there is none) — the
    // point is only that it got that far.
    #[tokio::test]
    async fn stating_the_era_gets_a_non_pinned_venue_past_the_guard() {
        for args in [
            args_for(SUPERSEDED_ETH_HTLC_ADDRESS, Some(11_316_985), None),
            args_for(SUPERSEDED_ETH_HTLC_ADDRESS, None, Some(1_787_270_400)),
        ] {
            let err = cmd_chain_report(args, false)
                .await
                .expect_err("there is no RPC at 127.0.0.1:1");
            assert!(
                !matches!(err, SwapError::InvalidConfig(_)),
                "the era guard fired even though the era was stated: {err:?}"
            );
            // Past the guard, the run dies on the interface-version handshake
            // — and nothing is listening, so that must be reported as the
            // transport failure it is. Reporting it as a verdict about the
            // deployment ("no INTERFACE_VERSION getter, redeploy EthHTLC")
            // would be a diagnosis no answered call ever made.
            let SwapError::EthRpc(msg) = err else {
                panic!("an unreachable endpoint must be an RPC error, got: {err:?}");
            };
            assert!(
                msg.contains("ETH_RPC_URL"),
                "the remedy is the endpoint, not the contract: {msg}"
            );
            assert!(
                !msg.contains("Redeploy"),
                "nothing established anything about the deployment: {msg}"
            );
            // ...and it was retried on a fresh connection each time, exactly as
            // an uncorroborated scan is, rather than dying on the first RPC.
            assert!(
                msg.contains("fresh connection") && msg.contains(&MAX_SCAN_ATTEMPTS.to_string()),
                "a transport fault on the opening handshake must redraw a backend: {msg}"
            );
        }
    }

    // The corroboration proof rests entirely on the batch reaching one backend,
    // and only the HTTP transport sends one. URL schemes are case-insensitive
    // and alloy lowercases before dispatching, so a `WSS://` this rewrite
    // skipped would still land on the WebSocket transport, where alloy-pubsub
    // splits the batch into separate sends — the canary and the query it
    // vouches for answered by different nodes.
    #[test]
    fn every_websocket_spelling_is_rewritten_to_http() {
        assert_eq!(
            to_http_url("wss://ethereum-sepolia-rpc.publicnode.com"),
            "https://ethereum-sepolia-rpc.publicnode.com"
        );
        assert_eq!(
            to_http_url("WSS://ethereum-sepolia-rpc.publicnode.com"),
            "https://ethereum-sepolia-rpc.publicnode.com"
        );
        assert_eq!(to_http_url("ws://127.0.0.1:8545"), "http://127.0.0.1:8545");
        assert_eq!(to_http_url("WS://127.0.0.1:8545"), "http://127.0.0.1:8545");
        assert_eq!(to_http_url("Wss://Host.Example/Path"), "https://Host.Example/Path");

        // Already HTTP: left exactly as given, host case and all.
        for url in [
            "http://127.0.0.1:8545",
            "https://Host.Example/v1/KEY",
            "HTTPS://Host.Example",
        ] {
            assert_eq!(to_http_url(url), url);
        }
    }

    // The rewrite is a convenience; this check is what makes the proof safe. A
    // scheme the rewrite does not know about must be a loud refusal, not a
    // count published with `corroborated: true` behind a split batch.
    #[tokio::test]
    async fn a_transport_that_cannot_batch_is_refused_before_connecting() {
        for url in [
            "ipc:///tmp/anvil.ipc",
            "file:///tmp/anvil.ipc",
            "127.0.0.1:8545",
            "/tmp/anvil.ipc",
        ] {
            let err = connect(url)
                .await
                .err()
                .unwrap_or_else(|| panic!("'{url}' cannot carry a JSON-RPC batch"));
            let SwapError::InvalidConfig(msg) = err else {
                panic!("expected an InvalidConfig refusal for '{url}', got: {err:?}");
            };
            assert!(
                msg.contains("http:// or https://"),
                "the refusal must name the transport it needs: {msg}"
            );
            assert!(
                msg.contains("ONE backend as ONE request"),
                "and why it needs it: {msg}"
            );
        }

        // A WebSocket URL is not refused — it is rewritten to the same host
        // over HTTP, in every spelling.
        for url in ["wss://example.invalid", "WSS://example.invalid"] {
            assert!(
                connect(url).await.is_ok(),
                "'{url}' rewrites to https:// and must be accepted"
            );
        }
    }

    // The sentence that terminates every failed run. The remedy differs by what
    // the probes saw, and `refusal_message` has already branched on it — so
    // this wrapper must not append a diagnosis that contradicts it.
    #[test]
    fn the_exhausted_retry_message_adds_no_diagnosis_of_its_own() {
        let throttled = report::refusal_message(
            100,
            200,
            &report::Anchoring::Unanchored {
                oldest: report::ProbeMiss {
                    rate_limited: 128,
                    ..Default::default()
                },
                newest: report::ProbeMiss {
                    rate_limited: 128,
                    ..Default::default()
                },
            },
        )
        .expect("a throttled scan is a refusal");
        let final_throttled = exhausted_message(&throttled);
        assert!(final_throttled.contains("asked for fewer requests"));
        assert!(final_throttled.contains("No count is published."));
        assert!(
            !final_throttled.contains("archive"),
            "a rate limit is not fixed by a different provider: {final_throttled}"
        );
        assert!(
            !final_throttled.contains("covers the whole range"),
            "no probe answered, so coverage was never established: {final_throttled}"
        );

        let broken = report::refusal_message(
            100,
            200,
            &report::Anchoring::Unanchored {
                oldest: report::ProbeMiss {
                    failed_requests: 80,
                    last_error: Some("connection reset by peer".into()),
                    ..Default::default()
                },
                newest: report::ProbeMiss {
                    failed_requests: 80,
                    last_error: Some("connection reset by peer".into()),
                    ..Default::default()
                },
            },
        )
        .expect("an errored scan is a refusal");
        let final_broken = exhausted_message(&broken);
        assert!(final_broken.contains("did not answer a single anchor probe"));
        assert!(final_broken.contains("serves historical logs reliably"));
        assert!(final_broken.contains("No count is published."));

        // What the wrapper itself contributes is true in both.
        for msg in [&final_throttled, &final_broken] {
            assert!(msg.contains(&MAX_SCAN_ATTEMPTS.to_string()));
            assert!(msg.contains("fresh connection"));
        }
    }

    // Reconnecting redraws a backend, which can only help a fault that a
    // different backend would not have. A wrong-era contract is not one of
    // those: it is a verdict the endpoint already delivered, and retrying it
    // five times would blunt the immediate startup refusal it is meant to be.
    #[test]
    fn only_a_transport_fault_earns_a_reconnect() {
        assert!(is_retryable_handshake_failure(&SwapError::EthRpc(
            "HTTP error 429 with body: {\"error\":{\"code\":-32005}}".into()
        )));
        assert!(is_retryable_handshake_failure(&SwapError::EthRpc(
            "connection refused".into()
        )));

        assert!(
            !is_retryable_handshake_failure(&SwapError::EthAbiMismatch(
                "reports INTERFACE_VERSION 1, but this app build speaks 2".into()
            )),
            "a version skew is the same on every backend"
        );
        assert!(!is_retryable_handshake_failure(&SwapError::InvalidConfig(
            "--eth-rpc-url must be an http:// or https:// endpoint".into()
        )));
        assert!(!is_retryable_handshake_failure(
            &SwapError::EthLogsUncorroborated("blocks 1-2: ...".into())
        ));
    }

    // Reconnecting is what gets past a pruned backend, but it is the wrong
    // answer to a 429: the next request goes out within milliseconds, which is
    // the burst that provoked the throttle in the first place. A throttled
    // request must wait; anything else should be retried or surfaced promptly.
    // Both throttled call sites on this path read the same zero/non-zero answer.
    #[test]
    fn only_a_throttled_request_waits_before_being_retried() {
        let throttled = SwapError::EthRpc(
            "HTTP error 429 with body: {\"error\":{\"code\":-32005,\"message\":\"Rate limit \
             exceeded\"}}"
                .into(),
        );
        // The schedule `scan_range` already uses for a throttled chunk.
        assert_eq!(
            throttle_backoff(&throttled, 1),
            report::RATE_LIMIT_BACKOFF
        );
        assert_eq!(
            throttle_backoff(&throttled, 2),
            report::RATE_LIMIT_BACKOFF * 2
        );
        assert_eq!(
            throttle_backoff(&throttled, 3),
            report::RATE_LIMIT_BACKOFF * 4
        );
        assert!(throttle_backoff(&throttled, 1) > std::time::Duration::ZERO);

        // A different backend fixes these, so waiting buys nothing.
        for err in [
            SwapError::EthRpc("connection refused".into()),
            SwapError::EthRpc("eth_chainId failed: 502 Bad Gateway".into()),
            SwapError::EthAbiMismatch("reports INTERFACE_VERSION 1".into()),
        ] {
            assert_eq!(
                throttle_backoff(&err, 1),
                std::time::Duration::ZERO,
                "{err:?} is not a throttle"
            );
        }
    }

    // The window search is ~17 serial eth_getBlockByNumber calls per bound over
    // the pinned venue's history, which is the burst shape this endpoint answers
    // with a 429. It must wait that out rather than abort the run — but only for
    // a throttle; anything else has nothing to gain from asking again.
    #[tokio::test(start_paused = true)]
    async fn the_window_search_waits_out_a_throttle_but_not_other_failures() {
        let calls = std::cell::Cell::new(0usize);
        let ts = block_timestamp_paced(11_417_462, || {
            let n = calls.get() + 1;
            calls.set(n);
            async move {
                if n <= 2 {
                    Err(SwapError::EthRpc(
                        "HTTP error 429 with body: {\"error\":{\"code\":-32005}}".into(),
                    ))
                } else {
                    Ok(1_787_270_400)
                }
            }
        })
        .await
        .expect("a throttle is waited out, not fatal");
        assert_eq!(ts, 1_787_270_400);
        assert_eq!(calls.get(), 3, "both throttled replies were retried");

        // Not a throttle: surfaced on the first failure, unchanged.
        let calls = std::cell::Cell::new(0usize);
        let err = block_timestamp_paced(11_417_462, || {
            calls.set(calls.get() + 1);
            async { Err(SwapError::EthRpc("block 11417462 not found".into())) }
        })
        .await
        .expect_err("a non-throttle failure is not retried");
        assert!(matches!(err, SwapError::EthRpc(ref m) if m.contains("not found")));
        assert_eq!(calls.get(), 1, "asking the same node again buys nothing");

        // A throttle that never clears still gives up rather than spinning.
        let calls = std::cell::Cell::new(0usize);
        assert!(
            block_timestamp_paced(11_417_462, || {
                calls.set(calls.get() + 1);
                async { Err(SwapError::EthRpc("429 Too Many Requests".into())) }
            })
            .await
            .is_err()
        );
        assert_eq!(
            calls.get() as u32,
            report::MAX_RATE_LIMIT_RETRIES + 1,
            "bounded by the module's own retry budget"
        );
    }

    #[tokio::test]
    async fn a_zero_chunk_size_is_refused() {
        let mut args = args_for(PINNED_ETH_HTLC_ADDRESS, None, None);
        args.chunk_blocks = 0;
        assert!(matches!(
            cmd_chain_report(args, false).await,
            Err(SwapError::InvalidConfig(_))
        ));
    }
}
