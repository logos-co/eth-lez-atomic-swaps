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
    lez::client::LezClient,
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
    lez_htlc_program_id: String,
    lez_amount: String,
    eth_amount: String,
    lez_timelock_minutes: String,
    eth_timelock_minutes: String,
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

    let config = match parse_config(json_str) {
        Ok(c) => c,
        Err(e) => return json_err(&e),
    };

    // Derive ETH address from private key.
    let eth_signer: std::result::Result<PrivateKeySigner, _> = config.eth_private_key.parse();
    let eth_address = eth_signer.as_ref().ok().map(|s| format!("{}", s.address()));

    // Derive LEZ account ID.
    let lez_client_result = LezClient::new(&config);
    let lez_account = lez_client_result
        .as_ref()
        .ok()
        .map(|c| account_id_to_base58(&c.account_id()));

    runtime().block_on(async {
        // Fetch ETH balance.
        let eth_fut = async {
            let signer = eth_signer.map_err(|e| format!("invalid ETH private key: {e}"))?;
            let addr = signer.address();
            let eth_client = EthClient::new(&config).await.map_err(|e| e.to_string())?;
            let balance = eth_client
                .provider()
                .get_balance(addr)
                .await
                .map_err(|e| e.to_string())?;
            Ok::<String, String>(balance.to_string())
        };

        // Fetch LEZ balance.
        let lez_fut = async {
            let client = lez_client_result.as_ref().map_err(|e| e.to_string())?;
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

        // Crash recovery FIRST, mirroring the CLI loop (P1-3): reconcile
        // journaled in-flight swaps (refund / claim / resume) and fully RESOLVE
        // them before accepting any new swap (P1-A), so the 256-block replay
        // watcher can never rematch a still-OPEN lock and double-fund it.
        let resume_handles = reconcile(&base_config, &store, /* json = */ true).await;
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
}
