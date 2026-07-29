//! Liquidity-bot support for `swap-cli maker --loop`.
//!
//! Provides the pieces that turn the auto-accept maker loop
//! ([`crate::swap::maker::run_maker_loop`]) into an unattended daemon:
//!
//! - **State file + reconciliation** — the LEZ chain has no "list escrows by
//!   owner" query, so in-flight hashlocks are journaled to a small JSON file.
//!   On startup each journaled escrow is inspected on-chain and either
//!   refunded (LEZ timelock expired), completed (taker already revealed the
//!   preimage — claim the ETH), resumed (still live — background watcher), or
//!   dropped (terminal on-chain state).
//! - **Startup guards** — timelock-margin and LEZ-inventory validation.
//! - **Heartbeat offer publisher** — supervises a long-lived Node.js
//!   `@waku/sdk` lightpush sidecar that republishes the offer on an interval
//!   (the fleet runs `store=false`, so late-joining subscribers only see live
//!   messages).
//! - **Pinata faucet sidecar** (`--fund-to`) — loops `wallet pinata claim`
//!   (150 LEZ per claim) until the maker balance reaches a target.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use alloy::primitives::FixedBytes;
use lee_core::program::ProgramId;
use lez_htlc_program::HTLCState;
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;
use tracing::{error, info, warn};

use crate::config::{SwapConfig, account_id_to_base58};
use crate::error::{Result, SwapError};
use crate::eth::client::EthClient;
use crate::lez::client::LezClient;
use crate::lez::watcher::{self as lez_watcher, LezHtlcEvent};
use crate::swap::refund::now_unix;

// ---------------------------------------------------------------------------
// Timelock / inventory guards
// ---------------------------------------------------------------------------

/// Validate the maker-safety timelock invariant before the first offer:
/// the ETH (taker, long) timelock must exceed the LEZ (maker, short) timelock
/// by at least `margin_minutes`. A margin below 5 minutes is rejected — the
/// EthHTLC contract enforces `minTimelockDelta = 300s` and the maker needs
/// room to observe the preimage and claim.
pub fn validate_timelocks(lez_minutes: u64, eth_minutes: u64, margin_minutes: u64) -> Result<()> {
    if margin_minutes < 5 {
        return Err(SwapError::InvalidConfig(format!(
            "timelock margin must be >= 5 minutes (got {margin_minutes}); \
             EthHTLC minTimelockDelta is 300s and the maker needs claim headroom"
        )));
    }
    if eth_minutes < lez_minutes + margin_minutes {
        return Err(SwapError::InvalidConfig(format!(
            "unsafe timelocks: ETH_TIMELOCK_MINUTES ({eth_minutes}) must be >= \
             LEZ_TIMELOCK_MINUTES ({lez_minutes}) + margin ({margin_minutes}). \
             The taker locks first with the LONG timelock; the maker locks second \
             with the SHORT one."
        )));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Offer payload helpers
// ---------------------------------------------------------------------------

/// Encode a LEZ `ProgramId` (`[u32; 8]`) to the 64-hex wire form used in the
/// offer payload. Inverse of [`crate::config::parse_program_id`] (little-endian
/// per u32 word).
pub fn program_id_to_hex(id: &ProgramId) -> String {
    let mut bytes = Vec::with_capacity(32);
    for word in id {
        bytes.extend_from_slice(&word.to_le_bytes());
    }
    hex::encode(bytes)
}

// ---------------------------------------------------------------------------
// In-flight swap journal (crash recovery)
// ---------------------------------------------------------------------------

/// One journaled in-flight swap: recorded when the maker matches an ETH lock
/// (just before locking LEZ), removed when the swap reaches a terminal state.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InFlightSwap {
    /// 64-char lowercase hex, no 0x prefix.
    pub hashlock: String,
    /// 0x-prefixed 32-byte hex (EthHTLC swap id), if known.
    pub swap_id: String,
    /// Unix seconds when the entry was recorded.
    pub recorded_at: u64,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct BotState {
    in_flight: Vec<InFlightSwap>,
}

/// Small JSON journal of in-flight swaps, persisted on every mutation.
/// Needed because LEZ escrow PDAs are derived from the hashlock and cannot be
/// enumerated by owner — without the journal a crash strands locked LEZ until
/// someone replays the hashlock by hand.
pub struct StateStore {
    path: PathBuf,
    state: Mutex<BotState>,
}

impl StateStore {
    pub fn load(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref().to_path_buf();
        let state = match std::fs::read_to_string(&path) {
            Ok(raw) => serde_json::from_str(&raw).map_err(|e| {
                SwapError::InvalidConfig(format!(
                    "corrupt maker state file {}: {e} (move it aside to start fresh)",
                    path.display()
                ))
            })?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => BotState::default(),
            Err(e) => {
                return Err(SwapError::InvalidConfig(format!(
                    "cannot read maker state file {}: {e}",
                    path.display()
                )));
            }
        };
        Ok(Self {
            path,
            state: Mutex::new(state),
        })
    }

    fn persist(&self, state: &BotState) {
        let json = match serde_json::to_string_pretty(state) {
            Ok(j) => j,
            Err(e) => {
                error!("maker state serialize failed: {e}");
                return;
            }
        };
        let tmp = self.path.with_extension("json.tmp");
        if let Err(e) =
            std::fs::write(&tmp, &json).and_then(|_| std::fs::rename(&tmp, &self.path))
        {
            error!("maker state write failed ({}): {e}", self.path.display());
        }
    }

    /// Record an in-flight swap (idempotent on hashlock).
    pub fn add(&self, swap: InFlightSwap) {
        let mut state = self.state.lock().expect("state lock");
        if !state.in_flight.iter().any(|s| s.hashlock == swap.hashlock) {
            state.in_flight.push(swap);
            self.persist(&state);
        }
    }

    /// Remove a swap by hashlock (no-op if absent).
    pub fn remove(&self, hashlock: &str) {
        let mut state = self.state.lock().expect("state lock");
        let before = state.in_flight.len();
        state.in_flight.retain(|s| s.hashlock != hashlock);
        if state.in_flight.len() != before {
            self.persist(&state);
        }
    }

    pub fn snapshot(&self) -> Vec<InFlightSwap> {
        self.state.lock().expect("state lock").in_flight.clone()
    }
}

// ---------------------------------------------------------------------------
// Reconciliation
// ---------------------------------------------------------------------------

fn parse_hashlock_hex(s: &str) -> Option<[u8; 32]> {
    let bytes = hex::decode(s.trim_start_matches("0x")).ok()?;
    bytes.try_into().ok()
}

fn parse_swap_id(s: &str) -> Option<FixedBytes<32>> {
    s.parse().ok()
}

/// Claim the ETH side with a revealed preimage. Best-effort: errors are logged
/// (the HTLC may already be claimed/refunded), never fatal to the bot.
async fn claim_eth(config: &SwapConfig, swap_id_str: &str, preimage: [u8; 32]) {
    let Some(swap_id) = parse_swap_id(swap_id_str) else {
        warn!("reconcile: unparseable swap_id '{swap_id_str}', cannot claim ETH");
        return;
    };
    match EthClient::new(config).await {
        Ok(eth) => match eth.claim(swap_id, preimage).await {
            Ok(tx) => info!(%tx, "reconcile: claimed ETH with recovered preimage"),
            Err(e) => warn!("reconcile: ETH claim failed for {swap_id_str}: {e}"),
        },
        Err(e) => warn!("reconcile: ETH client init failed: {e}"),
    }
}

/// Background resumption of one live (unexpired, still-Locked) escrow found at
/// startup: watch it until the taker claims (→ claim ETH with the revealed
/// preimage) or the LEZ timelock expires (→ refund LEZ).
async fn resume_swap(
    config: SwapConfig,
    entry: InFlightSwap,
    hashlock: [u8; 32],
    timelock_ms: u64,
    store: Arc<StateStore>,
) {
    let lez_client = match LezClient::new(&config) {
        Ok(c) => c,
        Err(e) => {
            error!("resume: LEZ client init failed for {}: {e}", entry.hashlock);
            return;
        }
    };

    let (tx, mut rx) = mpsc::channel::<LezHtlcEvent>(16);
    let watcher_client = match LezClient::new(&config) {
        Ok(c) => c,
        Err(e) => {
            error!("resume: LEZ watcher init failed for {}: {e}", entry.hashlock);
            return;
        }
    };
    let poll = config.poll_interval;
    let watcher = tokio::spawn(async move {
        let _ = lez_watcher::watch_escrow(&watcher_client, hashlock, poll, tx).await;
    });

    let expiry_secs = (timelock_ms / 1000).saturating_sub(now_unix());
    info!(
        hashlock = %entry.hashlock,
        "resume: watching live escrow ({expiry_secs}s until LEZ timelock)"
    );

    loop {
        tokio::select! {
            Some(event) = rx.recv() => match event {
                LezHtlcEvent::Claimed { preimage, .. } => {
                    if let Ok(preimage) = <[u8; 32]>::try_from(preimage) {
                        info!(hashlock = %entry.hashlock, "resume: taker claimed LEZ, claiming ETH");
                        claim_eth(&config, &entry.swap_id, preimage).await;
                    } else {
                        warn!(hashlock = %entry.hashlock, "resume: preimage has wrong length");
                    }
                    break;
                }
                LezHtlcEvent::Refunded { .. } => {
                    info!(hashlock = %entry.hashlock, "resume: escrow already refunded");
                    break;
                }
                LezHtlcEvent::Locked { .. } => {}
            },
            _ = tokio::time::sleep(std::time::Duration::from_secs(expiry_secs.max(1))) => {
                info!(hashlock = %entry.hashlock, "resume: LEZ timelock expired, refunding");
                match lez_client.refund(&hashlock).await {
                    Ok(tx) => info!(%tx, "resume: LEZ refunded"),
                    Err(e) => warn!(hashlock = %entry.hashlock, "resume: LEZ refund failed: {e}"),
                }
                break;
            }
        }
    }

    watcher.abort();
    store.remove(&entry.hashlock);
}

/// Startup reconciliation pass: inspect every journaled in-flight swap
/// on-chain and refund / complete / resume / drop it. Never fatal — failures
/// are logged and the entry retained for the next restart.
pub async fn reconcile(config: &SwapConfig, store: &Arc<StateStore>, json: bool) {
    let entries = store.snapshot();
    if entries.is_empty() {
        return;
    }
    if !json {
        println!(
            "Reconciling {} in-flight swap(s) from previous run...",
            entries.len()
        );
    }

    let lez_client = match LezClient::new(config) {
        Ok(c) => c,
        Err(e) => {
            error!("reconcile: LEZ client init failed, keeping journal: {e}");
            return;
        }
    };

    for entry in entries {
        let Some(hashlock) = parse_hashlock_hex(&entry.hashlock) else {
            warn!("reconcile: dropping unparseable hashlock '{}'", entry.hashlock);
            store.remove(&entry.hashlock);
            continue;
        };

        match lez_client.get_escrow(&hashlock).await {
            Ok(None) => {
                info!(hashlock = %entry.hashlock, "reconcile: no escrow on-chain, dropping");
                store.remove(&entry.hashlock);
            }
            Ok(Some(escrow)) => match escrow.state {
                HTLCState::Claimed => {
                    // Taker revealed the preimage while we were down — collect the ETH.
                    match escrow.preimage.and_then(|p| <[u8; 32]>::try_from(p).ok()) {
                        Some(preimage) => {
                            info!(hashlock = %entry.hashlock, "reconcile: escrow claimed, claiming ETH");
                            claim_eth(config, &entry.swap_id, preimage).await;
                        }
                        None => warn!(
                            hashlock = %entry.hashlock,
                            "reconcile: escrow claimed but preimage missing/invalid"
                        ),
                    }
                    store.remove(&entry.hashlock);
                }
                HTLCState::Refunded => {
                    info!(hashlock = %entry.hashlock, "reconcile: escrow already refunded, dropping");
                    store.remove(&entry.hashlock);
                }
                HTLCState::Locked => {
                    // Escrow timelock is stored in milliseconds.
                    if now_unix() * 1000 >= escrow.timelock {
                        info!(hashlock = %entry.hashlock, "reconcile: escrow expired, refunding LEZ");
                        match lez_client.refund(&hashlock).await {
                            Ok(tx) => {
                                info!(%tx, "reconcile: LEZ refunded");
                                store.remove(&entry.hashlock);
                            }
                            Err(e) => {
                                // Keep the entry: retry on next restart.
                                warn!(hashlock = %entry.hashlock, "reconcile: refund failed: {e}");
                            }
                        }
                    } else {
                        // Still live — resume it in the background.
                        tokio::spawn(resume_swap(
                            config.clone(),
                            entry.clone(),
                            hashlock,
                            escrow.timelock,
                            store.clone(),
                        ));
                    }
                }
            },
            Err(e) => {
                warn!(hashlock = %entry.hashlock, "reconcile: escrow query failed, keeping: {e}");
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Heartbeat offer publisher (Node.js @waku/sdk lightpush sidecar)
// ---------------------------------------------------------------------------

/// Environment passed to the publisher sidecar. The sidecar recomputes fresh
/// absolute timelocks from the minute durations on every heartbeat.
pub struct OfferPublisherEnv {
    pub lez_amount: u128,
    pub eth_amount_wei: u128,
    pub maker_eth_address: String,
    pub maker_lez_account: String,
    pub lez_timelock_minutes: u64,
    pub eth_timelock_minutes: u64,
    pub lez_htlc_program_id_hex: String,
    pub eth_htlc_address: String,
}

impl OfferPublisherEnv {
    pub fn from_config(config: &SwapConfig, lez_minutes: u64, eth_minutes: u64) -> Self {
        let maker_lez_account = match &config.lez_auth {
            crate::config::LezAuth::Wallet { account_id, .. } => account_id_to_base58(account_id),
            crate::config::LezAuth::RawKey(_) => String::new(), // filled by caller via client
        };
        Self {
            lez_amount: config.lez_amount,
            eth_amount_wei: config.eth_amount,
            maker_eth_address: config.eth_recipient_address.to_string(),
            maker_lez_account,
            lez_timelock_minutes: lez_minutes,
            eth_timelock_minutes: eth_minutes,
            lez_htlc_program_id_hex: program_id_to_hex(&config.lez_htlc_program_id),
            eth_htlc_address: config.eth_htlc_address.to_string(),
        }
    }

    fn to_env(&self, heartbeat_secs: u64) -> Vec<(String, String)> {
        vec![
            ("OFFER_LEZ_AMOUNT".into(), self.lez_amount.to_string()),
            ("OFFER_ETH_AMOUNT_WEI".into(), self.eth_amount_wei.to_string()),
            ("OFFER_MAKER_ETH_ADDRESS".into(), self.maker_eth_address.clone()),
            ("OFFER_MAKER_LEZ_ACCOUNT".into(), self.maker_lez_account.clone()),
            (
                "OFFER_LEZ_TIMELOCK_MINUTES".into(),
                self.lez_timelock_minutes.to_string(),
            ),
            (
                "OFFER_ETH_TIMELOCK_MINUTES".into(),
                self.eth_timelock_minutes.to_string(),
            ),
            (
                "OFFER_LEZ_HTLC_PROGRAM_ID".into(),
                self.lez_htlc_program_id_hex.clone(),
            ),
            ("OFFER_ETH_HTLC_ADDRESS".into(), self.eth_htlc_address.clone()),
            ("OFFER_HEARTBEAT_SECS".into(), heartbeat_secs.to_string()),
        ]
    }
}

/// Spawn and supervise the long-lived Node.js lightpush sidecar. The child
/// connects to the Waku fleet once and republishes the offer every
/// `heartbeat_secs`. If it exits (crash, network loss) it is restarted with a
/// 30s backoff until `cancel` is set. The offer heartbeat is best-effort: its
/// failure never affects the swap loop (which coordinates purely on-chain).
pub fn spawn_offer_publisher(
    script: PathBuf,
    env: OfferPublisherEnv,
    heartbeat_secs: u64,
    cancel: Arc<AtomicBool>,
) -> tokio::task::JoinHandle<()> {
    let env_vars = env.to_env(heartbeat_secs);
    tokio::spawn(async move {
        loop {
            if cancel.load(Ordering::Relaxed) {
                return;
            }
            info!("offer publisher: starting {} (heartbeat {heartbeat_secs}s)", script.display());
            let mut cmd = tokio::process::Command::new("node");
            cmd.arg(&script).envs(env_vars.iter().cloned()).kill_on_drop(true);
            match cmd.spawn() {
                Ok(mut child) => match child.wait().await {
                    Ok(status) => {
                        warn!("offer publisher exited ({status}); restarting in 30s")
                    }
                    Err(e) => warn!("offer publisher wait failed: {e}; restarting in 30s"),
                },
                Err(e) => warn!(
                    "offer publisher spawn failed ({}): {e}; retrying in 30s \
                     (is node installed and `npm install` run in web/offer-board?)",
                    script.display()
                ),
            }
            for _ in 0..30 {
                if cancel.load(Ordering::Relaxed) {
                    return;
                }
                tokio::time::sleep(std::time::Duration::from_secs(1)).await;
            }
        }
    })
}

// ---------------------------------------------------------------------------
// Pinata faucet sidecar (--fund-to)
// ---------------------------------------------------------------------------

/// Loop `wallet pinata claim --to <maker>` until the maker LEZ balance reaches
/// `target`. Requires wallet-mode auth (LEZ_WALLET_HOME + LEZ_ACCOUNT_ID) —
/// the pinata faucet is driven through the standalone `wallet` binary.
/// Returns the final balance.
pub async fn fund_to_target(
    config: &SwapConfig,
    wallet_bin: &str,
    target: u128,
    json: bool,
) -> Result<u128> {
    let (home, account_id) = match &config.lez_auth {
        crate::config::LezAuth::Wallet { home, account_id } => (home.clone(), *account_id),
        crate::config::LezAuth::RawKey(_) => {
            return Err(SwapError::InvalidConfig(
                "--fund-to requires wallet-mode auth (LEZ_WALLET_HOME + LEZ_ACCOUNT_ID): \
                 pinata claims go through the `wallet` binary"
                    .into(),
            ));
        }
    };
    let account_b58 = account_id_to_base58(&account_id);
    let lez_client = LezClient::new(config)?;

    let mut consecutive_failures: u32 = 0;
    let mut claims: u64 = 0;
    loop {
        let balance = lez_client.get_balance(&account_id).await?;
        if balance >= target {
            if !json {
                println!("Funding target reached: balance {balance} >= {target} ({claims} claim(s))");
            }
            return Ok(balance);
        }
        let needed = target - balance;
        if !json {
            println!(
                "Balance {balance} < target {target} (need {needed}); claiming 150 LEZ from pinata..."
            );
        }
        let output = tokio::process::Command::new(wallet_bin)
            .env("LEE_WALLET_HOME_DIR", &home)
            .args(["pinata", "claim", "--to", &account_b58])
            .output()
            .await
            .map_err(|e| {
                SwapError::InvalidConfig(format!(
                    "failed to run `{wallet_bin} pinata claim` (set LEZ_WALLET_BIN?): {e}"
                ))
            })?;
        if output.status.success() {
            consecutive_failures = 0;
            claims += 1;
        } else {
            consecutive_failures += 1;
            warn!(
                "pinata claim failed ({}): {}",
                output.status,
                String::from_utf8_lossy(&output.stderr).trim()
            );
            if consecutive_failures >= 5 {
                return Err(SwapError::InvalidConfig(
                    "5 consecutive pinata claim failures — aborting funding loop".into(),
                ));
            }
        }
        // Let the claim land before re-checking the balance.
        tokio::time::sleep(std::time::Duration::from_secs(3)).await;
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::parse_program_id;

    #[test]
    fn timelock_guard_accepts_safe_margins() {
        assert!(validate_timelocks(20, 40, 5).is_ok());
        assert!(validate_timelocks(5, 10, 5).is_ok());
    }

    #[test]
    fn timelock_guard_rejects_inverted_or_tight() {
        // Inverted: LEZ longer than ETH.
        assert!(validate_timelocks(40, 20, 5).is_err());
        // Equal — no margin at all.
        assert!(validate_timelocks(20, 20, 5).is_err());
        // Margin too small even if requested.
        assert!(validate_timelocks(20, 40, 2).is_err());
        // Delta smaller than requested margin.
        assert!(validate_timelocks(20, 24, 5).is_err());
    }

    #[test]
    fn program_id_hex_roundtrips() {
        let hex_in = "0102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f20";
        let id = parse_program_id(hex_in).unwrap();
        assert_eq!(program_id_to_hex(&id), hex_in);
    }

    #[test]
    fn state_store_roundtrips_and_persists() {
        let path = std::env::temp_dir().join(format!(
            "maker-state-test-{}.json",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&path);

        let store = StateStore::load(&path).unwrap();
        assert!(store.snapshot().is_empty());

        store.add(InFlightSwap {
            hashlock: "ab".repeat(32),
            swap_id: format!("0x{}", "cd".repeat(32)),
            recorded_at: 123,
        });
        // Idempotent on hashlock.
        store.add(InFlightSwap {
            hashlock: "ab".repeat(32),
            swap_id: format!("0x{}", "cd".repeat(32)),
            recorded_at: 456,
        });
        assert_eq!(store.snapshot().len(), 1);

        // Reload from disk — entry survives a "crash".
        let store2 = StateStore::load(&path).unwrap();
        assert_eq!(store2.snapshot().len(), 1);
        assert_eq!(store2.snapshot()[0].recorded_at, 123);

        store2.remove(&"ab".repeat(32));
        assert!(store2.snapshot().is_empty());
        let store3 = StateStore::load(&path).unwrap();
        assert!(store3.snapshot().is_empty());

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn corrupt_state_file_is_an_error_not_a_wipe() {
        let path = std::env::temp_dir().join(format!(
            "maker-state-corrupt-{}.json",
            std::process::id()
        ));
        std::fs::write(&path, "{not json").unwrap();
        assert!(StateStore::load(&path).is_err());
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn hashlock_and_swap_id_parse() {
        assert!(parse_hashlock_hex(&"ab".repeat(32)).is_some());
        assert!(parse_hashlock_hex("zz").is_none());
        assert!(parse_hashlock_hex("abcd").is_none());
        assert!(parse_swap_id(&format!("0x{}", "cd".repeat(32))).is_some());
        assert!(parse_swap_id("nope").is_none());
    }
}
