# logos-delivery-module v0.1.1 dogfooding log

This file tracks only the findings from integrating
[`logos-delivery-module`](https://github.com/logos-co/logos-delivery-module/tree/v0.1.1)
into this app during this session.

## Integration scope

- **Module:** `logos-co/logos-delivery-module` v0.1.1
- **Resolved revision:** `0c346c0c2ab2404c11a62cd6c385e806e8465434`
- **Boundary:** `swap-module` owns Delivery-backed offer publish/fetch.
- **UI dependency rule:** `swap-ui` continues to depend only on `swap`.
- **M1 scope:** offer publish/fetch on `/atomic-swaps/1/offers/json` only.
- **Deferred:** per-swap maker/taker coordination on `/atomic-swaps/1/swap-<hashlock>/json`.

## Docs checklist followed

- Added metadata dependency named exactly `delivery_module`.
- Added flake input named exactly `delivery_module`.
- Pinned the flake input to `github:logos-co/logos-delivery-module/v0.1.1`.
- Kept the universal public header free of Qt/Logos runtime types.
- Injected `LogosAPI*` from generated Qt glue during Nix `preConfigure`; no generated files were edited by hand.
- Followed Delivery lifecycle: `createNode -> start -> subscribe -> send -> unsubscribe -> stop`.
- Consumed `messageReceived` event payload from `data[2]` as base64.

## Positive highlights

- The Delivery demo's `LogosModules(api)` pattern made dependency-module calls easy to discover and adapt.
- `delivery_module_plugin.h` documents the event contract clearly enough to wire `messageReceived`, `messageError`, and `connectionStateChanged` without reading liblogosdelivery internals.
- The module README clearly explains the Nix build outputs and synchronous method surface.
- Runtime dogfooding confirmed the documented `send` / `messageReceived` path works across two Basecamp instances when both use the same Nix-built LGX artifacts and isolated per-instance state.

## Issues and classifications

### Upstream #18: multi-instance port configuration

**Classification:** upstream/module issue; locally mitigated, not fixed.

The adapter accepts `delivery.portsShift` or top-level `portsShift` in `messagingInit` config so two local nodes can be tested without hardcoded port collisions. Runtime Basecamp testing reproduced the need for this: with default Delivery ports, one instance could connect while another returned `start failed:` with no useful error text. The UI now follows the Delivery demo pattern by assigning a per-process random `portsShift`, but this is only a local workaround. It is not equivalent to consumer-neutral environment-variable port overrides and should not be treated as an upstream fix.

**Disposition:** track upstream; do not claim resolved from this repo.

### Upstream #26: `messageReceived` timestamp shape

**Classification:** upstream docs/API consistency issue; tracked, not re-filed from this repo yet.

The documented/demo contract gives `messageReceived` a nanoseconds-since-epoch timestamp while other events expose local ISO-8601 timestamps. The adapter avoids depending on that inconsistent timestamp and stamps received offers locally with `timestamp_ms`.

**Disposition:** mention if filing docs/API feedback; no local blocker for M1.

### Upstream #27: watch item

**Classification:** watch-only until reproduced in this repo.

No runtime evidence from this integration has reproduced #27 yet.

**Disposition:** do not confirm or file from this integration unless M1 runtime verification reproduces it.

### Upstream/doc packet: first-class peer count/status API

**Classification:** upstream module/API and docs ergonomics issue; locally mitigated, not fixed.

The Delivery docs/demo expose connection readiness through `connectionStateChanged`, but peer count is not available as a first-class status method or event payload. The demo currently polls `delivery_module.getNodeInfo("Metrics")` and parses the Prometheus `libp2p_peers` gauge. A trial implementation that called `getNodeInfo("Metrics")` from the embedded swap module triggered repeated capability-token churn in Basecamp (`LogosAPI not available` / empty `requestModule` results), so this app now avoids showing the old Waku peer-count placeholder and falls back to Delivery connection state instead. Downstream apps should not need to parse metrics text or add fragile cross-module polling for a common UI status indicator.

**Suggested fix:** add a documented Delivery API/status field for peer count, for example `getPeerCount()` or a structured `getStatus()` response containing `connectionStatus`, `peerCount`, and optionally node peer ID. The doc packet should mention this in the app integration journey so downstream UIs do not infer peer status from metrics.

**Disposition:** capture for the doc packet; do not file yet. Local use of `getNodeInfo("Metrics")` is only a workaround.

### Downstream: old Waku bootstrap gate blocked Delivery defaults

**Classification:** downstream app issue; fixed locally.

`swap-ui` previously required `waku_bootstrap_multiaddr` before calling `messagingInit`. Delivery can bootstrap from its own preset/config, so the UI now allows an empty legacy bootstrap field while still validating non-empty multiaddrs.

**Disposition:** fixed in this repo.

### Downstream/Basecamp: module proxy logs include sensitive method arguments

**Classification:** downstream tooling/logging and dogfooding hygiene issue; tracked, not fixed here.

Runtime Basecamp logs print full remote method arguments for calls such as `fetchBalances` and `publishOffer`. During local testing that included `.env` values such as private keys, account identifiers, RPC URLs, and public offer payloads. This is useful for debugging but makes raw log snippets unsafe to paste into upstream issues or docs without redaction.

**Suggested fix:** provide a redaction mode or argument filtering for Basecamp/module proxy logs, especially for known config keys like `eth_private_key`, `lez_signing_key`, wallet paths, RPC URLs, and raw payloads.

**Disposition:** do not file yet; redact logs manually before any upstream/downstream issue filing.

### Downstream: maker loop retries sequencer failures too aggressively

**Classification:** downstream app issue; locally mitigated.

When the LEZ sequencer is unreachable, the live maker loop emits `AutoAcceptSwapFailed` for every balance-check failure. Runtime testing showed this can flood the Completed Swaps list with failures in seconds. The loop now waits for the configured poll interval before retrying client-init and balance-check failures.

**Disposition:** fixed locally; still worth validating whether long-running live maker should eventually surface a terminal infrastructure error instead of retrying forever.

### Repo reproducibility: ignored backend lockfile

**Classification:** repo issue; fixed locally.

`swap-module/flake.lock` was ignored, which made the Delivery dependency graph less reproducible. The ignore rule was removed and the lockfile now records the resolved `delivery_module` revision.

**Disposition:** fixed in this repo.

### Upstream packaging/cache: cold downstream build took about an hour

**Classification:** upstream packaging / binary-cache issue; not acceptable as normal downstream app friction.

Adding `logos-delivery-module` to this app caused a cold local build of heavy transitive dependencies before the app could build, especially `zerokit-0.9.0-vendor-staging`, `zerokit-0.9.0`, `liblogosdelivery-dev`, and `logos-delivery_module-module`. The machine already had the Logos Cachix substituter configured, but these exact derivations still built locally for this platform/input graph.

A downstream app developer should not have to spend about an hour compiling Delivery's native dependency stack just to try a documented module integration. If cache misses are expected for supported systems, the docs should call that out explicitly; preferably, CI should publish substitutes for the module and its heavy transitive dependencies.

**Suggested fix:** publish/verify binary cache artifacts for supported systems for `zerokit`, `liblogosdelivery-dev`, `logos-delivery_module-module`, and the module headers/lib outputs; add a cold-consumer build check that confirms a downstream app can integrate Delivery mostly from cache.

**Disposition:** capture for later upstream packaging/cache feedback; do not file yet, and do not normalize as expected app-developer experience.

## Verification status

- Static boundary proof: complete.
- Adapter LSP diagnostics: passing.
- Adapter no-Qt stub compile with `-Wall -Wextra -Werror`: passing.
- `git diff --check`: passing.
- Flake evaluation and dry-runs: passing.
- Full package build: passing after the cold Delivery dependency graph completed.
- Unit tests: passing.
- Same-artifact runtime smoke: `QT_QPA_PLATFORM=offscreen nix run . -- --help` passes.
- Cross-node Delivery receipt proof: passing for M1 offer discovery. Two isolated Basecamp instances loaded the rebuilt LGX packages, maker published on `/atomic-swaps/1/offers/json`, taker logged `messageReceived`, and taker `fetchOffers` returned the offer.
- Per-swap Delivery coordination proof: still pending; current flow falls back to the existing on-chain/polling swap path after offer discovery.

## Evidence required before upstream/downstream filing

- Exact command run and result.
- Redacted config used for `createNode`.
- Whether `onInit` fired in the same Nix-built artifact.
- Delivery `start` / `subscribe` result.
- Cross-node receipt evidence via `messageReceived`, not just `send` success. Captured for M1 offer discovery in Basecamp runtime logs; redact before sharing because Basecamp logs include full method args.
- Cold-build timing and whether each heavy derivation came from cache or built locally.
- For #18, whether local multi-instance verification needed `portsShift`.
- For #26, the raw event timestamp shape if reproduced.

## Runtime proof notes

- Two Basecamp instances were launched with isolated `LOGOS_DATA_DIR`, HOME, XDG, runtime, and wallet paths.
- `delivery_module`, `swap`, and `swap_ui` LGX packages were installed into both isolated Basecamp roots.
- Delivery offer discovery used the public Delivery preset (`mode: Core`, `preset: logos.dev`) with per-instance `portsShift`; it did not use the local Waku rendezvous node from `make infra`.
- `make infra` is still required for the existing ETH/LEZ swap flow after offer discovery. During this run it regenerated `.env` and `.env.taker` with Anvil on `ws://localhost:60531` and LEZ sequencer on `http://127.0.0.1:3040/`.
- M1 succeeded: maker offer publish produced Delivery send/propagation logs, taker received the offer via `messageReceived`, and taker displayed the discovered offer.
- M2 remains open: per-swap maker/taker coordination should move to `/atomic-swaps/1/swap-<hashlock>/json` instead of relying only on the existing on-chain/polling flow after discovery.

## Redaction reminder

Before filing upstream, downstream, or repo issues, redact private keys, node keys, account data, private multiaddrs, RPC URLs, and raw private payloads.
