# logos-delivery dogfooding log

We're integrating
[`waku-bindings`](https://github.com/logos-messaging/logos-delivery-rust-bindings)
into the LEZ ↔ ETH atomic-swaps PoC (this repo). This file tracks
papercuts, missing features, surprising behaviors, and workarounds we
hit so the `logos-delivery` / `waku-bindings` team can prioritize.

**Pinned to:** `waku-bindings` rev
[`9504040`](https://github.com/logos-messaging/logos-delivery-rust-bindings/tree/9504040)
(master HEAD as of 2026-04-16) → wraps libwaku from
[`logos-messaging-nim`](https://github.com/logos-messaging/logos-delivery)
via `waku-sys`'s git submodule.

## Format

Each entry: **what we hit**, **where (file:line in bindings)**,
**workaround**, **suggested fix**.

Anyone touching `src/messaging/` or `swap-ffi/src/lib.rs` is expected
to append here when they hit something new. New issues at the bottom.

---

## 1. No autosharding helper

**What:** `relay_subscribe` / `relay_publish_message` require an
explicit `&PubsubTopic`. The C API (`libwaku.h`) only defines
`waku_relay_publish(... pubSubTopic, ...)` — no autosharding overload.
Apps coming from the nwaku REST `/relay/v1/auto/...` endpoints (which
auto-route by content topic) have to compute pubsub topics themselves.

**Where:** `waku-bindings/src/node/mod.rs:118-136`,
`waku-bindings/src/node/relay.rs:43-71`,
`logos-messaging-nim/library/libwaku.h`.

**Workaround:** Hardcode `cluster_id=0`, `shards=[0]`,
`pubsub_topic="/waku/2/rs/0/0"` across all nodes (see
`src/messaging/topics.rs`). Content topics (`/atomic-swaps/1/offers/json`)
are kept as-is and routed application-side.

**Suggested fix:** Either expose RFC-51 autosharding hash as a helper
(`PubsubTopic::auto_for(content_topic, cluster_id)`), or add a
`relay_publish_auto(content_topic, message)` convenience that derives
the pubsub topic.

## 2. Stale doc comment on `relay_publish_message`

**What:** Docstring says "The pubsub_topic parameter is optional and
if not specified it will be derived from the contentTopic", but the
parameter is `&PubsubTopic` (not `Option<&PubsubTopic>`). Cost us 30
minutes thinking we'd missed an overload.

**Where:** `waku-bindings/src/node/mod.rs:128-130`.

**Suggested fix:** Either update the doc, or implement what the doc
claims (see #1).

## 3. No `Drop` impl on `WakuNodeHandle`

**What:** Cleanup requires explicit `stop().await` then
`waku_destroy().await`. Panics leak the underlying Nim runtime, libp2p
host stays alive, TCP port stays bound — the next test run that tries
to bind the same port fails.

**Where:** No `impl Drop` anywhere in `waku-bindings/src/node/`. See
also `src/node/context.rs:38-40` (`reset_ptr` does nothing async).

**Workaround:**
- `MessagingClient::shutdown()` drives both async destructors
  (`src/messaging/client.rs`).
- Every CLI/demo/test path calls `shutdown().await` (or
  `Arc::try_unwrap(...).shutdown().await`).
- Use a free-port helper for ephemeral nodes (see #12) instead of fixed
  ports, so leaks on panic don't cascade into the next run.

**Suggested fix:** A `Drop` impl that does best-effort
`stop()`+`waku_destroy()`. Even a sync best-effort cleanup that warns
on failure is better than silent leak.

## 4. Event callback runs on a Nim thread, not a tokio task

**What:** `set_event_callback` invokes the closure on a libwaku-spawned
Nim thread. You can't `.await` or use most tokio primitives inside.

**Where:** `waku-bindings/src/macros.rs:5-28` (the FFI trampoline),
`waku-bindings/src/node/mod.rs:76-81`.

**Workaround:** Use `tokio::sync::mpsc::UnboundedSender::send` (it's
non-blocking and runtime-agnostic) inside the callback to hand off to
async code (see `src/messaging/node.rs::register_callback`).

**Suggested fix:** Document this prominently. Optionally provide a
helper that wraps the callback into a `Stream<Item = WakuEvent>` so
users don't have to wire the channel themselves.

## 5. `set_event_callback` is only on `Initialized` state

**What:** Must register before `start().await`. Easy to mis-shape an
API where callback registration happens after a "ready" handle is
returned.

**Where:** `src/node/mod.rs:81-87` (impl block on `Initialized` only).

**Workaround:** Our `spawn_node` enforces ordering: `waku_new` →
register callback → `start` → return handle (`src/messaging/node.rs`).

**Suggested fix:** Either allow `set_event_callback` on `Running` too
(if libwaku supports late registration), or add a constructor variant
`waku_new_with_events(cfg, cb)` that fuses the two steps.

## 6. `dns_discovery_url`, `storenode`, `log_level` are `Option<&'static str>`

**What:** These config fields take `&'static str`, which is awkward
for runtime values from env vars / TOML / FFI JSON. We don't use them
yet but will need ENRTREE for production.

**Where:** `waku-bindings/src/node/config.rs:37, 44, 58`.

**Workaround:** `Box::leak(s.into_boxed_str())` (when we get there).

**Suggested fix:** Change to `Option<String>` or
`Option<Cow<'static, str>>`.

## 7. `store_query`'s `peer_addr` panics on invalid multiaddr

**What:** Internal `peer_addr.parse::<Multiaddr>().expect(...)` —
caller passes `&str`, gets a panic instead of a `Result`.

**Where:** `waku-bindings/src/node/store.rs:161`.

**Workaround:** N/A — we don't currently call `store_query` against a
real peer (see #8) so this hasn't bitten us, but it's a landmine.

**Suggested fix:** Take `&Multiaddr` typed parameter instead of `&str`,
or return `Err` from `store_query`.

## 8. No way to enable the Waku store protocol server-side

**What:** `WakuNodeConfig` exposes `storenode: Option<&'static str>`
which is the multiaddr of a store node to QUERY, but there's no
`store: bool` / `store_message_db_url: String` equivalent of the nwaku
CLI flags. Embedded nodes can only consume store, not serve it.

**Where:** `waku-bindings/src/node/config.rs:36-38` — no `store`
fields. Compare to nwaku CLI: `--store=true
--store-message-db-url=sqlite:///data/store.db`.

**Impact for us:** The previous Docker setup had `nwaku1` with
`--store=true`, so a taker that came online AFTER the maker published
could query store and find the offer. With embedded nodes we lose
that. `MessagingClient::store_query` is now a no-op stub returning
empty (`src/messaging/client.rs`). For the demo we work around it by
having the taker subscribe BEFORE the maker publishes; for the
auto-accept maker loop we rely on periodic republishing; for the
manual `swap-cli maker` flow there's a regression where takers who
join after the publish can't find the offer.

**Suggested fix:** Expose store-server enablement in `WakuNodeConfig`
mirroring the nwaku CLI flags (`store`, `store_message_db_url`,
`store_message_retention_policy`). Without this, "embedded node" is
strictly less capable than "node-in-Docker".

## 9. Not on crates.io at current version

**What:** `Cargo.toml` declares `1.0.0`; latest crates.io publish is
`waku-bindings 0.6.0` (Feb 2024). Downstream apps must depend via
`git = "...", rev = "..."` and pin manually.

**Suggested fix:** Cut a `1.0.0` (or `0.7.0`) crates.io release.

## 10. Build needs `git + GNU make + C toolchain` (Nim auto-bootstrapped)

**What:** `waku-sys/build.rs` runs `git submodule init/update` and
`make libwaku STATIC=1` inside `vendor/`. First build is 5–10 minutes
on Apple Silicon. Bootstraps Nim itself via `nimbus-build-system`,
which is great — no manual Nim install required.

**Where:** `waku-sys/build.rs`.

**Suggested fix:** Document explicitly in the README which host tools
are required. Consider publishing pre-built `libwaku.a` artifacts for
common platforms to skip the Nim compile.

## 11. macOS aarch64 needs specific linker flags

**What:** Without `-framework CoreFoundation -framework Security
-framework CoreServices -lresolv`, link fails on Apple Silicon
(libwaku pulls in Go-sourced crypto deps).

**Where:** `logos-delivery-rust-bindings/.cargo/config.toml`.

**Workaround:** Same flags copied into our workspace
`.cargo/config.toml`. Without this, downstream consumers must
rediscover the requirement empirically.

**Suggested fix:** Either bake the flags into `waku-sys/build.rs` via
`println!("cargo:rustc-link-arg=...")`, or document them prominently
as a required downstream config.

## 12. `tcp_port: 0` is rejected by libwaku despite docs saying "random"

**What:** `WakuNodeConfig.tcp_port` doc says `Use 0 for random` (`src/node/config.rs:17`).
Passing `Some(0)` actually fails at runtime:
```
[Chronicles] CREATE_NODE failed
exception in createWaku when parsing configuration. exc: The supplied port
must be an integer value in the range 1-65535. string that could not be
parsed: 0. expected type: Port
```

**Where:** Surfaces from libwaku Nim config parsing
(`logos-messaging-nim`); the bindings just forward the value via JSON.

**Workaround:** `pick_free_port()` helper in
`src/messaging/node.rs` — bind a transient `TcpListener` on
`127.0.0.1:0`, read the OS-assigned port, drop the listener, hand
that port to libwaku. Small race window but acceptable for
tests/demo. Combined with #3, this prevents port leaks from
poisoning subsequent runs.

**Suggested fix:** Either accept `0` in libwaku and bind to an
OS-assigned port internally (the natural OS-level meaning), or fix
the doc to remove the "random" claim.

## 13. `WakuNodeHandle` is `!Send` and `!Sync` — async fns return non-`Send` Futures

**What:** `WakuNodeContext` holds a raw `*mut c_void`, so the auto-derived
`Send`/`Sync` fail. Calling any `node.relay_publish_message(...).await`
from inside `tokio::spawn` fails to compile because the returned Future
captures `&WakuNodeHandle` across the await point.

**Where:** `waku-bindings/src/node/context.rs:10-13` — raw pointer field.

**Workaround:**
- `unsafe impl Send + Sync for MessagingClient` (we know libwaku is
  thread-safe — every operation goes through libwaku's own internal
  locking).
- That alone isn't enough: the inner `&WakuNodeHandle` captured by
  awaited inner futures still pollutes auto-trait inference. So every
  `MessagingClient` async fn wraps the inner await in
  `tokio::task::block_in_place(|| Handle::current().block_on(...))`.
  This keeps the !Send future local to the closure (sync from the
  outer state machine's perspective), so the outer `async fn`'s
  Future is `Send` and works inside `tokio::spawn`.
- Documented at `src/messaging/client.rs::run_blocking`.

**Suggested fix:** Add `unsafe impl Send for WakuNodeContext {}` and
`unsafe impl Sync for WakuNodeContext {}` upstream — libwaku is
designed for concurrent multi-thread use. Without this, downstream
consumers can't use the bindings naturally with
multi-thread tokio runtimes; we had to invent the `block_in_place`
trampoline.

## 14. `rln` transitive dep is a dep-graph wedge

**What:** `waku-bindings` depends on `rln 0.3.4`, which strict-pins
many of its deps (`ark-serialize "=0.4.1"`, `thiserror "=1.0.39"`,
`color-eyre "=0.6.2"`, `wasmer "=2.3.0"`, ...) — these conflict with
modern downstream trees. Specifically: our LEZ deps require
`ark-serialize ^0.4.2` (via `logos-blockchain-groth16`), so the
resolver fails immediately.

The `rln` Rust crate is pulled in only for symbol availability —
`waku-bindings/src/lib.rs` does `use rln;` with `#[allow(unused)]`
because `libwaku`'s Nim code statically references `rln` C symbols
(`new`, `flush`, `atomic_operation`, `generate_rln_proof`,
`verify_with_roots`, `poseidon_hash`, …) at link time even when
RLN isn't enabled at runtime.

**Where:** `waku-bindings/Cargo.toml:34`, `waku-bindings/src/lib.rs:14-16`.

**Workaround:** Local `[patch.crates-io]` to a vendored stub at
`vendor/rln-patched/` that re-exports the FFI symbol surface (`new`,
`set_tree`, `delete_leaf`, `set_leaf`, …, `poseidon_hash`) — every
function returns `false`/`0` so the linker is happy but no real RLN
work is done. Safe because we never enable `rln_relay` in
`WakuNodeConfig`. See `vendor/rln-patched/src/lib.rs` for the symbol
list.

**Suggested fix:** Make `rln` a feature-gated dep
(`features = ["rln"]`, default-off), or upgrade `waku-bindings` to
the latest `rln 0.7.0+` (which uses non-strict `^0.5.0` ark-* pins
and would coexist cleanly with modern dep trees).

## 15. `multiaddr 0.17` → `multihash 0.17` → `core2 0.4.0` (yanked everywhere)

**What:** `waku-bindings` exposes `pub use multiaddr::Multiaddr` from
`multiaddr 0.17`, whose only-valid `multihash 0.17` requires
`core2 ^0.4.0`. **Every published version of `core2` is yanked**, so
fresh resolves on a new machine fail with
`failed to select a version for the requirement core2 = "^0.4.0":
version 0.4.0 is yanked`.

**Where:** `waku-bindings/Cargo.toml:22`.

**Workaround:** Vendored `core2 0.4.0` source under
`vendor/core2-vendored/` and added a `[patch.crates-io]` entry so the
yanked version is bypassed.

**Suggested fix:** Bump `multiaddr` to `0.18.x` (uses a `multihash`
that doesn't depend on `core2`). Will be a small breaking change to
consumers' `Multiaddr` type imports but unlocks fresh builds.

## 16. `[patch.crates-io]` surprises: highest-version-from-crates-io wins

**What:** Two cargo-patch traps we hit while trying to patch `rln`:

1. If your patched version is `0.3.4` (matching upstream) but
   crates.io has `0.3.5`, cargo prefers `0.3.5` from crates.io
   (because `^0.3.4` allows higher) and your patch is silently
   ignored. The fix is either to bump your patched version above the
   highest crates.io release (we used `0.3.999`) OR to anchor
   resolution by adding the crate as a direct workspace dep with
   `=0.3.4`.
2. The patched crate's own deps (e.g. rln's `=` pinned `ark-ec`,
   `thiserror`, etc.) must ALSO satisfy the workspace tree. If they
   don't, cargo silently falls back to the un-patched crate from
   crates.io and you see the original conflict in the error message
   (which is misleading — looks like the patch isn't loaded at all).

**Where:** N/A — cargo behavior, but worth a one-paragraph note in
`waku-bindings`' README "if you need to patch out rln".

**Suggested fix:** Document the `[patch.crates-io] rln = …` recipe
upstream. Even better, eliminate the need for it (#14).

## 17. swap-ffi separate-package context loses workspace-level `[patch]`

**What:** Our `swap-ffi/` lives at `path = ".."` of the orchestrator
crate. Without an explicit `[workspace]` section, `cargo build` from
`swap-ffi/` re-resolves independently and ignores the root's
`[patch.crates-io]` — so it re-hits the rln + core2 wedges.

**Where:** Our `Cargo.toml` (root) and `swap-ffi/Cargo.toml`.

**Workaround:** Added `[workspace] members = [".", "swap-ffi"]` so
both crates share the patch table. Worth flagging because it's a
cargo gotcha that downstream consumers will hit independently of
waku-bindings — but waku-bindings docs could surface it.

**Suggested fix:** N/A on the bindings side; just a note that
multi-crate downstream consumers need a real workspace.

---
(Append new entries below as we hit them.)
