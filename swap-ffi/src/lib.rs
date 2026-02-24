use std::ffi::{CStr, CString, c_char, c_void};
use std::sync::OnceLock;
use std::time::Duration;

use serde::Deserialize;
use tokio::runtime::Runtime;

use sha2::{Digest, Sha256};

use swap_orchestrator::{
    config::{SwapConfig, parse_account_id, parse_program_id},
    eth::client::EthClient,
    lez::client::LezClient,
    messaging::client::{MessagingClient, decode_waku_payload},
    messaging::types::{SwapOffer, OFFERS_TOPIC},
    swap::{
        maker::run_maker,
        progress::SwapProgress,
        refund::{now_unix, refund_eth, refund_lez},
        taker::run_taker,
        types::SwapOutcome,
    },
};

fn runtime() -> &'static Runtime {
    static RT: OnceLock<Runtime> = OnceLock::new();
    RT.get_or_init(|| Runtime::new().expect("failed to create tokio runtime"))
}

/// Callback invoked on each progress event (called from a worker thread).
pub type ProgressCallback = Option<unsafe extern "C" fn(*const c_char, *mut c_void)>;

// ---------------------------------------------------------------------------
// JSON helpers
// ---------------------------------------------------------------------------

fn json_ok() -> *mut c_char {
    to_c_string(r#"{"ok":true}"#)
}

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

// ---------------------------------------------------------------------------
// Config parsing (mirrors ConfigArgs::into_swap_config at src/cli/mod.rs:93)
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct FfiConfig {
    eth_rpc_url: String,
    eth_private_key: String,
    eth_htlc_address: String,
    lez_sequencer_url: String,
    lez_signing_key: String,
    lez_htlc_program_id: String,
    lez_amount: String,
    eth_amount: String,
    lez_timelock_minutes: String,
    eth_timelock_minutes: String,
    eth_recipient_address: String,
    lez_taker_account_id: String,
    #[serde(default = "default_poll")]
    poll_interval_ms: String,
    #[serde(default)]
    nwaku_url: Option<String>,
}

fn default_poll() -> String {
    "2000".into()
}

fn parse_config(json_str: &str) -> Result<SwapConfig, String> {
    let c: FfiConfig = serde_json::from_str(json_str).map_err(|e| format!("bad config JSON: {e}"))?;

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
    let lez_taker_account_id =
        parse_account_id(&c.lez_taker_account_id).map_err(|e| e.to_string())?;

    let lez_amount: u128 = c.lez_amount.parse().map_err(|e| format!("invalid lez_amount: {e}"))?;
    let eth_amount: u128 = c.eth_amount.parse().map_err(|e| format!("invalid eth_amount: {e}"))?;
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

    Ok(SwapConfig {
        eth_rpc_url: c.eth_rpc_url,
        eth_private_key: c.eth_private_key,
        eth_htlc_address,
        lez_sequencer_url: c.lez_sequencer_url,
        lez_signing_key: c.lez_signing_key,
        lez_htlc_program_id,
        lez_amount,
        eth_amount,
        lez_timelock: now + lez_timelock_minutes * 60,
        eth_timelock: now + eth_timelock_minutes * 60,
        eth_recipient_address,
        lez_taker_account_id,
        poll_interval: Duration::from_millis(poll_interval_ms),
        nwaku_url: c.nwaku_url,
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

fn forward_progress(cb: ProgressCallback, user_data: *mut c_void) -> Option<swap_orchestrator::swap::progress::ProgressSender> {
    let cb = cb?;
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<SwapProgress>();

    // user_data is thread-safe (opaque pointer managed by the C++ caller).
    let ud = user_data as usize;
    tokio::spawn(async move {
        while let Some(progress) = rx.recv().await {
            if let Ok(json) = serde_json::to_string(&progress) {
                if let Ok(c_str) = CString::new(json) {
                    unsafe { cb(c_str.as_ptr(), ud as *mut c_void) };
                }
            }
        }
    });

    Some(tx)
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
    let result = if path.is_null() {
        dotenvy::dotenv().map(|_| ())
    } else {
        let path_str = match unsafe { c_str_to_str(path) } {
            Some(s) => s,
            None => return json_err("invalid UTF-8 path"),
        };
        dotenvy::from_filename(path_str).map(|_| ())
    };

    match result {
        Ok(()) => json_ok(),
        Err(e) if e.not_found() => json_ok(),
        Err(e) => json_err(&format!("failed to load .env: {e}")),
    }
}

/// Run the maker flow. Blocks until the swap completes or times out.
///
/// # Safety
/// `config_json` must be a valid null-terminated JSON C string.
/// `preimage_hex` may be null (maker generates internally) or a 64-char hex string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swap_ffi_run_maker(
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

    // Parse optional preimage override.
    let override_preimage: Option<[u8; 32]> = if preimage_hex.is_null() {
        None
    } else {
        match unsafe { c_str_to_str(preimage_hex) } {
            Some(s) if s.is_empty() => None,
            Some(s) => {
                let s = s.strip_prefix("0x").unwrap_or(s);
                match hex::decode(s) {
                    Ok(b) if b.len() == 32 => {
                        let mut arr = [0u8; 32];
                        arr.copy_from_slice(&b);
                        Some(arr)
                    }
                    Ok(_) => return json_err("preimage must be 32 bytes (64 hex chars)"),
                    Err(e) => return json_err(&format!("invalid preimage hex: {e}")),
                }
            }
            None => None,
        }
    };

    runtime().block_on(async {
        let progress = forward_progress(cb, user_data);

        let eth_client = match EthClient::new(&config).await {
            Ok(c) => c,
            Err(e) => return json_err(&e.to_string()),
        };
        let lez_client = match LezClient::new(&config) {
            Ok(c) => c,
            Err(e) => return json_err(&e.to_string()),
        };

        match run_maker(&config, &eth_client, &lez_client, override_preimage, progress).await {
            Ok(ref outcome) => {
                let hashlock = match outcome {
                    SwapOutcome::Completed { preimage, .. } => {
                        use sha2::{Digest, Sha256};
                        Sha256::digest(preimage).into()
                    }
                    _ => [0u8; 32],
                };
                to_c_string(&outcome_to_json(outcome, &hashlock))
            }
            Err(e) => json_err(&e.to_string()),
        }
    })
}

/// Run the taker flow. Blocks until the swap completes or times out.
///
/// # Safety
/// `config_json` and `hashlock_hex` must be valid null-terminated C strings.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swap_ffi_run_taker(
    config_json: *const c_char,
    hashlock_hex: *const c_char,
    cb: ProgressCallback,
    user_data: *mut c_void,
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
        let progress = forward_progress(cb, user_data);

        let eth_client = match EthClient::new(&config).await {
            Ok(c) => c,
            Err(e) => return json_err(&e.to_string()),
        };
        let lez_client = match LezClient::new(&config) {
            Ok(c) => c,
            Err(e) => return json_err(&e.to_string()),
        };

        // When messaging is enabled, wait for LEZ escrow to appear before starting
        // the taker flow (maker may still be locking). Matches the CLI demo pattern.
        if config.nwaku_url.is_some() {
            let deadline = tokio::time::Instant::now() + Duration::from_secs(300);
            loop {
                match lez_client.get_escrow(&hashlock_bytes).await {
                    Ok(Some(escrow)) if escrow.amount >= config.lez_amount => break,
                    _ => {}
                }
                if tokio::time::Instant::now() >= deadline {
                    return json_err("taker: LEZ escrow did not appear within 5 minutes");
                }
                tokio::time::sleep(Duration::from_secs(2)).await;
            }
            // Brief pause so maker's ETH event watcher is ready.
            tokio::time::sleep(Duration::from_secs(3)).await;
        }

        match run_taker(&config, &eth_client, &lez_client, hashlock_bytes, progress).await {
            Ok(ref outcome) => to_c_string(&outcome_to_json(outcome, &hashlock_bytes)),
            Err(e) => json_err(&e.to_string()),
        }
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
            Ok(tx_hash) => {
                to_c_string(&serde_json::json!({ "ok": true, "tx_hash": format!("{tx_hash}") }).to_string())
            }
            Err(e) => json_err(&e.to_string()),
        }
    })
}

/// Publish a swap offer via nwaku messaging. Returns JSON with hashlock and preimage.
///
/// # Safety
/// `config_json` and `nwaku_url` must be valid null-terminated C strings.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swap_ffi_publish_offer(
    config_json: *const c_char,
    nwaku_url: *const c_char,
) -> *mut c_char {
    let json_str = match unsafe { c_str_to_str(config_json) } {
        Some(s) => s,
        None => return json_err("null or invalid config_json"),
    };
    let nwaku = match unsafe { c_str_to_str(nwaku_url) } {
        Some(s) => s,
        None => return json_err("null or invalid nwaku_url"),
    };

    let config = match parse_config(json_str) {
        Ok(c) => c,
        Err(e) => return json_err(&e),
    };

    runtime().block_on(async {
        // Generate preimage + hashlock.
        let preimage: [u8; 32] = rand::random();
        let hashlock: [u8; 32] = Sha256::digest(preimage).into();

        let lez_client = match LezClient::new(&config) {
            Ok(c) => c,
            Err(e) => return json_err(&e.to_string()),
        };

        let program_id_bytes: Vec<u8> = config
            .lez_htlc_program_id
            .iter()
            .flat_map(|w| w.to_le_bytes())
            .collect();

        let offer = SwapOffer {
            hashlock: hex::encode(hashlock),
            lez_amount: config.lez_amount,
            eth_amount: config.eth_amount,
            maker_eth_address: format!("{}", config.eth_recipient_address),
            maker_lez_account: hex::encode(lez_client.account_id().value()),
            lez_timelock: config.lez_timelock,
            eth_timelock: config.eth_timelock,
            lez_htlc_program_id: hex::encode(&program_id_bytes),
            eth_htlc_address: format!("{}", config.eth_htlc_address),
        };

        let messaging = MessagingClient::new(nwaku);
        if let Err(e) = messaging.subscribe(&[OFFERS_TOPIC]).await {
            return json_err(&format!("failed to subscribe: {e}"));
        }
        if let Err(e) = messaging.publish(OFFERS_TOPIC, &offer).await {
            return json_err(&format!("failed to publish offer: {e}"));
        }

        let result = serde_json::json!({
            "ok": true,
            "hashlock": hex::encode(hashlock),
            "preimage": hex::encode(preimage),
        });
        to_c_string(&result.to_string())
    })
}

/// Fetch available swap offers from nwaku messaging. Returns JSON array of offers.
///
/// # Safety
/// `nwaku_url` must be a valid null-terminated C string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swap_ffi_fetch_offers(
    nwaku_url: *const c_char,
) -> *mut c_char {
    let nwaku = match unsafe { c_str_to_str(nwaku_url) } {
        Some(s) => s,
        None => return json_err("null or invalid nwaku_url"),
    };

    runtime().block_on(async {
        let messaging = MessagingClient::new(nwaku);
        if let Err(e) = messaging.subscribe(&[OFFERS_TOPIC]).await {
            return json_err(&format!("failed to subscribe: {e}"));
        }

        let mut offers: Vec<serde_json::Value> = Vec::new();
        let mut seen_hashlocks = std::collections::HashSet::new();

        // Query store for last 30 minutes.
        let now_ns = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos() as i64;
        let now_ms = now_ns / 1_000_000;
        let start_ns = now_ns - 30 * 60 * 1_000_000_000;

        if let Ok(entries) = messaging.store_query(&[OFFERS_TOPIC], Some(start_ns), Some(50)).await {
            for entry in &entries {
                if let Some(ref msg) = entry.message {
                    if let Ok(offer) = decode_waku_payload::<SwapOffer>(&msg.payload) {
                        if seen_hashlocks.insert(offer.hashlock.clone()) {
                            let ts_ms = msg.timestamp.map(|t| t / 1_000_000).unwrap_or(now_ms);
                            let mut val = serde_json::to_value(&offer).unwrap();
                            val.as_object_mut().unwrap().insert("timestamp_ms".to_string(), serde_json::json!(ts_ms));
                            offers.push(val);
                        }
                    }
                }
            }
        }

        // Also poll relay cache.
        let relay_offers: Vec<SwapOffer> = messaging.poll_messages(OFFERS_TOPIC).await.unwrap_or_default();
        for offer in relay_offers {
            if seen_hashlocks.insert(offer.hashlock.clone()) {
                let mut val = serde_json::to_value(&offer).unwrap();
                val.as_object_mut().unwrap().insert("timestamp_ms".to_string(), serde_json::json!(now_ms));
                offers.push(val);
            }
        }

        let result = serde_json::json!({ "offers": offers });
        to_c_string(&result.to_string())
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
