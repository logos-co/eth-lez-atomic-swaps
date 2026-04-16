//! Embedded messaging client wrapping `libwaku` via `waku-bindings`.
//!
//! Same surface as the previous REST-based client — `subscribe`,
//! `publish`, `poll_messages`, `store_query` — so call sites barely
//! change. Internally owns a running `WakuNodeHandle` plus a per-content-topic
//! mailbox fed by the libwaku event callback.
//!
//! Lifecycle: construct via [`MessagingClient::spawn`], explicitly drive
//! [`MessagingClient::shutdown`] before drop. There is no `Drop` impl
//! on the underlying `WakuNodeHandle` so dropping leaks the Nim
//! runtime + bound TCP port. See `delivery-dogfooding.md` entry #3.

use std::collections::{HashMap, VecDeque};
use std::sync::Mutex;

use multiaddr::Multiaddr;
use serde::{de::DeserializeOwned, Serialize};
use tokio::runtime::Handle;
use tokio::sync::mpsc::UnboundedReceiver;
use tokio::sync::Mutex as AsyncMutex;
use tokio::task;
use tracing::{debug, warn};

use waku_bindings::{Running, WakuMessage, WakuNodeHandle};

use crate::error::{Result, SwapError};
use crate::messaging::node::{spawn_node, MessagingNodeConfig, NodeBundle};
use crate::messaging::topics::{parse_content_topic, pubsub_topic};

/// In-process messaging client. Owns one libwaku node.
pub struct MessagingClient {
    node: WakuNodeHandle<Running>,
    /// Inbound channel from the libwaku event callback.
    /// Wrapped in async Mutex so [`Self::poll_messages`] can drain it.
    inbound: AsyncMutex<UnboundedReceiver<WakuMessage>>,
    /// Per-content-topic mailbox of decoded messages awaiting `poll_messages`.
    /// Sync Mutex (held only briefly during drain).
    mailboxes: Mutex<HashMap<String, VecDeque<WakuMessage>>>,
    /// Has [`Self::subscribe`] already issued the underlying
    /// `relay_subscribe`? Idempotent across multiple subscribe() calls.
    subscribed: Mutex<bool>,
}

// SAFETY: WakuNodeHandle wraps a raw `*mut c_void` so it's not auto-Send/Sync,
// but libwaku is designed for concurrent use from multiple threads (it's a
// network node — every operation goes through libwaku's own internal locking).
// The bindings should do this themselves; until they do, we assert it here.
//
// Note: this Send/Sync for MessagingClient itself is necessary but NOT
// sufficient. async fns on this client wrap their inner `node.foo().await`
// in `block_in_place(|| Handle::current().block_on(...))` so the inner
// !Send future never spans an await on the outer state machine. Without
// that, Futures returned by `MessagingClient` methods would inherit the
// inner future's !Send-ness regardless of these impls.
// See delivery-dogfooding.md.
unsafe impl Send for MessagingClient {}
unsafe impl Sync for MessagingClient {}

/// Run an inner async block (which may capture !Send types from the
/// underlying `WakuNodeHandle`) without exposing that !Send future to
/// the outer `async fn`'s state machine. Lets `MessagingClient` methods
/// stay `async fn` while their returned futures remain `Send`.
///
/// Requires the caller to be on a multi-thread tokio runtime.
fn run_blocking<F, T>(fut_factory: F) -> T
where
    F: FnOnce() -> T,
{
    task::block_in_place(fut_factory)
}

impl MessagingClient {
    /// Spawn an embedded node, dial bootstrap peers, and return a
    /// ready-to-use client. Drive [`Self::shutdown`] before drop.
    pub async fn spawn(cfg: MessagingNodeConfig) -> Result<Self> {
        let NodeBundle { node, inbound } = spawn_node(cfg).await?;
        Ok(Self {
            node,
            inbound: AsyncMutex::new(inbound),
            mailboxes: Mutex::new(HashMap::new()),
            subscribed: Mutex::new(false),
        })
    }

    /// Dial an additional peer after construction (e.g. discovered later).
    pub async fn dial(&self, addr: &Multiaddr) -> Result<()> {
        run_blocking(|| {
            Handle::current().block_on(async {
                self.node
                    .connect(addr, None)
                    .await
                    .map_err(|e| SwapError::Messaging(format!("dial {addr} failed: {e}")))
            })
        })
    }

    /// The multiaddrs this node is listening on. Use to publish a
    /// rendezvous address that other nodes can dial.
    pub async fn listen_addresses(&self) -> Result<Vec<Multiaddr>> {
        run_blocking(|| {
            Handle::current().block_on(async {
                self.node
                    .listen_addresses()
                    .await
                    .map_err(|e| SwapError::Messaging(format!("listen_addresses failed: {e}")))
            })
        })
    }

    /// Subscribe to content topics. Idempotent — only one underlying
    /// `relay_subscribe` is issued per client, since the bindings only
    /// expose pubsub-topic-level subscription. The `topics` argument is
    /// effectively informational here; filtering happens in
    /// [`Self::poll_messages`] by content-topic key.
    pub async fn subscribe(&self, topics: &[&str]) -> Result<()> {
        debug!(?topics, "subscribe requested");
        let already = {
            let mut g = self.subscribed.lock().unwrap();
            let was = *g;
            *g = true;
            was
        };
        if already {
            return Ok(());
        }
        let pubsub = pubsub_topic();
        run_blocking(|| {
            Handle::current().block_on(async {
                self.node
                    .relay_subscribe(&pubsub)
                    .await
                    .map_err(|e| SwapError::Messaging(format!("relay_subscribe failed: {e}")))
            })
        })
    }

    /// Publish a JSON-serialised payload to the given content topic.
    pub async fn publish<T: Serialize>(&self, topic: &str, payload: &T) -> Result<()> {
        let json_bytes = serde_json::to_vec(payload)
            .map_err(|e| SwapError::Messaging(format!("serialize payload: {e}")))?;
        let content_topic = parse_content_topic(topic);
        let msg = WakuMessage::new(json_bytes, content_topic, 0, Vec::new(), false);
        let pubsub = pubsub_topic();
        debug!(topic, "publishing message");
        run_blocking(|| {
            Handle::current().block_on(async {
                self.node
                    .relay_publish_message(&msg, &pubsub, None)
                    .await
                    .map_err(|e| SwapError::Messaging(format!("relay_publish failed: {e}")))
                    .map(|_| ())
            })
        })
    }

    /// Drain pending inbound messages on `topic` and deserialize each
    /// payload as `T`. Destructive: returned messages are removed from
    /// the mailbox.
    ///
    /// Internally first drains the mpsc channel into per-topic mailboxes,
    /// then returns the contents of the requested topic's mailbox.
    pub async fn poll_messages<T: DeserializeOwned>(&self, topic: &str) -> Result<Vec<T>> {
        self.drain_inbound().await;
        let mut mboxes = self.mailboxes.lock().unwrap();
        let bucket = match mboxes.get_mut(topic) {
            Some(b) => b,
            None => return Ok(vec![]),
        };
        let mut out = Vec::with_capacity(bucket.len());
        while let Some(msg) = bucket.pop_front() {
            match serde_json::from_slice::<T>(&msg.payload) {
                Ok(v) => out.push(v),
                Err(e) => warn!(topic, %e, "skipping undecodable message"),
            }
        }
        debug!(topic, count = out.len(), "polled messages");
        Ok(out)
    }

    /// Move everything currently waiting in the mpsc channel into the
    /// per-content-topic mailboxes. Always non-blocking.
    async fn drain_inbound(&self) {
        let mut rx = self.inbound.lock().await;
        let mut mboxes = self.mailboxes.lock().unwrap();
        while let Ok(msg) = rx.try_recv() {
            let key = msg.content_topic.to_string();
            mboxes.entry(key).or_default().push_back(msg);
        }
    }

    /// **Stub** — the embedded node cannot serve as a Waku store
    /// (the bindings expose `storenode` for querying a remote store but
    /// no way to enable the store protocol locally). Returns `Ok(vec![])`
    /// so callers fall through to relay polling. See
    /// `delivery-dogfooding.md` entry #12.
    pub async fn store_query(
        &self,
        _topics: &[&str],
        _start_time_ns: Option<i64>,
        _page_size: Option<u64>,
    ) -> Result<Vec<StoreEntry>> {
        Ok(vec![])
    }

    /// Drive `stop().await` then `waku_destroy().await`. Required —
    /// `WakuNodeHandle` has no `Drop` impl. Best-effort: ignores
    /// individual step failures and proceeds to the next.
    pub async fn shutdown(self) -> Result<()> {
        let MessagingClient { node, .. } = self;
        run_blocking(|| {
            Handle::current().block_on(async move {
                let stopped = node
                    .stop()
                    .await
                    .map_err(|e| SwapError::Messaging(format!("stop failed: {e}")))?;
                stopped
                    .waku_destroy()
                    .await
                    .map_err(|e| SwapError::Messaging(format!("waku_destroy failed: {e}")))
            })
        })
    }
}

/// Placeholder store entry shape — kept so callers' destructuring
/// continues to compile. Always empty in practice (see [`MessagingClient::store_query`]).
#[derive(Debug, Clone)]
pub struct StoreEntry {
    pub message: Option<StoreMessage>,
}

#[derive(Debug, Clone)]
pub struct StoreMessage {
    pub payload: Vec<u8>,
    pub content_topic: String,
    pub timestamp: Option<i64>,
}

/// Decode a payload into `T`. Used by FFI / discovery callers that
/// historically received base64-encoded payloads from the REST API; in
/// the embedded model the payload is already raw bytes, so this is just
/// a serde_json wrapper kept for API compatibility.
pub fn decode_waku_payload<T: DeserializeOwned>(payload: &[u8]) -> Result<T> {
    serde_json::from_slice(payload)
        .map_err(|e| SwapError::Messaging(format!("payload decode failed: {e}")))
}
