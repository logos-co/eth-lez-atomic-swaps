use std::ffi::{CStr, CString, c_char, c_void};
use std::sync::{Arc, Mutex, OnceLock};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use serde::Deserialize;
use tokio::runtime::Runtime;

use sha2::{Digest, Sha256};

use alloy::providers::Provider;
use alloy::signers::local::PrivateKeySigner;

use swap_orchestrator::{
    config::{LezAuth, MessagingConfig, SwapConfig, account_id_to_base58, eth_to_wei, parse_base58_account_id, parse_program_id},
    eth::client::EthClient,
    lez::client::LezClient,
    messaging::client::MessagingClient,
    messaging::node::MessagingNodeConfig,
    messaging::topics::OFFERS_TOPIC,
    messaging::types::SwapOffer,
    swap::{
        maker::{run_maker, run_maker_loop, AutoAcceptConfig},
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

/// Process-wide embedded waku client. Initialized once via
/// [`swap_ffi_messaging_init`] and torn down via
/// [`swap_ffi_messaging_shutdown`]. `Arc` so we can hand out cheap
/// clones to spawned tasks while the global retains ownership.
fn messaging_slot() -> &'static Mutex<Option<Arc<MessagingClient>>> {
    static MSG: OnceLock<Mutex<Option<Arc<MessagingClient>>>> = OnceLock::new();
    MSG.get_or_init(|| Mutex::new(None))
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

/// Parse an optional 32-byte hex string from a C pointer.
/// Returns `None` for null pointers or empty strings.
unsafe fn parse_optional_bytes32(ptr: *const c_char, name: &str) -> std::result::Result<Option<[u8; 32]>, *mut c_char> {
    if ptr.is_null() {
        return Ok(None);
    }
    match unsafe { c_str_to_str(ptr) } {
        Some(s) if s.is_empty() => Ok(None),
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
    lez_taker_account_id: String,
    #[serde(default = "default_poll")]
    poll_interval_ms: String,
    /// Multiaddr of the messaging rendezvous node (typically written by
    /// `make infra` into .env). When omitted, messaging is disabled.
    #[serde(default)]
    waku_bootstrap_multiaddr: Option<String>,
    #[serde(default = "default_waku_port")]
    waku_listen_port: u16,
}

fn default_waku_port() -> u16 {
    0
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
        parse_base58_account_id(&c.lez_taker_account_id).map_err(|e| e.to_string())?;

    let lez_amount: u128 = c.lez_amount.parse().map_err(|e| format!("invalid lez_amount: {e}"))?;
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

    Ok(SwapConfig {
        eth_rpc_url: c.eth_rpc_url,
        eth_private_key: c.eth_private_key,
        eth_htlc_address,
        lez_sequencer_url: c.lez_sequencer_url,
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
        messaging: c.waku_bootstrap_multiaddr.map(|bootstrap_multiaddr| MessagingConfig {
            bootstrap_multiaddr,
            listen_port: c.waku_listen_port,
        }),
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
        let progress = forward_progress(cb, user_data);

        let eth_client = match EthClient::new(&config).await {
            Ok(c) => c,
            Err(e) => return json_err(&e.to_string()),
        };
        let lez_client = match LezClient::new(&config) {
            Ok(c) => c,
            Err(e) => return json_err(&e.to_string()),
        };

        match run_maker(&config, &eth_client, &lez_client, hashlock_opt, None, progress).await {
            Ok(ref outcome) => {
                let hashlock = match outcome {
                    SwapOutcome::Completed { preimage, .. } => {
                        Sha256::digest(preimage).into()
                    }
                    _ => hashlock_opt.unwrap_or([0u8; 32]),
                };
                to_c_string(&outcome_to_json(outcome, &hashlock))
            }
            Err(e) => json_err(&e.to_string()),
        }
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
        let progress = forward_progress(cb, user_data);

        let eth_client = match EthClient::new(&config).await {
            Ok(c) => c,
            Err(e) => return json_err(&e.to_string()),
        };
        let lez_client = match LezClient::new(&config) {
            Ok(c) => c,
            Err(e) => return json_err(&e.to_string()),
        };

        match run_taker(&config, &eth_client, &lez_client, override_preimage, progress).await {
            Ok(ref outcome) => {
                let hashlock = match outcome {
                    SwapOutcome::Completed { preimage, .. } => {
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

/// Configuration for [`swap_ffi_messaging_init`].
#[derive(Deserialize)]
struct FfiMessagingConfig {
    /// Multiaddr of the messaging rendezvous node to dial.
    bootstrap_multiaddr: String,
    /// libp2p TCP listen port. `0` = OS-assigned.
    #[serde(default)]
    listen_port: u16,
}

/// Initialize the embedded waku messaging client. Spawns a node, dials
/// the rendezvous peer, and subscribes to the offers topic. Idempotent —
/// calling twice is a no-op (returns ok). Must be called once per
/// process before any of the offer-related FFI functions.
///
/// # Safety
/// `config_json` must be a valid null-terminated JSON C string with
/// shape `{"bootstrap_multiaddr": "...", "listen_port": 0}`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swap_ffi_messaging_init(config_json: *const c_char) -> *mut c_char {
    let json_str = match unsafe { c_str_to_str(config_json) } {
        Some(s) => s,
        None => return json_err("null or invalid config_json"),
    };

    let cfg: FfiMessagingConfig = match serde_json::from_str(json_str) {
        Ok(c) => c,
        Err(e) => return json_err(&format!("bad messaging config JSON: {e}")),
    };

    {
        let guard = messaging_slot().lock().unwrap();
        if guard.is_some() {
            return json_ok();
        }
    }

    runtime().block_on(async {
        let client = match MessagingClient::spawn(MessagingNodeConfig {
            listen_port: cfg.listen_port,
            node_key_path: None,
            bootstrap_peers: vec![cfg.bootstrap_multiaddr.clone()],
        })
        .await
        {
            Ok(c) => c,
            Err(e) => return json_err(&format!("messaging spawn failed: {e}")),
        };
        if let Err(e) = client.subscribe(&[OFFERS_TOPIC]).await {
            return json_err(&format!("subscribe failed: {e}"));
        }
        let mut guard = messaging_slot().lock().unwrap();
        *guard = Some(Arc::new(client));
        json_ok()
    })
}

/// Shut down the embedded waku messaging client (drives stop+destroy).
/// Required because `WakuNodeHandle` has no `Drop` impl. Safe to call
/// even if init was never called.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swap_ffi_messaging_shutdown() -> *mut c_char {
    let client_arc = {
        let mut guard = messaging_slot().lock().unwrap();
        guard.take()
    };
    let Some(arc) = client_arc else {
        return json_ok();
    };
    let Some(client) = Arc::into_inner(arc) else {
        // Outstanding clones still hold the client (e.g. an in-flight
        // operation). Safest: leave the slot empty and let the last
        // clone drop without explicit shutdown. The libwaku node will
        // leak but the process is presumably exiting.
        return json_err("messaging client has outstanding references — shutdown skipped");
    };
    runtime().block_on(async {
        match client.shutdown().await {
            Ok(_) => json_ok(),
            Err(e) => json_err(&format!("shutdown failed: {e}")),
        }
    })
}

/// Publish a standing swap offer via the embedded messaging client.
///
/// In taker-locks-first, the maker publishes an offer without a hashlock.
/// The taker will generate the preimage and hashlock when accepting.
/// Requires [`swap_ffi_messaging_init`] to have been called first.
///
/// # Safety
/// `config_json` must be a valid null-terminated C string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swap_ffi_publish_offer(
    config_json: *const c_char,
) -> *mut c_char {
    let json_str = match unsafe { c_str_to_str(config_json) } {
        Some(s) => s,
        None => return json_err("null or invalid config_json"),
    };

    let config = match parse_config(json_str) {
        Ok(c) => c,
        Err(e) => return json_err(&e),
    };

    let messaging = match messaging_slot().lock().unwrap().clone() {
        Some(m) => m,
        None => return json_err("messaging not initialized — call swap_ffi_messaging_init first"),
    };

    runtime().block_on(async {
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
            hashlock: String::new(), // standing offer — no hashlock yet
            lez_amount: config.lez_amount,
            eth_amount: config.eth_amount,
            maker_eth_address: format!("{}", config.eth_recipient_address),
            maker_lez_account: account_id_to_base58(&lez_client.account_id()),
            lez_timelock: config.lez_timelock,
            eth_timelock: config.eth_timelock,
            lez_htlc_program_id: hex::encode(&program_id_bytes),
            eth_htlc_address: format!("{}", config.eth_htlc_address),
        };

        if let Err(e) = messaging.publish(OFFERS_TOPIC, &offer).await {
            return json_err(&format!("failed to publish offer: {e}"));
        }

        let result = serde_json::json!({
            "ok": true,
        });
        to_c_string(&result.to_string())
    })
}

/// Fetch available swap offers from the embedded messaging client.
/// Returns JSON array of offers seen on the relay since the last call.
/// Requires [`swap_ffi_messaging_init`] to have been called first.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swap_ffi_fetch_offers() -> *mut c_char {
    let messaging = match messaging_slot().lock().unwrap().clone() {
        Some(m) => m,
        None => return json_err("messaging not initialized — call swap_ffi_messaging_init first"),
    };

    runtime().block_on(async {
        let mut offers: Vec<serde_json::Value> = Vec::new();
        let mut seen = std::collections::HashSet::new();
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0);

        // Drain the relay mailbox. (Store query is a no-op stub — see
        // delivery-dogfooding.md #12 — so we only have what's been
        // delivered via gossipsub since this client started.)
        let relay_msgs: Vec<SwapOffer> = messaging.poll_messages(OFFERS_TOPIC).await.unwrap_or_default();
        for offer in relay_msgs {
            let key = serde_json::to_string(&offer).unwrap_or_default();
            if !seen.insert(key) {
                continue;
            }
            let mut val = serde_json::to_value(&offer).unwrap();
            val.as_object_mut().unwrap().insert("timestamp_ms".to_string(), serde_json::json!(now_ms));
            offers.push(val);
        }

        let result = serde_json::json!({ "offers": offers });
        to_c_string(&result.to_string())
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
    let lez_account = lez_client_result.as_ref().ok().map(|c| account_id_to_base58(&c.account_id()));

    runtime().block_on(async {
        // Fetch ETH balance.
        let eth_fut = async {
            let signer = eth_signer.map_err(|e| format!("invalid ETH private key: {e}"))?;
            let addr = signer.address();
            let eth_client = EthClient::new(&config).await.map_err(|e| e.to_string())?;
            let balance = eth_client.provider().get_balance(addr).await.map_err(|e| e.to_string())?;
            Ok::<String, String>(balance.to_string())
        };

        // Fetch LEZ balance.
        let lez_fut = async {
            let client = lez_client_result.as_ref().map_err(|e| e.to_string())?;
            let balance = client.get_balance(&client.account_id()).await.map_err(|e| e.to_string())?;
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

/// Run the maker in an auto-accept loop. Blocks until cancelled, out of funds,
/// or an unrecoverable error. Returns JSON: `{ "completed": N, "failed": M }`.
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

    runtime().block_on(async {
        let progress = forward_progress(cb, user_data);
        let result = run_maker_loop(&base_config, &auto_config, &MAKER_LOOP_CANCEL, progress).await;
        let json = serde_json::json!({
            "completed": result.total_completed,
            "failed": result.total_failed,
        });
        to_c_string(&json.to_string())
    })
}

/// Signal the maker auto-accept loop to stop after the current iteration.
#[unsafe(no_mangle)]
pub extern "C" fn swap_ffi_stop_maker_loop() {
    MAKER_LOOP_CANCEL.store(true, Ordering::SeqCst);
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
