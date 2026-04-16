//! Embedded `libwaku` node lifecycle.
//!
//! Wraps `waku-bindings` to spawn a node, register the event callback
//! before `start()` (the only state where `set_event_callback` is
//! allowed), and dial bootstrap peers. The event callback runs on a
//! Nim thread — we hand messages off via a non-blocking unbounded
//! mpsc to keep tokio-side code unblocked.
//!
//! See `delivery-dogfooding.md` entries #4 (Nim-thread callback) and
//! #5 (callback only on `Initialized` state).

use std::net::TcpListener;
use std::path::PathBuf;

use multiaddr::Multiaddr;
use secp256k1::{rand::thread_rng, SecretKey};
use tokio::sync::mpsc::{self, UnboundedReceiver, UnboundedSender};
use tracing::{debug, warn};

use waku_bindings::{
    waku_new, LibwakuResponse, Running, WakuEvent, WakuMessage, WakuNodeConfig, WakuNodeHandle,
};

use crate::error::{Result, SwapError};
use crate::messaging::topics::PUBSUB_TOPIC_STR;

/// Configuration for spawning an embedded messaging node.
#[derive(Debug, Clone)]
pub struct MessagingNodeConfig {
    /// libp2p TCP listen port. Use `0` for OS-assigned (recommended for
    /// ephemeral nodes — tests, demo, CLI — to avoid port collisions
    /// since `WakuNodeHandle` has no `Drop` impl and panics leak the
    /// bound port). See dogfooding entry #3.
    pub listen_port: u16,

    /// Path to persist the secp256k1 node key (libp2p peer identity).
    /// `None` means generate a fresh key each spawn (ephemeral identity).
    /// `Some(path)` loads if present, generates and writes if not.
    pub node_key_path: Option<PathBuf>,

    /// Multiaddr strings to dial after the node has started. Parsed
    /// internally so callers don't need to depend on the `multiaddr` crate.
    pub bootstrap_peers: Vec<String>,
}

impl MessagingNodeConfig {
    /// Ephemeral config — random port, ephemeral key, no bootstrap.
    /// Useful for the second node in the demo (which dials the first).
    pub fn ephemeral() -> Self {
        Self {
            listen_port: 0,
            node_key_path: None,
            bootstrap_peers: vec![],
        }
    }
}

/// Bundled node + inbound message receiver. The receiver is fed by the
/// libwaku event callback; consumers drain it via the `MessagingClient`
/// mailbox.
pub struct NodeBundle {
    pub node: WakuNodeHandle<Running>,
    pub inbound: UnboundedReceiver<WakuMessage>,
}

/// Spawn a node, register the event callback, start it, and dial any
/// configured bootstrap peers.
pub async fn spawn_node(cfg: MessagingNodeConfig) -> Result<NodeBundle> {
    let node_key = match &cfg.node_key_path {
        Some(path) => Some(load_or_generate_key(path)?),
        None => Some(SecretKey::new(&mut thread_rng())),
    };

    // libwaku's Nim config parser rejects `tcp_port: 0` ("Port must be
    // 1-65535") despite the bindings' doc saying 0 = random. So we pick
    // a free port ourselves via a transient listener bind.
    // See delivery-dogfooding.md.
    let actual_port = if cfg.listen_port == 0 {
        pick_free_port()?
    } else {
        cfg.listen_port
    };

    let waku_cfg = WakuNodeConfig {
        tcp_port: Some(actual_port as usize),
        node_key,
        cluster_id: Some(0),
        shards: vec![0],
        relay: Some(true),
        relay_topics: vec![PUBSUB_TOPIC_STR.to_string()],
        keep_alive: Some(true),
        ..Default::default()
    };

    let node = waku_new(Some(waku_cfg))
        .await
        .map_err(|e| SwapError::Messaging(format!("waku_new failed: {e}")))?;

    let (tx, rx) = mpsc::unbounded_channel::<WakuMessage>();
    register_callback(&node, tx)?;

    let node = node
        .start()
        .await
        .map_err(|e| SwapError::Messaging(format!("waku start failed: {e}")))?;

    for peer in &cfg.bootstrap_peers {
        let addr: Multiaddr = peer
            .parse()
            .map_err(|e| SwapError::Messaging(format!("invalid bootstrap multiaddr {peer}: {e}")))?;
        debug!(%addr, "dialing bootstrap peer");
        node.connect(&addr, None)
            .await
            .map_err(|e| SwapError::Messaging(format!("connect to {addr} failed: {e}")))?;
    }

    Ok(NodeBundle { node, inbound: rx })
}

fn register_callback(
    node: &WakuNodeHandle<waku_bindings::Initialized>,
    tx: UnboundedSender<WakuMessage>,
) -> Result<()> {
    node.set_event_callback(move |response| {
        // Runs on a Nim thread — must NOT block or .await.
        if let LibwakuResponse::Success(Some(json)) = response {
            match serde_json::from_str::<WakuEvent>(&json) {
                Ok(WakuEvent::WakuMessage(evt)) => {
                    // Non-blocking send; receiver is unbounded so this never errors
                    // unless the receiver is dropped (which means client shutdown).
                    let _ = tx.send(evt.waku_message);
                }
                Ok(WakuEvent::ConnectionChange(_)) | Ok(WakuEvent::RelayTopicHealthChange(_)) => {
                    // Ignored for now.
                }
                Ok(WakuEvent::Unrecognized(v)) => {
                    warn!(?v, "unrecognized waku event");
                }
                Ok(_) => {}
                Err(e) => warn!(%e, "failed to parse waku event"),
            }
        }
    })
    .map_err(|e| SwapError::Messaging(format!("set_event_callback failed: {e}")))?;
    Ok(())
}

/// Bind a transient TCP listener on `127.0.0.1:0`, read the OS-assigned
/// port, then drop the listener. Small race window before libwaku binds
/// it, but acceptable for tests/demo where collisions are rare.
fn pick_free_port() -> Result<u16> {
    let listener = TcpListener::bind("127.0.0.1:0")
        .map_err(|e| SwapError::Messaging(format!("pick_free_port bind: {e}")))?;
    let port = listener
        .local_addr()
        .map_err(|e| SwapError::Messaging(format!("pick_free_port local_addr: {e}")))?
        .port();
    Ok(port)
}

fn load_or_generate_key(path: &PathBuf) -> Result<SecretKey> {
    if path.exists() {
        let hex_str = std::fs::read_to_string(path)
            .map_err(|e| SwapError::Messaging(format!("read node_key {path:?}: {e}")))?;
        let bytes = hex::decode(hex_str.trim())
            .map_err(|e| SwapError::Messaging(format!("decode node_key {path:?}: {e}")))?;
        SecretKey::from_slice(&bytes)
            .map_err(|e| SwapError::Messaging(format!("parse node_key {path:?}: {e}")))
    } else {
        let key = SecretKey::new(&mut thread_rng());
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| SwapError::Messaging(format!("mkdir {parent:?}: {e}")))?;
        }
        std::fs::write(path, hex::encode(key.secret_bytes()))
            .map_err(|e| SwapError::Messaging(format!("write node_key {path:?}: {e}")))?;
        Ok(key)
    }
}
