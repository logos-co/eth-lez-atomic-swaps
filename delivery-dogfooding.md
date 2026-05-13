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

## Issues and classifications

### Upstream #18: multi-instance port configuration

**Classification:** upstream/module issue; locally mitigated, not fixed.

The adapter accepts `delivery.portsShift` or top-level `portsShift` in `messagingInit` config so two local nodes can be tested without hardcoded port collisions. This is only a local workaround. It is not equivalent to consumer-neutral environment-variable port overrides and should not be treated as an upstream fix.

**Disposition:** track upstream; do not claim resolved from this repo.

### Upstream #26: `messageReceived` timestamp shape

**Classification:** upstream docs/API consistency issue; tracked, not re-filed from this repo yet.

The documented/demo contract gives `messageReceived` a nanoseconds-since-epoch timestamp while other events expose local ISO-8601 timestamps. The adapter avoids depending on that inconsistent timestamp and stamps received offers locally with `timestamp_ms`.

**Disposition:** mention if filing docs/API feedback; no local blocker for M1.

### Upstream #27: watch item

**Classification:** watch-only until reproduced in this repo.

No runtime evidence from this integration has reproduced #27 yet.

**Disposition:** do not confirm or file from this integration unless M1 runtime verification reproduces it.

### Downstream: old Waku bootstrap gate blocked Delivery defaults

**Classification:** downstream app issue; fixed locally.

`swap-ui` previously required `waku_bootstrap_multiaddr` before calling `messagingInit`. Delivery can bootstrap from its own preset/config, so the UI now allows an empty legacy bootstrap field while still validating non-empty multiaddrs.

**Disposition:** fixed in this repo.

### Repo reproducibility: ignored backend lockfile

**Classification:** repo issue; fixed locally.

`swap-module/flake.lock` was ignored, which made the Delivery dependency graph less reproducible. The ignore rule was removed and the lockfile now records the resolved `delivery_module` revision.

**Disposition:** fixed in this repo.

### Upstream packaging/cache: cold downstream build took about an hour

**Classification:** upstream packaging / binary-cache issue; not acceptable as normal downstream app friction.

Adding `logos-delivery-module` to this app caused a cold local build of heavy transitive dependencies before the app could build, especially `zerokit-0.9.0-vendor-staging`, `zerokit-0.9.0`, `liblogosdelivery-dev`, and `logos-delivery_module-module`. The machine already had the Logos Cachix substituter configured, but these exact derivations still built locally for this platform/input graph.

A downstream app developer should not have to spend about an hour compiling Delivery's native dependency stack just to try a documented module integration. If cache misses are expected for supported systems, the docs should call that out explicitly; preferably, CI should publish substitutes for the module and its heavy transitive dependencies.

**Suggested fix:** publish/verify binary cache artifacts for supported systems for `zerokit`, `liblogosdelivery-dev`, `logos-delivery_module-module`, and the module headers/lib outputs; add a cold-consumer build check that confirms a downstream app can integrate Delivery mostly from cache.

**Disposition:** file upstream packaging/cache feedback; do not normalize as expected app-developer experience.

## Verification status

- Static boundary proof: complete.
- Adapter LSP diagnostics: passing.
- Adapter no-Qt stub compile with `-Wall -Wextra -Werror`: passing.
- `git diff --check`: passing.
- Flake evaluation and dry-runs: passing.
- Full package build: passing after the cold Delivery dependency graph completed.
- Unit tests: passing.
- Same-artifact runtime smoke: `QT_QPA_PLATFORM=offscreen nix run . -- --help` passes.
- Cross-node Delivery receipt proof: still pending; needs two-node `messageReceived` evidence, not just a successful build or `send` result.

## Evidence required before upstream/downstream filing

- Exact command run and result.
- Redacted config used for `createNode`.
- Whether `onInit` fired in the same Nix-built artifact.
- Delivery `start` / `subscribe` result.
- Cross-node receipt evidence via `messageReceived`, not just `send` success.
- Cold-build timing and whether each heavy derivation came from cache or built locally.
- For #18, whether local multi-instance verification needed `portsShift`.
- For #26, the raw event timestamp shape if reproduced.

## Redaction reminder

Before filing upstream, downstream, or repo issues, redact private keys, node keys, account data, private multiaddrs, RPC URLs, and raw private payloads.
