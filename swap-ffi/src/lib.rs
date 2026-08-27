use std::ffi::{CStr, CString, c_char, c_void};
use std::sync::{Arc, OnceLock};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use serde::Deserialize;
use tokio::runtime::Runtime;

use sha2::{Digest, Sha256};

use alloy::providers::Provider;
use alloy::signers::local::PrivateKeySigner;

use swap_orchestrator::{
    cli::bot::{StateStore, reconcile, validate_timelocks},
    config::{
        LezAuth, SwapConfig, account_id_to_base58, eth_to_wei, parse_base58_account_id,
        parse_program_id,
    },
    eth::client::EthClient,
    lez::{
        client::LezClient,
        onboard::{FundingProgress, Signer as LezSigner, claim_to_target, sequencer_client},
    },
    ops::OpsLedger,
    swap::{
        maker::{AutoAcceptConfig, run_maker, run_maker_loop},
        progress::SwapProgress,
        refund::{now_unix, refund_eth, refund_lez},
        taker::run_taker,
        types::SwapOutcome,
    },
};

mod lez_htlc_program_id;
use lez_htlc_program_id::LEZ_HTLC_PROGRAM_ID_HEX;

fn runtime() -> &'static Runtime {
    static RT: OnceLock<Runtime> = OnceLock::new();
    RT.get_or_init(|| Runtime::new().expect("failed to create tokio runtime"))
}

/// Callback invoked on each progress event (called from a worker thread).
pub type ProgressCallback = Option<unsafe extern "C" fn(*const c_char, *mut c_void)>;

// ---------------------------------------------------------------------------
// JSON helpers
// ---------------------------------------------------------------------------

fn json_err(msg: &str) -> *mut c_char {
    let val = serde_json::json!({ "error": msg });
    to_c_string(&val.to_string())
}

fn to_c_string(s: &str) -> *mut c_char {
    CString::new(s).unwrap_or_default().into_raw()
}

unsafe fn c_str_to_str<'a>(ptr: *const c_char) -> Option<&'a str> {
    if ptr.is_null() {
        return None;
    }
    unsafe { CStr::from_ptr(ptr) }.to_str().ok()
}

/// Parse an optional 32-byte hex string from a C pointer.
/// Returns `None` for null pointers or empty strings.
unsafe fn parse_optional_bytes32(
    ptr: *const c_char,
    name: &str,
) -> std::result::Result<Option<[u8; 32]>, *mut c_char> {
    if ptr.is_null() {
        return Ok(None);
    }
    match unsafe { c_str_to_str(ptr) } {
        Some("") => Ok(None),
        Some(s) => {
            let s = s.strip_prefix("0x").unwrap_or(s);
            match hex::decode(s) {
                Ok(b) if b.len() == 32 => {
                    let mut arr = [0u8; 32];
                    arr.copy_from_slice(&b);
                    Ok(Some(arr))
                }
                Ok(_) => Err(json_err(&format!("{name} must be 32 bytes (64 hex chars)"))),
                Err(e) => Err(json_err(&format!("invalid {name} hex: {e}"))),
            }
        }
        None => Ok(None),
    }
}

fn env_key_to_config_key(key: &str) -> Option<&'static str> {
    match key {
        "ETH_RPC_URL" => Some("eth_rpc_url"),
        "ETH_PRIVATE_KEY" => Some("eth_private_key"),
        "ETH_HTLC_ADDRESS" => Some("eth_htlc_address"),
        "LEZ_SEQUENCER_URL" => Some("lez_sequencer_url"),
        "LEZ_SIGNING_KEY" => Some("lez_signing_key"),
        "LEZ_WALLET_HOME" => Some("lez_wallet_home"),
        "LEZ_ACCOUNT_ID" => Some("lez_account_id"),
        "LEZ_HTLC_PROGRAM_ID" => Some("lez_htlc_program_id"),
        "LEZ_AMOUNT" => Some("lez_amount"),
        "ETH_AMOUNT" => Some("eth_amount"),
        "LEZ_TIMELOCK_MINUTES" => Some("lez_timelock_minutes"),
        "ETH_TIMELOCK_MINUTES" => Some("eth_timelock_minutes"),
        "ETH_RECIPIENT_ADDRESS" => Some("eth_recipient_address"),
        "LEZ_TAKER_ACCOUNT_ID" => Some("lez_taker_account_id"),
        "POLL_INTERVAL_MS" => Some("poll_interval_ms"),
        _ => None,
    }
}

fn dotenv_config_json(path: &str) -> std::result::Result<String, String> {
    let iter = dotenvy::from_path_iter(path)
        .map_err(|e| format!("failed to read env file '{path}': {e}"))?;
    let mut config = serde_json::Map::new();

    for item in iter {
        let (key, value) = item.map_err(|e| format!("failed to parse env file '{path}': {e}"))?;
        if let Some(config_key) = env_key_to_config_key(&key) {
            config.insert(config_key.to_string(), serde_json::Value::String(value));
        }
    }

    if !config.contains_key("eth_timelock_minutes") {
        config.insert(
            "eth_timelock_minutes".into(),
            serde_json::Value::String("10".into()),
        );
    }
    if !config.contains_key("lez_timelock_minutes") {
        config.insert(
            "lez_timelock_minutes".into(),
            serde_json::Value::String("5".into()),
        );
    }
    if !config.contains_key("poll_interval_ms") {
        config.insert(
            "poll_interval_ms".into(),
            serde_json::Value::String("2000".into()),
        );
    }
    Ok(serde_json::Value::Object(config).to_string())
}

// ---------------------------------------------------------------------------
// Config parsing (mirrors ConfigArgs::into_swap_config at src/cli/mod.rs:93)
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct FfiConfig {
    eth_rpc_url: String,
    eth_private_key: String,
    eth_htlc_address: String,
    lez_sequencer_url: String,
    /// Raw signing key (hex). Used when wallet fields are absent.
    #[serde(default)]
    lez_signing_key: Option<String>,
    /// Scaffold wallet home directory. If set with lez_account_id, uses wallet auth.
    #[serde(default)]
    lez_wallet_home: Option<String>,
    /// Scaffold wallet account ID (base58). Required when lez_wallet_home is set.
    #[serde(default)]
    lez_account_id: Option<String>,
    // The fields below are `#[serde(default)]` so a config JSON from a form
    // the user has not finished (Setup tab, balance-only reads) still parses.
    // `parse_config` (the swap path) stays strict: an empty string fails the
    // typed parse with the same "invalid …" error a missing key used to give.
    // Only `parse_balance_config` tolerates them.
    #[serde(default)]
    lez_htlc_program_id: String,
    #[serde(default)]
    lez_amount: String,
    #[serde(default)]
    eth_amount: String,
    #[serde(default)]
    lez_timelock_minutes: String,
    #[serde(default)]
    eth_timelock_minutes: String,
    #[serde(default)]
    eth_recipient_address: String,
    /// OPTIONAL designated counterparty (base58). A maker no longer requires it
    /// — the taker publishes its own LEZ account in its ETH lock and the maker
    /// binds the escrow to that — so an absent or empty value is valid and
    /// means "serve any taker". When present it acts as an allowlist.
    #[serde(default)]
    lez_taker_account_id: Option<String>,
    #[serde(default = "default_poll")]
    poll_interval_ms: String,
}

fn default_poll() -> String {
    "2000".into()
}

fn parse_config(json_str: &str) -> Result<SwapConfig, String> {
    let c: FfiConfig =
        serde_json::from_str(json_str).map_err(|e| format!("bad config JSON: {e}"))?;

    let eth_htlc_address = c
        .eth_htlc_address
        .parse()
        .map_err(|e| format!("invalid eth_htlc_address: {e}"))?;
    let eth_recipient_address = c
        .eth_recipient_address
        .parse()
        .map_err(|e| format!("invalid eth_recipient_address: {e}"))?;
    let lez_htlc_program_id =
        parse_program_id(&c.lez_htlc_program_id).map_err(|e| e.to_string())?;
    // An empty string is treated as absent: the GUI always sends every config
    // key, and a blank Config-tab field must mean "no designated counterparty",
    // not "invalid base58".
    let lez_taker_account_id = match c
        .lez_taker_account_id
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        Some(s) => Some(parse_base58_account_id(s).map_err(|e| e.to_string())?),
        None => None,
    };

    let lez_amount: u128 = c
        .lez_amount
        .parse()
        .map_err(|e| format!("invalid lez_amount: {e}"))?;
    let eth_amount: u128 = eth_to_wei(&c.eth_amount)?;
    let lez_timelock_minutes: u64 = c
        .lez_timelock_minutes
        .parse()
        .map_err(|e| format!("invalid lez_timelock_minutes: {e}"))?;
    let eth_timelock_minutes: u64 = c
        .eth_timelock_minutes
        .parse()
        .map_err(|e| format!("invalid eth_timelock_minutes: {e}"))?;
    let poll_interval_ms: u64 = c
        .poll_interval_ms
        .parse()
        .map_err(|e| format!("invalid poll_interval_ms: {e}"))?;

    let now = now_unix();

    // The UI/config-JSON path always carries a sequencer URL entered in the
    // Config tab, so treat any non-empty value as explicit — it then overrides
    // the wallet config's sequencer_addr in wallet mode (matches the CLI's
    // resolve_sequencer_url semantics).
    let lez_sequencer_url_explicit = !c.lez_sequencer_url.is_empty();

    Ok(SwapConfig {
        eth_rpc_url: c.eth_rpc_url,
        eth_private_key: c.eth_private_key,
        eth_htlc_address,
        lez_sequencer_url: c.lez_sequencer_url,
        lez_sequencer_url_explicit,
        lez_auth: match (&c.lez_wallet_home, &c.lez_account_id) {
            (Some(home), Some(account_id)) => LezAuth::Wallet {
                home: std::path::PathBuf::from(home),
                account_id: parse_base58_account_id(account_id).map_err(|e| e.to_string())?,
            },
            _ => LezAuth::RawKey(
                c.lez_signing_key
                    .ok_or("lez_signing_key is required when lez_wallet_home is not set")?,
            ),
        },
        lez_htlc_program_id,
        lez_amount,
        eth_amount,
        lez_timelock: now + lez_timelock_minutes * 60,
        eth_timelock: now + eth_timelock_minutes * 60,
        eth_recipient_address,
        lez_taker_account_id,
        poll_interval: Duration::from_millis(poll_interval_ms),
    })
}

/// Balance-read config: the LENIENT counterpart of `parse_config`.
///
/// A balance read needs an RPC endpoint and an account per chain — nothing
/// else. It must not demand the swap-only fields (amounts, timelocks,
/// recipient, program ID): on a fresh install the GUI refreshes balances
/// while the user is still on the Setup tab, long before those exist, and a
/// strict parse turned that refresh into a red "fix validation errors" banner
/// over a Setup screen that was working fine.
///
/// Per-chain readiness is reported, not enforced: a missing ETH key or LEZ
/// account leaves that side's `Option` as `None` and `swap_ffi_fetch_balances`
/// reports a per-side error for it, so the other chain still reads. The
/// swap-only fields are filled with inert placeholders — this config is never
/// handed to a swap; every swap entry point calls the strict `parse_config`.
#[derive(Debug)]
struct BalanceConfig {
    /// Full config for the ETH read (`EthClient::new`), `None` when the ETH
    /// side is not set up yet (empty RPC URL, key, or HTLC address).
    eth: Option<SwapConfig>,
    /// Full config for the LEZ read (`LezClient::new`), `None` when the LEZ
    /// side is not set up yet (empty sequencer URL or no auth at all).
    lez: Option<SwapConfig>,
}

fn parse_balance_config(json_str: &str) -> Result<BalanceConfig, String> {
    let c: FfiConfig =
        serde_json::from_str(json_str).map_err(|e| format!("bad config JSON: {e}"))?;

    let has_wallet = matches!(
        (&c.lez_wallet_home, &c.lez_account_id),
        (Some(h), Some(a)) if !h.trim().is_empty() && !a.trim().is_empty()
    );
    let has_raw_key = c
        .lez_signing_key
        .as_deref()
        .map(|k| !k.trim().is_empty())
        .unwrap_or(false);

    let eth_ready = !c.eth_rpc_url.trim().is_empty()
        && !c.eth_private_key.trim().is_empty()
        && !c.eth_htlc_address.trim().is_empty();
    let lez_ready = !c.lez_sequencer_url.trim().is_empty() && (has_wallet || has_raw_key);

    if !eth_ready && !lez_ready {
        return Err("no chain is set up for a balance read yet: need an ETH RPC URL \
                    plus key, or a LEZ sequencer URL plus account"
            .into());
    }

    // Inert placeholders for everything a balance read never touches.
    let eth_htlc_address: alloy::primitives::Address = if eth_ready {
        c.eth_htlc_address
            .parse()
            .map_err(|e| format!("invalid eth_htlc_address: {e}"))?
    } else {
        alloy::primitives::Address::ZERO
    };
    let lez_htlc_program_id = if c.lez_htlc_program_id.trim().is_empty() {
        // All-zero placeholder; a balance read never touches the program.
        parse_program_id(&"00".repeat(32)).map_err(|e| e.to_string())?
    } else {
        parse_program_id(&c.lez_htlc_program_id).map_err(|e| e.to_string())?
    };
    let lez_auth = if has_wallet {
        LezAuth::Wallet {
            home: std::path::PathBuf::from(c.lez_wallet_home.clone().unwrap_or_default()),
            account_id: parse_base58_account_id(c.lez_account_id.as_deref().unwrap_or(""))
                .map_err(|e| e.to_string())?,
        }
    } else {
        LezAuth::RawKey(c.lez_signing_key.clone().unwrap_or_default())
    };
    let now = now_unix();
    let base = SwapConfig {
        eth_rpc_url: c.eth_rpc_url,
        eth_private_key: c.eth_private_key,
        eth_htlc_address,
        lez_sequencer_url: c.lez_sequencer_url.clone(),
        lez_sequencer_url_explicit: !c.lez_sequencer_url.is_empty(),
        lez_auth,
        lez_htlc_program_id,
        lez_amount: 0,
        eth_amount: 0,
        lez_timelock: now,
        eth_timelock: now,
        eth_recipient_address: alloy::primitives::Address::ZERO,
        lez_taker_account_id: None,
        poll_interval: Duration::from_millis(2000),
    };

    Ok(BalanceConfig {
        eth: eth_ready.then(|| base.clone()),
        lez: lez_ready.then_some(base),
    })
}

// ---------------------------------------------------------------------------
// Outcome serialization
// ---------------------------------------------------------------------------

fn outcome_to_json(outcome: &SwapOutcome, hashlock: &[u8; 32]) -> String {
    match outcome {
        SwapOutcome::Completed {
            preimage,
            eth_tx,
            lez_tx,
        } => serde_json::json!({
            "status": "completed",
            "preimage": hex::encode(preimage),
            "eth_tx": format!("{eth_tx}"),
            "lez_tx": lez_tx,
            "hashlock": hex::encode(hashlock),
        })
        .to_string(),
        SwapOutcome::Refunded {
            eth_refund_tx,
            lez_refund_tx,
        } => serde_json::json!({
            "status": "refunded",
            "eth_refund_tx": eth_refund_tx.map(|tx| format!("{tx}")),
            "lez_refund_tx": lez_refund_tx,
            "hashlock": hex::encode(hashlock),
        })
        .to_string(),
    }
}

// ---------------------------------------------------------------------------
// Progress forwarding
// ---------------------------------------------------------------------------

fn forward_progress(
    cb: ProgressCallback,
    user_data: *mut c_void,
) -> (
    Option<swap_orchestrator::swap::progress::ProgressSender>,
    Option<tokio::task::JoinHandle<()>>,
) {
    let Some(cb) = cb else {
        return (None, None);
    };
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<SwapProgress>();

    // user_data is thread-safe (opaque pointer managed by the C++ caller).
    let ud = user_data as usize;
    let handle = tokio::spawn(async move {
        while let Some(progress) = rx.recv().await {
            if let Ok(json) = serde_json::to_string(&progress)
                && let Ok(c_str) = CString::new(json)
            {
                unsafe { cb(c_str.as_ptr(), ud as *mut c_void) };
            }
        }
    });

    (Some(tx), Some(handle))
}

async fn drain_progress_forwarder(handle: Option<tokio::task::JoinHandle<()>>) {
    if let Some(handle) = handle {
        let _ = handle.await;
    }
}

/// Same shape as [`forward_progress`], for [`FundingProgress`] events (the
/// onboarding funding job) rather than [`SwapProgress`] (maker/taker swaps).
/// Kept separate rather than made generic: the two event enums are forwarded
/// on entirely different job roles and a shared generic would buy nothing but
/// an extra type parameter at every call site.
fn forward_funding_progress(
    cb: ProgressCallback,
    user_data: *mut c_void,
) -> (
    Option<tokio::sync::mpsc::UnboundedSender<FundingProgress>>,
    Option<tokio::task::JoinHandle<()>>,
) {
    let Some(cb) = cb else {
        return (None, None);
    };
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<FundingProgress>();

    let ud = user_data as usize;
    let handle = tokio::spawn(async move {
        while let Some(progress) = rx.recv().await {
            if let Ok(json) = serde_json::to_string(&progress)
                && let Ok(c_str) = CString::new(json)
            {
                unsafe { cb(c_str.as_ptr(), ud as *mut c_void) };
            }
        }
    });

    (Some(tx), Some(handle))
}

// ---------------------------------------------------------------------------
// FFI exports
// ---------------------------------------------------------------------------

/// Load environment variables from a .env file.
///
/// # Safety
/// `path` must be a valid null-terminated C string, or null to use the default ".env".
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swap_ffi_load_env(path: *const c_char) -> *mut c_char {
    let path_str = if path.is_null() {
        ".env"
    } else {
        match unsafe { c_str_to_str(path) } {
            Some(s) => s,
            None => return json_err("invalid UTF-8 path"),
        }
    };

    match dotenv_config_json(path_str) {
        Ok(json) => to_c_string(&json),
        Err(e) => json_err(&e),
    }
}

/// Run the maker flow (taker-locks-first). Blocks until the swap completes or times out.
///
/// The maker receives a hashlock, watches for the taker's ETH lock, locks LEZ,
/// waits for the taker to claim LEZ (revealing the preimage), then claims ETH.
///
/// # Safety
/// `config_json` must be a valid null-terminated JSON C string.
/// `hashlock_hex` must be a valid 64-char hex string (the taker's hashlock).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swap_ffi_run_maker(
    config_json: *const c_char,
    hashlock_hex: *const c_char,
    cb: ProgressCallback,
    user_data: *mut c_void,
) -> *mut c_char {
    let json_str = match unsafe { c_str_to_str(config_json) } {
        Some(s) => s,
        None => return json_err("null or invalid config_json"),
    };

    let config = match parse_config(json_str) {
        Ok(c) => c,
        Err(e) => return json_err(&e),
    };

    let hashlock_opt = match unsafe { parse_optional_bytes32(hashlock_hex, "hashlock") } {
        Ok(v) => v,
        Err(e) => return e,
    };

    runtime().block_on(async {
        let eth_client = match EthClient::new(&config).await {
            Ok(c) => c,
            Err(e) => return json_err(&e.to_string()),
        };
        let lez_client = match LezClient::new(&config) {
            Ok(c) => c,
            Err(e) => return json_err(&e.to_string()),
        };

        let (progress, progress_forwarder) = forward_progress(cb, user_data);
        let result = match run_maker(
            &config,
            &eth_client,
            &lez_client,
            hashlock_opt,
            None,
            progress,
            // Loop-only guards (timelock margin + crash journal) stay off for
            // the single-shot UI swap, matching the CLI single-shot path.
            None,
        )
        .await
        {
            Ok(ref outcome) => {
                let hashlock = match outcome {
                    SwapOutcome::Completed { preimage, .. } => Sha256::digest(preimage).into(),
                    _ => hashlock_opt.unwrap_or([0u8; 32]),
                };
                to_c_string(&outcome_to_json(outcome, &hashlock))
            }
            Err(e) => json_err(&e.to_string()),
        };
        drain_progress_forwarder(progress_forwarder).await;
        result
    })
}

/// Run the taker flow (taker-locks-first). Blocks until the swap completes or times out.
///
/// The taker generates a preimage, locks ETH first, waits for the maker to lock LEZ,
/// then claims LEZ (revealing the preimage on the LEZ chain).
///
/// # Safety
/// `config_json` must be a valid null-terminated JSON C string.
/// `preimage_hex` may be null (taker generates internally) or a 64-char hex string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swap_ffi_run_taker(
    config_json: *const c_char,
    preimage_hex: *const c_char,
    cb: ProgressCallback,
    user_data: *mut c_void,
) -> *mut c_char {
    let json_str = match unsafe { c_str_to_str(config_json) } {
        Some(s) => s,
        None => return json_err("null or invalid config_json"),
    };

    let config = match parse_config(json_str) {
        Ok(c) => c,
        Err(e) => return json_err(&e),
    };

    let override_preimage = match unsafe { parse_optional_bytes32(preimage_hex, "preimage") } {
        Ok(v) => v,
        Err(e) => return e,
    };

    runtime().block_on(async {
        let eth_client = match EthClient::new(&config).await {
            Ok(c) => c,
            Err(e) => return json_err(&e.to_string()),
        };
        let lez_client = match LezClient::new(&config) {
            Ok(c) => c,
            Err(e) => return json_err(&e.to_string()),
        };

        let (progress, progress_forwarder) = forward_progress(cb, user_data);
        let result = match run_taker(
            &config,
            &eth_client,
            &lez_client,
            override_preimage,
            progress,
        )
        .await
        {
            Ok(ref outcome) => {
                let hashlock = match outcome {
                    SwapOutcome::Completed { preimage, .. } => Sha256::digest(preimage).into(),
                    _ => [0u8; 32],
                };
                to_c_string(&outcome_to_json(outcome, &hashlock))
            }
            Err(e) => json_err(&e.to_string()),
        };
        drain_progress_forwarder(progress_forwarder).await;
        result
    })
}

/// Refund LEZ from an HTLC escrow.
///
/// # Safety
/// `config_json` and `hashlock_hex` must be valid null-terminated C strings.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swap_ffi_refund_lez(
    config_json: *const c_char,
    hashlock_hex: *const c_char,
) -> *mut c_char {
    let json_str = match unsafe { c_str_to_str(config_json) } {
        Some(s) => s,
        None => return json_err("null or invalid config_json"),
    };
    let hashlock_str = match unsafe { c_str_to_str(hashlock_hex) } {
        Some(s) => s.strip_prefix("0x").unwrap_or(s),
        None => return json_err("null or invalid hashlock_hex"),
    };

    let config = match parse_config(json_str) {
        Ok(c) => c,
        Err(e) => return json_err(&e),
    };

    let hashlock_bytes = match hex::decode(hashlock_str) {
        Ok(b) if b.len() == 32 => {
            let mut arr = [0u8; 32];
            arr.copy_from_slice(&b);
            arr
        }
        Ok(_) => return json_err("hashlock must be 32 bytes (64 hex chars)"),
        Err(e) => return json_err(&format!("invalid hashlock hex: {e}")),
    };

    runtime().block_on(async {
        let lez_client = match LezClient::new(&config) {
            Ok(c) => c,
            Err(e) => return json_err(&e.to_string()),
        };

        match refund_lez(&lez_client, &hashlock_bytes, config.lez_timelock).await {
            Ok(tx_hash) => {
                to_c_string(&serde_json::json!({ "ok": true, "tx_hash": tx_hash }).to_string())
            }
            Err(e) => json_err(&e.to_string()),
        }
    })
}

/// Refund ETH from an HTLC contract.
///
/// # Safety
/// `config_json` and `swap_id_hex` must be valid null-terminated C strings.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swap_ffi_refund_eth(
    config_json: *const c_char,
    swap_id_hex: *const c_char,
) -> *mut c_char {
    let json_str = match unsafe { c_str_to_str(config_json) } {
        Some(s) => s,
        None => return json_err("null or invalid config_json"),
    };
    let swap_id_str = match unsafe { c_str_to_str(swap_id_hex) } {
        Some(s) => s.strip_prefix("0x").unwrap_or(s),
        None => return json_err("null or invalid swap_id_hex"),
    };

    let config = match parse_config(json_str) {
        Ok(c) => c,
        Err(e) => return json_err(&e),
    };

    let swap_id_bytes = match hex::decode(swap_id_str) {
        Ok(b) if b.len() == 32 => alloy_primitives::FixedBytes::<32>::from_slice(&b),
        Ok(_) => return json_err("swap_id must be 32 bytes (64 hex chars)"),
        Err(e) => return json_err(&format!("invalid swap_id hex: {e}")),
    };

    runtime().block_on(async {
        let eth_client = match EthClient::new(&config).await {
            Ok(c) => c,
            Err(e) => return json_err(&e.to_string()),
        };

        match refund_eth(&eth_client, swap_id_bytes).await {
            Ok(tx_hash) => to_c_string(
                &serde_json::json!({ "ok": true, "tx_hash": format!("{tx_hash}") }).to_string(),
            ),
            Err(e) => json_err(&e.to_string()),
        }
    })
}

/// Fetch ETH and LEZ wallet balances concurrently.
///
/// Returns JSON with eth_address, eth_balance, lez_account, lez_balance.
/// Each chain is independent — one failing doesn't block the other.
///
/// # Safety
/// `config_json` must be a valid null-terminated JSON C string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swap_ffi_fetch_balances(config_json: *const c_char) -> *mut c_char {
    let json_str = match unsafe { c_str_to_str(config_json) } {
        Some(s) => s,
        None => return json_err("null or invalid config_json"),
    };

    // Lenient on purpose — see parse_balance_config. The swap-only fields are
    // not needed to read a balance and must not block one.
    let BalanceConfig { eth: eth_config, lez: lez_config } = match parse_balance_config(json_str) {
        Ok(c) => c,
        Err(e) => return json_err(&e),
    };

    const ETH_NOT_SET_UP: &str = "Ethereum key not set up yet";
    const LEZ_NOT_SET_UP: &str = "LEZ account not set up yet";

    // Derive ETH address from private key.
    let eth_signer: Option<std::result::Result<PrivateKeySigner, String>> = eth_config
        .as_ref()
        .map(|c| c.eth_private_key.parse().map_err(|e| format!("invalid ETH private key: {e}")));
    let eth_address = eth_signer
        .as_ref()
        .and_then(|r| r.as_ref().ok())
        .map(|s| format!("{}", s.address()));

    // Derive LEZ account ID.
    let lez_client_result = lez_config.as_ref().map(LezClient::new);
    let lez_account = lez_client_result
        .as_ref()
        .and_then(|r| r.as_ref().ok())
        .map(|c| account_id_to_base58(&c.account_id()));

    runtime().block_on(async {
        // Fetch ETH balance.
        let eth_fut = async {
            let config = eth_config.as_ref().ok_or_else(|| ETH_NOT_SET_UP.to_string())?;
            let signer = eth_signer.clone().ok_or_else(|| ETH_NOT_SET_UP.to_string())??;
            let addr = signer.address();
            let eth_client = EthClient::new(config).await.map_err(|e| e.to_string())?;
            let balance = eth_client
                .provider()
                .get_balance(addr)
                .await
                .map_err(|e| e.to_string())?;
            Ok::<String, String>(balance.to_string())
        };

        // Fetch LEZ balance.
        let lez_fut = async {
            let client = lez_client_result
                .as_ref()
                .ok_or_else(|| LEZ_NOT_SET_UP.to_string())?
                .as_ref()
                .map_err(|e| e.to_string())?;
            let balance = client
                .get_balance(&client.account_id())
                .await
                .map_err(|e| e.to_string())?;
            Ok::<String, String>(balance.to_string())
        };

        let (eth_result, lez_result) = tokio::join!(eth_fut, lez_fut);

        let result = serde_json::json!({
            "eth_address": eth_address,
            "eth_balance": eth_result.as_ref().ok(),
            "eth_error": eth_result.as_ref().err(),
            "lez_account": lez_account,
            "lez_balance": lez_result.as_ref().ok(),
            "lez_error": lez_result.as_ref().err(),
        });

        to_c_string(&result.to_string())
    })
}

// ---------------------------------------------------------------------------
// Maker auto-accept loop
// ---------------------------------------------------------------------------

static MAKER_LOOP_CANCEL: AtomicBool = AtomicBool::new(false);

/// Resolve where the maker loop's crash-recovery journal lives.
///
/// The CLI's `MAKER_STATE_FILE` env knob wins; otherwise the journal is
/// persisted under the module's LEZ wallet home (the per-module storage
/// directory the FFI config carries in wallet-auth mode). Raw-key auth with no
/// explicit path is REFUSED rather than defaulted to the CLI's CWD-relative
/// `.maker-state.json`: a ui-host's working directory is not stable across
/// launch methods (Finder launches at `/`, launchers vary), so a CWD-relative
/// journal written before a crash may resolve to a different directory on the
/// next launch — reconcile would then see an empty journal, stranding the
/// locked LEZ, and the 256-block replay watcher could rematch the still-OPEN
/// ETH lock with an empty journal-skip belt (double-fund). A journal that can
/// silently go missing voids the crash-recovery guarantee (PR #40), so the
/// loop must not start with one.
fn resolve_maker_state_file(
    env_state_file: Option<&str>,
    lez_auth: &LezAuth,
) -> std::result::Result<std::path::PathBuf, String> {
    if let Some(p) = env_state_file
        && !p.trim().is_empty()
    {
        return Ok(p.into());
    }
    match lez_auth {
        LezAuth::Wallet { home, .. } => Ok(home.join(".maker-state.json")),
        LezAuth::RawKey(_) => Err(
            "maker loop requires a stable crash-recovery journal location, but raw-key auth \
             has no wallet home and the process working directory is not stable across \
             launches. Either set MAKER_STATE_FILE to an absolute path, or configure a LEZ \
             wallet home (lez_wallet_home + lez_account_id, env LEZ_WALLET_HOME + \
             LEZ_ACCOUNT_ID) so the journal can live there."
            .into(),
        ),
    }
}

/// Run the maker in an auto-accept loop. Blocks until cancelled or an
/// unrecoverable error. Running out of LEZ inventory no longer stops the
/// loop (issue #93 point 3 on the CLI side): it emits
/// `AutoAcceptInsufficientFunds` progress and waits, retrying the balance
/// check, rather than exiting — the caller decides whether to call
/// `swap_ffi_stop_maker_loop` or top up externally. Returns JSON:
/// `{ "completed": N, "failed": M }`.
///
/// # Safety
/// `config_json` must be a valid null-terminated JSON C string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swap_ffi_run_maker_loop(
    config_json: *const c_char,
    cb: ProgressCallback,
    user_data: *mut c_void,
) -> *mut c_char {
    MAKER_LOOP_CANCEL.store(false, Ordering::SeqCst);

    let json_str = match unsafe { c_str_to_str(config_json) } {
        Some(s) => s,
        None => return json_err("null or invalid config_json"),
    };

    // Parse FfiConfig to extract raw minutes before parse_config converts to absolute.
    let ffi_config: FfiConfig = match serde_json::from_str(json_str) {
        Ok(c) => c,
        Err(e) => return json_err(&format!("bad config JSON: {e}")),
    };
    let lez_timelock_minutes: u64 = match ffi_config.lez_timelock_minutes.parse() {
        Ok(v) => v,
        Err(e) => return json_err(&format!("invalid lez_timelock_minutes: {e}")),
    };
    let eth_timelock_minutes: u64 = match ffi_config.eth_timelock_minutes.parse() {
        Ok(v) => v,
        Err(e) => return json_err(&format!("invalid eth_timelock_minutes: {e}")),
    };

    let base_config = match parse_config(json_str) {
        Ok(c) => c,
        Err(e) => return json_err(&e),
    };

    let auto_config = AutoAcceptConfig {
        lez_timelock_minutes,
        eth_timelock_minutes,
    };

    // Timelock-margin startup guard, mirroring `swap-cli maker --loop`: honor
    // the same TIMELOCK_MARGIN_MINUTES env knob and default to the CLI's 5
    // minutes. The loop enforces this margin again at runtime against every
    // matched ETH lock (P1-1), so validating here just fails fast on a config
    // the loop would reject every lock for.
    let margin_minutes: u64 = match std::env::var("TIMELOCK_MARGIN_MINUTES") {
        Ok(v) => match v.trim().parse() {
            Ok(m) => m,
            Err(e) => return json_err(&format!("invalid TIMELOCK_MARGIN_MINUTES: {e}")),
        },
        Err(_) => 5,
    };
    if let Err(e) = validate_timelocks(lez_timelock_minutes, eth_timelock_minutes, margin_minutes) {
        return json_err(&e.to_string());
    }

    // Durable crash-recovery journal (fund-safety, PR #40): an in-flight swap
    // is recorded (fsync'd) BEFORE the LEZ lock and cleared only on a confirmed
    // terminal state, so a crash cannot strand locked LEZ.
    let state_file = match resolve_maker_state_file(
        std::env::var("MAKER_STATE_FILE").ok().as_deref(),
        &base_config.lez_auth,
    ) {
        Ok(p) => p,
        Err(e) => return json_err(&e),
    };

    runtime().block_on(async {
        let store = match StateStore::load(&state_file) {
            Ok(s) => Arc::new(s),
            Err(e) => return json_err(&e.to_string()),
        };

        // issue #98: durable, privacy-safe ops ledger, sibling of the
        // fund-safety journal — see `swap_orchestrator::ops` module docs.
        let ops_file = swap_orchestrator::ops::default_ledger_file(&state_file);
        let ops_ledger = match OpsLedger::load(&ops_file) {
            Ok(o) => Arc::new(o),
            Err(e) => return json_err(&e.to_string()),
        };

        // Crash recovery FIRST, mirroring the CLI loop (P1-3): reconcile
        // journaled in-flight swaps (refund / claim / resume) and fully RESOLVE
        // them before accepting any new swap (P1-A), so the 256-block replay
        // watcher can never rematch a still-OPEN lock and double-fund it.
        let resume_handles = reconcile(&base_config, &store, /* json = */ true, &ops_ledger).await;
        for handle in resume_handles {
            let _ = handle.await;
        }

        // If entries remain unresolved (reconcile could not reach a terminal
        // state — e.g. RPC/sequencer trouble), refuse to start, exactly like
        // the CLI's stop-for-supervised-restart gate: running the loop past
        // unresolved funds would let a fresh watcher rematch their locks.
        let unresolved = store.snapshot().len();
        if unresolved > 0 {
            return json_err(&format!(
                "{unresolved} in-flight swap(s) still unresolved after reconciliation; \
                 not starting the maker loop — check RPC/sequencer connectivity and retry \
                 (journal: {})",
                state_file.display()
            ));
        }

        let (progress, progress_forwarder) = forward_progress(cb, user_data);
        let result = run_maker_loop(
            &base_config,
            &auto_config,
            &MAKER_LOOP_CANCEL,
            progress,
            store.as_ref(),
            margin_minutes * 60,
            ops_ledger.as_ref(),
        )
        .await;
        let json = serde_json::json!({
            "completed": result.total_completed,
            "failed": result.total_failed,
        });
        drain_progress_forwarder(progress_forwarder).await;
        to_c_string(&json.to_string())
    })
}

/// Signal the maker auto-accept loop to stop after the current iteration.
#[unsafe(no_mangle)]
pub extern "C" fn swap_ffi_stop_maker_loop() {
    MAKER_LOOP_CANCEL.store(true, Ordering::SeqCst);
}

/// Return the canonical LEZ HTLC program ID compiled into this library as a
/// 64-char lowercase hex string (no `0x` prefix). Lets the UI default a
/// maker's program-ID field from the single canonical source instead of
/// hand-pasting it.
///
/// The value is a checked-in constant (see `lez_htlc_program_id.rs`): the
/// risc0 ImageID of the `lez-htlc` guest as DEPLOYED on the public testnet
/// (`testnet.lez.logos.co`), which is what a swap on that network executes
/// against. A `demo`-gated test guards it against drifting from
/// `lez_htlc_methods::LEZ_HTLC_PROGRAM_ID`.
///
/// The returned pointer must be freed with `swap_ffi_free_string`.
#[unsafe(no_mangle)]
pub extern "C" fn swap_ffi_default_lez_htlc_program_id() -> *mut c_char {
    to_c_string(LEZ_HTLC_PROGRAM_ID_HEX)
}

// ---------------------------------------------------------------------------
// Onboarding: key generation + LEZ account init/funding
// ---------------------------------------------------------------------------
//
// Replaces the worst part of first-run setup — hand-typing two private keys
// and two long account IDs into the Config tab (see #87/#91) — with buttons.
// No new crypto: ETH generation mirrors the existing `eth_private_key.parse()`
// path at the top of this file (`PrivateKeySigner`), and LEZ account
// creation/init/funding are thin wrappers over `src/lez/onboard.rs` (lifted
// from `lez-mcp` in #77 and live-verified against the public testnet).

/// Generate a fresh random ETH signing key. No network call.
///
/// Returns JSON `{"private_key":"0x...","address":"0x..."}`. The address is
/// what a taker should publish as its own `eth_recipient_address` — it is
/// the same key that will sign/claim its own ETH lock.
///
/// The returned pointer must be freed with `swap_ffi_free_string`.
#[unsafe(no_mangle)]
pub extern "C" fn swap_ffi_generate_eth_key() -> *mut c_char {
    let signer = PrivateKeySigner::random();
    let private_key = format!("0x{}", hex::encode(signer.to_bytes()));
    let address = format!("{}", signer.address());
    to_c_string(
        &serde_json::json!({
            "private_key": private_key,
            "address": address,
        })
        .to_string(),
    )
}

/// Generate a fresh LEZ signing key + its derived account ID. No network
/// call — the account does not exist on-chain until
/// `swap_ffi_lez_ensure_initialized` (or `swap_ffi_lez_claim_to_target`, which
/// calls that first) runs.
///
/// Returns JSON `{"signing_key":"<64-char hex>","account_id":"<base58>"}`.
///
/// The returned pointer must be freed with `swap_ffi_free_string`.
#[unsafe(no_mangle)]
pub extern "C" fn swap_ffi_generate_lez_account() -> *mut c_char {
    let signer = match LezSigner::generate() {
        Ok(s) => s,
        Err(e) => return json_err(&e),
    };
    to_c_string(
        &serde_json::json!({
            "signing_key": signer.signing_key.to_string(),
            "account_id": account_id_to_base58(&signer.account_id),
        })
        .to_string(),
    )
}

/// Idempotently ensure a LEZ account is initialized (owned by the
/// `authenticated_transfer` program) on-chain. Safe to call at any time —
/// checks on-chain ownership first and only submits a transaction if it
/// isn't already initialized.
///
/// `swap_ffi_lez_claim_to_target` below also calls this first internally
/// (see `src/lez/onboard.rs::claim_to_target`), so a caller that skips
/// straight to funding still gets init-before-claim: that ordering guarantee
/// lives in the Rust layer, not in whichever order a UI happens to call
/// these two functions.
///
/// Returns JSON `{"outcome":"AlreadyInitialized"}` or
/// `{"outcome":"Initialized","data":{"tx_hash":"..."}}` on success.
///
/// # Safety
/// `sequencer_url` and `signing_key_hex` must be valid null-terminated C strings.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swap_ffi_lez_ensure_initialized(
    sequencer_url: *const c_char,
    signing_key_hex: *const c_char,
) -> *mut c_char {
    let sequencer_url = match unsafe { c_str_to_str(sequencer_url) } {
        Some(s) => s,
        None => return json_err("null or invalid sequencer_url"),
    };
    let signing_key_hex = match unsafe { c_str_to_str(signing_key_hex) } {
        Some(s) => s,
        None => return json_err("null or invalid signing_key_hex"),
    };

    let signer = match LezSigner::from_raw_key(signing_key_hex) {
        Ok(s) => s,
        Err(e) => return json_err(&e),
    };
    let sequencer = match sequencer_client(sequencer_url) {
        Ok(c) => c,
        Err(e) => return json_err(&e),
    };

    runtime().block_on(async {
        match swap_orchestrator::lez::onboard::ensure_initialized(&sequencer, &signer).await {
            Ok(outcome) => match serde_json::to_string(&outcome) {
                Ok(json) => to_c_string(&json),
                Err(e) => json_err(&format!("failed to serialize outcome: {e}")),
            },
            Err(e) => json_err(&e),
        }
    })
}

/// Ensure a LEZ account is initialized, then claim from the native pinata
/// faucet (150 LEZ/claim, CPU-bound proof-of-work) until its balance reaches
/// `target_lez`. Blocks the calling thread until the target is reached or the
/// funding loop aborts (5 consecutive claim failures) — callers on a UI
/// thread must run this on a worker thread, exactly like
/// `swap_ffi_run_maker`/`swap_ffi_run_taker`. Reports progress (each
/// initialize/claim attempt) via `cb` if non-null; see
/// `src/lez/onboard.rs::FundingProgress` for the event shapes.
///
/// Returns JSON `{"balance":"<final balance, decimal LEZ>"}` on success.
///
/// # Safety
/// `sequencer_url`, `signing_key_hex` and `target_lez` must be valid
/// null-terminated C strings.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swap_ffi_lez_claim_to_target(
    sequencer_url: *const c_char,
    signing_key_hex: *const c_char,
    target_lez: *const c_char,
    cb: ProgressCallback,
    user_data: *mut c_void,
) -> *mut c_char {
    let sequencer_url = match unsafe { c_str_to_str(sequencer_url) } {
        Some(s) => s,
        None => return json_err("null or invalid sequencer_url"),
    };
    let signing_key_hex = match unsafe { c_str_to_str(signing_key_hex) } {
        Some(s) => s,
        None => return json_err("null or invalid signing_key_hex"),
    };
    let target_str = match unsafe { c_str_to_str(target_lez) } {
        Some(s) => s,
        None => return json_err("null or invalid target_lez"),
    };
    let target: u128 = match target_str.trim().parse() {
        Ok(v) => v,
        Err(e) => return json_err(&format!("invalid target_lez: {e}")),
    };

    let signer = match LezSigner::from_raw_key(signing_key_hex) {
        Ok(s) => s,
        Err(e) => return json_err(&e),
    };
    let sequencer = match sequencer_client(sequencer_url) {
        Ok(c) => c,
        Err(e) => return json_err(&e),
    };

    runtime().block_on(async {
        let (progress, progress_forwarder) = forward_funding_progress(cb, user_data);
        let result = match claim_to_target(&sequencer, &signer, target, progress).await {
            Ok(balance) => {
                to_c_string(&serde_json::json!({ "balance": balance.to_string() }).to_string())
            }
            Err(e) => json_err(&e),
        };
        drain_progress_forwarder(progress_forwarder).await;
        result
    })
}

/// Free a string previously returned by any `swap_ffi_*` function.
///
/// # Safety
/// `ptr` must have been returned by a `swap_ffi_*` function and not yet freed.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swap_ffi_free_string(ptr: *mut c_char) {
    if !ptr.is_null() {
        drop(unsafe { CString::from_raw(ptr) });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    /// Guards the checked-in `LEZ_HTLC_PROGRAM_ID_HEX` constant against drift:
    /// if the `lez-htlc` guest (or the pinned LEZ toolchain) changes, its
    /// deterministic risc0 ImageID changes and this test fails loudly with the
    /// new value. Update `src/lez_htlc_program_id.rs` accordingly.
    /// Demo-gated because building `lez_htlc_methods` requires the risc0
    /// toolchain; run via `cargo test -p swap-ffi --features demo`.
    ///
    /// The constant is the DEPLOYED public-testnet program ID, which is also
    /// the guest ImageID under this branch's LEZ v0.2.0 pin — this test is
    /// the permanent drift tripwire.
    #[cfg(feature = "demo")]
    #[test]
    fn checked_in_program_id_matches_guest_image_id() {
        let id: [u32; 8] = lez_htlc_methods::LEZ_HTLC_PROGRAM_ID;
        let bytes: Vec<u8> = id.iter().flat_map(|w| w.to_le_bytes()).collect();
        assert_eq!(
            hex::encode(bytes),
            LEZ_HTLC_PROGRAM_ID_HEX,
            "lez-htlc guest ImageID drifted; update LEZ_HTLC_PROGRAM_ID_HEX \
             in swap-ffi/src/lez_htlc_program_id.rs to the left-hand value"
        );
    }

    #[test]
    fn dotenv_config_json_maps_env_keys_to_ui_config_keys() {
        let path = std::env::temp_dir().join(format!("swap-ffi-env-{}.env", std::process::id()));
        fs::write(
            &path,
            "\
ETH_RPC_URL=ws://127.0.0.1:8545
ETH_PRIVATE_KEY=0x1111111111111111111111111111111111111111111111111111111111111111
ETH_HTLC_ADDRESS=0x2222222222222222222222222222222222222222
LEZ_SEQUENCER_URL=http://127.0.0.1:3040
LEZ_WALLET_HOME=.scaffold/wallet
LEZ_ACCOUNT_ID=7YXq9G
LEZ_HTLC_PROGRAM_ID=aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa
LEZ_AMOUNT=10
ETH_AMOUNT=0.01
ETH_RECIPIENT_ADDRESS=0x3333333333333333333333333333333333333333
LEZ_TAKER_ACCOUNT_ID=8ZZq9G
",
        )
        .unwrap();

        let json = dotenv_config_json(path.to_str().unwrap()).unwrap();
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();

        assert_eq!(value["eth_rpc_url"], "ws://127.0.0.1:8545");
        assert_eq!(value["lez_wallet_home"], ".scaffold/wallet");
        assert_eq!(value["eth_timelock_minutes"], "10");
        assert_eq!(value["lez_timelock_minutes"], "5");
        assert_eq!(value["poll_interval_ms"], "2000");

        let _ = fs::remove_file(path);
    }

    /// A form holding only the two things a balance read actually uses on the
    /// ETH side — an RPC URL and a key — must clear the balance path.
    const BALANCE_ONLY_CONFIG: &str = r#"{
        "eth_rpc_url": "ws://127.0.0.1:8545",
        "eth_private_key": "0x1111111111111111111111111111111111111111111111111111111111111111",
        "eth_htlc_address": "0x2222222222222222222222222222222222222222",
        "lez_sequencer_url": "http://127.0.0.1:3040",
        "lez_signing_key": "",
        "lez_wallet_home": "",
        "lez_account_id": "",
        "lez_htlc_program_id": "",
        "lez_amount": "",
        "eth_amount": "",
        "lez_timelock_minutes": "",
        "eth_timelock_minutes": "",
        "eth_recipient_address": "",
        "lez_taker_account_id": "",
        "poll_interval_ms": ""
    }"#;

    #[test]
    fn balance_only_config_passes_the_balance_path() {
        let cfg = parse_balance_config(BALANCE_ONLY_CONFIG).expect("balance parse");
        let eth = cfg.eth.expect("ETH side is set up");
        assert_eq!(eth.eth_rpc_url, "ws://127.0.0.1:8545");
        assert!(cfg.lez.is_none(), "no LEZ account yet -> LEZ side reported as not set up");
    }

    #[test]
    fn balance_only_config_still_fails_the_swap_path() {
        // The same JSON must NOT be accepted by the strict swap parse: the
        // lenient defaults exist for balances only.
        let err = parse_config(BALANCE_ONLY_CONFIG).unwrap_err();
        assert!(
            err.contains("invalid eth_recipient_address")
                || err.contains("program ID")
                || err.contains("invalid lez_amount"),
            "swap path must reject a balance-only form, got: {err}"
        );
    }

    #[test]
    fn balance_config_with_lez_only_reads_lez_side() {
        let json = BALANCE_ONLY_CONFIG
            .replace(r#""eth_private_key": "0x1111111111111111111111111111111111111111111111111111111111111111""#,
                     r#""eth_private_key": """#)
            .replace(r#""lez_signing_key": """#,
                     r#""lez_signing_key": "3333333333333333333333333333333333333333333333333333333333333333""#);
        let cfg = parse_balance_config(&json).expect("balance parse");
        assert!(cfg.eth.is_none(), "no ETH key -> ETH side not set up");
        let lez = cfg.lez.expect("LEZ side is set up");
        assert!(matches!(lez.lez_auth, LezAuth::RawKey(ref k) if k.len() == 64));
    }

    #[test]
    fn balance_config_with_nothing_set_up_is_rejected() {
        let json = BALANCE_ONLY_CONFIG.replace(
            r#""eth_private_key": "0x1111111111111111111111111111111111111111111111111111111111111111""#,
            r#""eth_private_key": """#,
        );
        let err = parse_balance_config(&json).unwrap_err();
        assert!(err.contains("no chain is set up"), "got: {err}");
    }

    #[test]
    fn missing_swap_keys_are_tolerated_by_serde_but_not_by_the_swap_parse() {
        // Keys absent entirely (not just empty): the GUI always sends every
        // key, but a hand-written config may not.
        let json = r#"{
            "eth_rpc_url": "ws://127.0.0.1:8545",
            "eth_private_key": "0x1111111111111111111111111111111111111111111111111111111111111111",
            "eth_htlc_address": "0x2222222222222222222222222222222222222222",
            "lez_sequencer_url": "http://127.0.0.1:3040"
        }"#;
        assert!(parse_balance_config(json).is_ok());
        assert!(parse_config(json).is_err());
    }

    #[test]
    fn dotenv_config_json_reports_missing_file() {
        let path =
            std::env::temp_dir().join(format!("swap-ffi-missing-{}.env", std::process::id()));
        let err = dotenv_config_json(path.to_str().unwrap()).unwrap_err();
        assert!(err.contains("failed to read env file"));
    }

    /// 32 zero bytes in base58 — a syntactically valid LEZ account ID.
    const TEST_ACCOUNT_B58: &str = "11111111111111111111111111111111";

    fn wallet_auth(home: &str) -> LezAuth {
        LezAuth::Wallet {
            home: std::path::PathBuf::from(home),
            account_id: parse_base58_account_id(TEST_ACCOUNT_B58).unwrap(),
        }
    }

    #[test]
    fn maker_state_file_env_knob_wins_over_auth_mode() {
        for auth in [wallet_auth("/data/wallet"), LezAuth::RawKey("aa".into())] {
            let path = resolve_maker_state_file(Some("/data/journal.json"), &auth).unwrap();
            assert_eq!(path, std::path::PathBuf::from("/data/journal.json"));
        }
    }

    #[test]
    fn maker_state_file_defaults_under_wallet_home() {
        // A blank env value counts as unset, mirroring the empty-string guard.
        for env in [None, Some(""), Some("  ")] {
            let path = resolve_maker_state_file(env, &wallet_auth("/data/wallet")).unwrap();
            assert_eq!(
                path,
                std::path::PathBuf::from("/data/wallet/.maker-state.json")
            );
        }
    }

    #[test]
    fn maker_state_file_refused_for_raw_key_without_env() {
        // Fund-safety: a CWD-relative journal can silently go missing across
        // ui-host launches (unstable CWD), voiding crash recovery — refuse to
        // start rather than default, and name both remedies.
        let err = resolve_maker_state_file(None, &LezAuth::RawKey("aa".into())).unwrap_err();
        assert!(err.contains("MAKER_STATE_FILE"), "missing env remedy: {err}");
        assert!(err.contains("lez_wallet_home"), "missing wallet remedy: {err}");
    }

    // ---------------------------------------------------------------------
    // Onboarding FFI (generate/init/fund) — see swap_ffi_generate_eth_key,
    // swap_ffi_generate_lez_account, swap_ffi_lez_ensure_initialized,
    // swap_ffi_lez_claim_to_target above.
    // ---------------------------------------------------------------------

    /// Free-standing helper: read a C string returned by an FFI call and free
    /// it, mirroring the C++ module's `takeAndFree`.
    fn take_c_string(ptr: *mut c_char) -> String {
        assert!(!ptr.is_null(), "FFI call returned a null pointer");
        let s = unsafe { CStr::from_ptr(ptr) }
            .to_str()
            .expect("FFI result was not valid UTF-8")
            .to_string();
        unsafe { swap_ffi_free_string(ptr) };
        s
    }

    /// Pure, no-network: `swap_ffi_generate_eth_key` produces a well-formed
    /// key/address pair. This is the one every fresh install hits first when
    /// clicking "Generate new key", so it's covered unconditionally (unlike
    /// the testnet-dependent test below).
    #[test]
    fn generate_eth_key_produces_well_formed_pair() {
        let json = take_c_string(swap_ffi_generate_eth_key());
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();
        let private_key = value["private_key"].as_str().unwrap();
        let address = value["address"].as_str().unwrap();

        assert!(private_key.starts_with("0x"));
        assert_eq!(private_key.len(), 66, "expected 0x + 64 hex chars");
        assert!(address.starts_with("0x"));
        assert_eq!(address.len(), 42, "expected 0x + 40 hex chars");

        // The generated key must actually parse back into a signer whose
        // address matches what we returned — otherwise "Generate new key"
        // would silently hand the UI an address it can never sign for.
        let signer: PrivateKeySigner = private_key.parse().unwrap();
        assert_eq!(format!("{}", signer.address()), address);

        // Two calls must not collide (astronomically unlikely, but this is
        // the whole point of "random").
        let json2 = take_c_string(swap_ffi_generate_eth_key());
        let value2: serde_json::Value = serde_json::from_str(&json2).unwrap();
        assert_ne!(value["private_key"], value2["private_key"]);
    }

    /// Pure, no-network: `swap_ffi_generate_lez_account` produces a
    /// well-formed signing key + account ID, and the pair is internally
    /// consistent (the returned account_id is really derived from the
    /// returned signing_key) — this is what `swap_ffi_lez_ensure_initialized`
    /// and `swap_ffi_lez_claim_to_target` will be given by the UI, so a
    /// mismatch here would silently onboard the wrong account.
    #[test]
    fn generate_lez_account_produces_well_formed_and_consistent_pair() {
        let json = take_c_string(swap_ffi_generate_lez_account());
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();
        let signing_key = value["signing_key"].as_str().unwrap();
        let account_id = value["account_id"].as_str().unwrap();

        assert_eq!(signing_key.len(), 64, "expected 64 hex chars, no 0x prefix");
        assert!(!account_id.is_empty());

        let signer = LezSigner::from_raw_key(signing_key).unwrap();
        assert_eq!(account_id_to_base58(&signer.account_id), account_id);
    }

    /// LOAD-BEARING EVIDENCE: exercises the full new onboarding FFI surface
    /// end to end against the REAL public LEZ testnet
    /// (`https://testnet.lez.logos.co`, same endpoint the Config tab defaults
    /// to) — generate a LEZ account, initialize it on-chain, and land at
    /// least one real pinata claim, entirely through the `swap_ffi_*` C ABI
    /// (not the underlying `src/lez/onboard.rs` functions directly), proving
    /// the FFI plumbing (marshaling, progress callback, JSON shapes) and not
    /// just the Rust logic beneath it.
    ///
    /// Ignored by default: needs network egress to the public testnet and
    /// the pinata claim is CPU-bound proof-of-work (difficulty 3), so this is
    /// slow (real chain block times, real PoW solve — often well over a
    /// minute). Run explicitly:
    ///   cargo test -p swap-ffi --lib -- --ignored onboarding_ffi_against_public_testnet
    #[test]
    #[ignore = "hits the real public LEZ testnet + does a CPU-bound PoW claim; run explicitly, see doc comment"]
    fn onboarding_ffi_against_public_testnet() {
        use std::sync::Mutex;

        // 1) Generate an ETH key through the FFI surface (no network).
        let eth_json = take_c_string(swap_ffi_generate_eth_key());
        let eth: serde_json::Value = serde_json::from_str(&eth_json).unwrap();
        println!("[onboarding_ffi] generated ETH address: {}", eth["address"]);

        // 2) Generate a LEZ account through the FFI surface (no network).
        let lez_json = take_c_string(swap_ffi_generate_lez_account());
        let lez: serde_json::Value = serde_json::from_str(&lez_json).unwrap();
        let signing_key = lez["signing_key"].as_str().unwrap().to_string();
        let account_id = lez["account_id"].as_str().unwrap().to_string();
        println!("[onboarding_ffi] generated LEZ account: {account_id}");

        let sequencer_url = CString::new("https://testnet.lez.logos.co").unwrap();
        let signing_key_c = CString::new(signing_key).unwrap();

        // 3) Initialize it on-chain — the sequencer silently drops claims
        // against a never-initialized account, so this MUST succeed before
        // step 4 can mean anything.
        let init_json = take_c_string(unsafe {
            swap_ffi_lez_ensure_initialized(sequencer_url.as_ptr(), signing_key_c.as_ptr())
        });
        println!("[onboarding_ffi] init result: {init_json}");
        assert!(
            !init_json.contains("\"error\""),
            "ensure_initialized failed: {init_json}"
        );

        // 4) Fund to 150 LEZ (exactly one pinata claim — see the funding
        // target rationale on the C++ SwapImpl::startLezFundingJob doc
        // comment), capturing every progress event through the C callback.
        let captured: Box<Mutex<Vec<String>>> = Box::new(Mutex::new(Vec::new()));
        let captured_ptr = Box::into_raw(captured);

        extern "C" fn capture_progress(json: *const c_char, user_data: *mut c_void) {
            let text = unsafe { CStr::from_ptr(json) }
                .to_str()
                .unwrap_or_default()
                .to_string();
            let mutex = unsafe { &*(user_data as *const Mutex<Vec<String>>) };
            mutex.lock().unwrap().push(text);
        }

        let target = CString::new("150").unwrap();
        let fund_json = take_c_string(unsafe {
            swap_ffi_lez_claim_to_target(
                sequencer_url.as_ptr(),
                signing_key_c.as_ptr(),
                target.as_ptr(),
                Some(capture_progress),
                captured_ptr as *mut c_void,
            )
        });

        // Reclaim ownership so the Box is dropped (and to read the events).
        let events = unsafe { Box::from_raw(captured_ptr) }.into_inner().unwrap();
        for event in &events {
            println!("[onboarding_ffi][progress] {event}");
        }
        println!("[onboarding_ffi] final funding result: {fund_json}");

        assert!(
            !fund_json.contains("\"error\""),
            "claim_to_target failed: {fund_json}"
        );
        let result: serde_json::Value = serde_json::from_str(&fund_json).unwrap();
        let balance: u128 = result["balance"].as_str().unwrap().parse().unwrap();
        assert!(balance >= 150, "expected balance >= 150 LEZ, got {balance}");
        assert!(
            events.iter().any(|e| e.contains("\"Claimed\"")),
            "expected at least one Claimed progress event with a real tx hash, got: {events:?}"
        );

        println!(
            "[onboarding_ffi] SUCCESS: account {account_id} initialized and funded to {balance} LEZ on testnet.lez.logos.co"
        );
    }
}
