# logos-delivery-module v0.1.1 dogfooding log

This file tracks only the findings from integrating
[`logos-delivery-module`](https://github.com/logos-co/logos-delivery-module/tree/v0.1.1)
into this app during this session.

## Integration scope

- **Module:** `logos-co/logos-delivery-module` v0.1.1
- **Resolved revision:** `0c346c0c2ab2404c11a62cd6c385e806e8465434`
- **Boundary:** `swap-module` owns Delivery-backed offer publish/fetch
  **and** per-swap coordination publish/subscribe.
- **UI dependency rule:** `swap-ui` continues to depend only on `swap`.
- **M1 scope:** offer publish/fetch on `/atomic-swaps/1/offers/json`.
- **M2 scope:** per-swap maker/taker coordination on
  `/atomic-swaps/1/swap-<hashlock>/json`. Layered on top of the existing
  on-chain ETH/LEZ flow — the Rust orchestrator still drives state via
  on-chain watchers; the Delivery channel surfaces the SwapAccept ack
  off-chain.

## Docs checklist followed

- Added metadata dependency named exactly `delivery_module`.
- Added flake input named exactly `delivery_module`.
- Pinned the flake input to `github:logos-co/logos-delivery-module/v0.1.1`.
- Kept the universal public header free of Qt/Logos runtime types.
- Injected `LogosAPI*` from generated Qt glue during Nix `preConfigure`; no generated files were edited by hand.
- Followed Delivery lifecycle: `createNode -> start -> subscribe -> send -> unsubscribe -> stop`.
- Consumed `messageReceived` event payload from `data[2]` as base64.
- M2: routed `messageReceived` by `data[1]` (contentTopic) so the same
  callback handles both `/atomic-swaps/1/offers/json` and per-swap
  `/atomic-swaps/1/swap-<hashlock>/json` topics without duplicating the
  base64-decode/JSON-parse boilerplate.
- M2: validated that received per-swap payloads carry a hashlock that
  matches the topic they arrived on before caching them, to prevent a
  malicious or bug-induced sender from polluting another swap's bucket.
- Removed the old Rust `waku-bindings` path entirely: `src/messaging`, the
  `waku` Cargo feature, Waku-only FFI offer helpers, Waku integration tests,
  and the vendored `rln`/`core2` patches are gone. Basecamp offer discovery
  and per-swap coordination now go through `logos-delivery-module` only.

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

**Classification:** downstream app issue; fixed locally and then removed.

`swap-ui` previously required `waku_bootstrap_multiaddr` before calling `messagingInit`. Delivery can bootstrap from its own preset/config, so the UI no longer requires or exposes a bootstrap multiaddr for the normal Basecamp flow.

The later cleanup removed the Rust-side Waku configuration and embedded Waku
FFI functions entirely. The remaining `messagingInit`/`publishOffer`/
`fetchOffers` module API is backed by the Delivery adapter, not by
`waku-bindings`.

**Disposition:** fixed in this repo.

### Downstream/Basecamp: module proxy logs include sensitive method arguments

**Classification:** downstream tooling/logging and dogfooding hygiene issue; tracked, not fixed here.

Runtime Basecamp logs print full remote method arguments for calls such as `fetchBalances` and `publishOffer`. During local testing that included `.env` values such as private keys, account identifiers, RPC URLs, and public offer payloads. This is useful for debugging but makes raw log snippets unsafe to paste into upstream issues or docs without redaction.

The issue also affects seemingly harmless startup polish. Auto-populating wallet balances after `loadEnvFile` could not safely reuse the normal `fetchBalances(configJson)` UI path, because the generated module proxy would log the full secret-bearing config as a method argument. The local mitigation is a path-based `fetchBalancesFromEnv(path)` module method: only the env-file path crosses the Basecamp IPC/logging boundary, while the core module loads the secret config internally and immediately calls the Rust FFI balance fetch.

**Suggested fix:** provide a redaction mode or argument filtering for Basecamp/module proxy logs, especially for known config keys like `eth_private_key`, `lez_signing_key`, wallet paths, RPC URLs, and raw payloads. The docs should also warn app authors that generated module methods are not an appropriate boundary for secret-bearing convenience arguments unless logging redaction is enabled.

**Disposition:** do not file yet; redact logs manually before any upstream/downstream issue filing.

### Downstream: maker loop retries sequencer failures too aggressively

**Classification:** downstream app issue; locally mitigated.

When the LEZ sequencer is unreachable, the live maker loop emits `AutoAcceptSwapFailed` for every balance-check failure. Runtime testing showed this can flood the Completed Swaps list with failures in seconds. The loop now waits for the configured poll interval before retrying client-init and balance-check failures.

**Disposition:** fixed locally; still worth validating whether long-running live maker should eventually surface a terminal infrastructure error instead of retrying forever.

### Upstream/doc packet: no second-subscribe race-window guidance for per-swap topics

**Classification:** upstream docs/API ergonomics issue; locally mitigated, not fixed.

The journey doc shows the correct lifecycle for a single, statically-known
content topic (`createNode → start → subscribe → send → unsubscribe → stop`).
Per-swap coordination on `/atomic-swaps/1/swap-<hashlock>/json` introduces
a topic that is only known after a runtime trigger (the maker learns the
hashlock when an on-chain ETH lock is detected; the taker learns it when
it generates the preimage). Subscribing late means a `SwapAccept` published
by the taker right after `EthLocked` can arrive before the maker subscribes
and is lost — Delivery `messageReceived` only fires for live messages.

The adapter mitigates by (a) draining `fetchSwapEvents` immediately after
`subscribeSwap` succeeds and (b) polling on a 1s timer while a swap
coordination is active. The taker still publishes `SwapAccept` exactly once,
because Delivery has no documented guidance on idempotent retries and `send`
already returns a request ID that the docs only describe surfacing through
`messageSent`/`messagePropagated`.

**Suggested doc-packet additions:**
- A worked example for a "topic per logical session" pattern (what to do
  when both sides subscribe at different times).
- Either an idempotent-retry recipe for the publishing side, or a documented
  store-and-forward bound for `subscribe`-after-`send`.
- Confirmation of whether `messageSent`/`messagePropagated` request IDs are
  meant to drive caller-side retry logic, or are observability-only.

**Disposition:** capture for the doc packet; do not file yet. The local
race-window was not reproduced under realistic load in this session.

### Upstream: no documented way to enumerate active subscriptions

**Classification:** upstream module/API ergonomics issue; locally mitigated.

`delivery_module` exposes `subscribe(topic)` and `unsubscribe(topic)` but
not a `getSubscriptions()` (or similar) method to query which topics the
underlying node currently subscribes to. The adapter has to maintain its
own per-process map of `hashlock → subscribed?` to know whether a `subscribe`
call was already issued for a given swap, which means the source of truth
about subscription state is split between Delivery and the consumer.

`messagingStatus` now exposes `swap_subscription_count` for visibility,
but only what the adapter believes — not what Delivery actually holds.

**Suggested fix:** add a documented status method that returns the active
subscription set, or a `subscribeIfNot` semantic so consumers don't have
to mirror the state.

**Disposition:** capture for the doc packet / module API feedback; do not
file yet. Local mitigation is acceptable for M2.

### Upstream/doc packet: idempotency of `subscribe` is undocumented

**Classification:** upstream docs issue; behaviour assumed but not verified
under contention; locally tolerated.

The README and journey doc document `subscribe(topic)` returning a
`LogosResult`, but do not specify whether issuing `subscribe` twice for the
same topic is a no-op, an error, or starts a second underlying subscription.
The M2 adapter conservatively skips redundant `subscribe` calls based on
its own state map (see issue above), which avoids the question.

**Suggested doc-packet addition:** explicitly state the contract for
duplicate `subscribe(topic)` and `unsubscribe(topic)` calls.

**Disposition:** capture for the doc packet; do not file yet.

### Repo reproducibility: ignored backend lockfile

**Classification:** repo issue; fixed locally.

`swap-module/flake.lock` was ignored, which made the Delivery dependency graph less reproducible. The ignore rule was removed and the lockfile now records the resolved `delivery_module` revision.

**Disposition:** fixed in this repo.

### Repo: ad-hoc two-Basecamp setup not reproducible across sessions

**Classification:** repo / dev tooling issue; fixed locally.

The M1 cross-node proof relied on hand-assembled isolated Basecamp roots
(separate `LOGOS_DATA_DIR`, HOME, XDG, runtime, wallet) plus manual `lgpm
install` invocations. Nothing in the repo captured that recipe, so each new
test session had to re-derive it. M2 verification needs the same dual-instance
setup repeatedly, which made this friction the bottleneck.

The repo now ships [`scripts/basecamp-instance.sh`](scripts/basecamp-instance.sh)
plus `make basecamp-{init,run,paths,clean}-{maker,taker}` targets that
materialize fully isolated instances under `.basecamp/<name>/` and install
the `delivery_module`, `swap`, and `swap_ui` LGX packages into each. README
documents the two-Basecamp workflow.

**Disposition:** fixed in this repo.

### Upstream/doc packet: short-path requirement for Basecamp runtime sockets is undocumented

**Classification:** upstream docs/UX issue; locally mitigated, not fixed.

When `XDG_RUNTIME_DIR` (and `TMPDIR`) point at a deep path such as
`<repo>/.basecamp/maker/run`, Basecamp aborts module loading with
`[SubprocessContainer] Unix socket path too long (122 >= 104):
<runtime>/logos_token_<module>_<pid>` because macOS caps `sockaddr_un.sun_path`
at 104 bytes. The error is surfaced clearly, but the requirement is not
documented anywhere in the Basecamp / liblogos integration journey, and
following the natural pattern of "isolated runtime dir alongside other
isolated dirs" hits this on any repo that lives under
`/Users/<user>/Developer/...`.

The launcher script forces `XDG_RUNTIME_DIR=/tmp/lbc-<name>/` (and the same
`TMPDIR`) so the socket suffix `logos_token_<module>_<pid>` always fits the
budget.

**Suggested doc-packet additions:**
- Call out the macOS `sun_path == 104` budget in the Basecamp embedding /
  multi-instance docs.
- Recommend setting `XDG_RUNTIME_DIR` (and `TMPDIR`) to a short prefix on
  macOS, with an example.
- Optionally fall back to a hashed/short directory inside Basecamp itself
  when the configured runtime dir would overflow.

**Disposition:** capture for the doc packet; do not file yet. Local
mitigation is in `scripts/basecamp-instance.sh`.

### Upstream: `bin-macos-app` / `lgpm` variant flavor mismatch silently breaks downstream LGX install

**Classification:** upstream packaging/distribution issue; locally mitigated,
not fixed.

The bundled macOS Basecamp distribution (`logos-co/logos-basecamp#bin-macos-app`)
and the host-installed `logos-co/logos-package-manager#lgpm` CLI disagree on
which `manifest.json` `main` variant key they will accept for the same host:

- `bin-macos-app` Basecamp links a PackageManagerLib compiled WITH
  `LGPM_PORTABLE_BUILD` defined. Its `platformVariantsToTry()` returns
  ONLY the bare host string (`darwin-arm64`, `linux-amd64`, etc.). All of
  its bundled embedded modules (`capability_module`, `package_manager`,
  `package_downloader`) ship with `manifest.json` `main` keyed by the
  bare host string and a `variant` file containing the bare host string.
- `lgpm` (host-installed via `~/.nix-profile/bin/lgpm`, currently
  `lgpm version 1.0.0`) is compiled WITHOUT `LGPM_PORTABLE_BUILD` —
  on `lgpm install --file <pkg.lgx>` it requires the LGX archive to
  contain a `variants/<host>-dev/` directory, and rejects portable LGX
  packages with `Error: Package does not contain variant for platform:
  darwin-arm64-dev`.
- The default `#lgx` output of every Logos module flake (via
  `nix-bundle-lgx`) bundles a manifest keyed by `<host>-dev` (and a
  `variants/<host>-dev/` archive directory), matching `lgpm` but NOT
  the bundled Basecamp.
- The `#lgx-portable` output bundles a manifest keyed by `<host>` (no
  `-dev`), matching the bundled Basecamp but NOT host `lgpm`.

The failure modes are silent and confusing for downstream apps:

1. `lgpm install` accepts the default `#lgx` happily, places the files
   under `<user-dir>/modules/<name>/`, and `lgpm list` shows the module.
   Then `bin-macos-app` Basecamp's PackageManagerLib scans the same
   directory, fails to resolve `mainFilePath` for the `<host>-dev` key,
   silently drops the module from the registry (the registry filters
   `mainFilePath.empty()`), and any UI plugin that depends on it logs
   `Cannot load unknown module: <name>` and `Failed to load core
   dependency` when the user opens the tab. None of this surfaces as an
   `lgpm install` error.
2. Building `#lgx-portable` and trying `lgpm install` errors out with
   the platform-variant message, which is at least loud — but `lgpm`
   does not document the `LGPM_PORTABLE_BUILD` flag distinction or the
   `darwin-arm64` vs `darwin-arm64-dev` matrix.

This is not specific to this app: it would hit any Logos universal
module whose downstream consumers are pointed at `bin-macos-app` and
told to use `lgpm` from the same Logos release channel. The upstream
`logos-delivery-module` v0.1.1 LGX is also affected — its `#lgx`
output bundles `darwin-arm64-dev` and is silently dropped by
`bin-macos-app` in the same way.

The two-Basecamp launcher in this repo now mitigates by:
- Building `#lgx-portable` for `delivery_module` v0.1.1, `swap-module`,
  and `swap-ui` (so the manifest matches `bin-macos-app`'s expected
  variant), and
- Bypassing `lgpm install` entirely with a small `extract_lgx_variant`
  shell helper in `scripts/basecamp-instance.sh`. The helper untars the
  LGX, copies `manifest.json` and `variants/<host>/<files>` into the
  user-dir under the layout the embedded PackageManagerLib expects, and
  writes a one-line `variant` file. This matches what `lgpm install`
  *would* have produced for a matching-variant package, and matches
  the layout of the bundled embedded modules shipped inside
  `bin-macos-app`.
- `make swap-lgx-build` now builds both `#lgx` and `#lgx-portable` for
  swap-module and swap-ui so neither downstream consumer (Basecamp vs
  logos-standalone-app) is left without a usable artifact.

**Suggested fix:**
- Either align `bin-macos-app` and `lgpm` on the same
  `LGPM_PORTABLE_BUILD` setting in the Logos release pipeline, or
- Have `lgpm install` accept either `<host>` or `<host>-dev` variant
  archives transparently and write them into the user-dir as the
  consumer Basecamp expects (i.e. derive the install variant from the
  consumer's PackageManagerLib build mode, not from the package name),
  and
- Surface a loud error from `bin-macos-app`'s PackageManagerLib when
  it scans a module directory that has a `manifest.json` but every
  `main[*]` key fails to match the configured variant list (today the
  module is silently dropped — the only signal is a downstream
  `Cannot load unknown module` further along the call graph).

**Disposition:** capture for the doc packet and upstream packaging
feedback; do not file yet. Local mitigation is in
`scripts/basecamp-instance.sh` and the M2 cross-node proof can now
proceed against `bin-macos-app` without further upstream changes.

### Downstream: QML singleton via relative `import "."` was undefined inside `bin-macos-app` Basecamp

**Classification:** downstream app issue surfaced by switching the cross-node
test harness onto `bin-macos-app`; fixed locally; partial upstream-doc
candidate.

`swap-ui/src/qml/Theme.qml` is a `pragma Singleton` with the colours,
spacings, and font sizes used across every panel. Originally it was
declared in a single `swap-ui/src/qml/qmldir` (`singleton Theme 1.0
Theme.qml`) and consumed via relative `import "."` from each `.qml`
file. With the previous `logos-standalone-app` host this was sufficient
— the renderer resolved `Theme.surface`, `Theme.spacingNormal`, etc.
correctly.

Inside `bin-macos-app` Basecamp's QML6 engine, the same QML files
loaded from disk produced a flood of
`Unable to assign [undefined] to QColor` / `Unable to assign [undefined]
to double` warnings on every property bound to a `Theme.*` value (full
log captured in `.basecamp/maker/basecamp.log`). The end result was the
broken render in `swap_ui` we hit during dogfooding: every panel
collapsed to (0,0)-anchored stacked text because the layout dimensions
(`Theme.spacing*`, `Theme.radius*`, `Theme.fontSmall`, etc.) all
evaluated to undefined and the layout system fell back to zero-sized
items. The error was not loud about Theme being unresolved — only the
downstream type-coercion failures showed up.

The bundled `package_manager_ui` plugin in the same Basecamp build
sidesteps this by always declaring qmldir entries inside a named
module (e.g. `qml/Panels/qmldir` starting with `module Panels`,
`qml/Icons/qmldir` starting with `module Icons` + `singleton
PackageIcons 1.0 PackageIcons.qml`) and importing them by name (e.g.
`import Panels`, `import Logos.Theme`, `import Icons`). It never relies
on `import "."` to pick up a same-directory singleton, even when the
QML file and the qmldir are siblings.

**Fix applied locally:**
- Moved `swap-ui/src/qml/Theme.qml` into `swap-ui/src/qml/SwapTheme/Theme.qml`.
- Created `swap-ui/src/qml/SwapTheme/qmldir` with `module SwapTheme` +
  `singleton Theme 1.0 Theme.qml`.
- Removed the now-empty top-level `swap-ui/src/qml/qmldir`.
- Replaced `import "."` with `import SwapTheme` in all eight QML files
  under `swap-ui/src/qml/` (`AtomicSwapView`, `ConfigPanel`, `Main`,
  `MakerView`, `ProgressStepper`, `RefundView`, `ResultCard`, `TakerView`).

Same-directory non-singleton types (`AtomicSwapView`, `ConfigPanel`,
etc.) continue to resolve through Qt 6's implicit same-directory
import, so removing `import "."` did not require any other source
changes.

There is also a sub-finding worth noting because it took a build cycle
to spot: nix flakes that use `src = ./.` only see git-tracked files.
Newly created files (the `SwapTheme/qmldir` here) need to be
`git add`-ed before they appear in the LGX archive — otherwise the
build silently produces a working `.lgx` whose `qml/SwapTheme/` has
`Theme.qml` but no `qmldir`, and the Theme singleton stays unresolved
at runtime even though the source tree on disk is correct. This is
generic to any nix-flake-built LGX package.

**Suggested doc-packet additions:**
- For module authors building UI plugins: recommend the named-module
  qmldir pattern (`module <Name>` + named `import <Name>`) over
  relative-directory `import "."` for singletons, even for
  same-directory singletons. Call out `bin-macos-app` Basecamp as the
  reference target where the relative form silently fails.
- For the LGX/nix builder docs: call out that `src = ./.` flakes will
  silently skip untracked QML support files (qmldir, .conf, generated
  resources), and the LGX will not flag the omission.

**Disposition:** fixed locally; capture the named-module recommendation
and the untracked-files warning for the doc packet.

### Downstream/release hygiene: stale installed LGX packages can mask rebuilt source

**Classification:** downstream test-harness issue; fixed locally by reinstalling
portable LGX packages before runtime proof.

During the final two-Basecamp proof, source and Nix package rebuilds were not
enough by themselves: the isolated Basecamp roots still had previously
installed `swap`/`swap_ui` packages. That produced misleading runtime behaviour
on the per-swap topic even though the working tree contained the fix. Rebuilding
and reinstalling the current portable LGX artifacts into both maker and taker
Basecamp roots resolved the mismatch and the swap completed end-to-end.

**Suggested doc-packet addition:** downstream Basecamp dogfooding docs should
call out the full loop: rebuild LGX, reinstall it into every isolated
`--user-dir`, then restart Basecamp. A successful Nix build does not imply an
already-running or already-installed Basecamp instance is using that artifact.

**Disposition:** fixed locally; capture for the doc packet / app integration
journey.

### Upstream highlight: `--user-dir` flag cleanly isolates Basecamp instances

**Classification:** upstream improvement that landed in `logos-basecamp`;
positive — note for downstream apps.

Earlier in this dogfooding session the planned isolation used
`LOGOS_DATA_DIR`, but the non-portable build appends `Dev` to it, which made
naming confusing. The current `logos-basecamp` exposes a first-class
`--user-dir` (and `LOGOS_USER_DIR` env) override that bypasses both the
portable/non-portable selection and the "Dev" suffix and is set as the exact
value passed in. Adopting it removed all of the suffix-handling boilerplate
in the launcher script.

**Disposition:** no upstream action; record so the doc packet recommends
`--user-dir` over `LOGOS_DATA_DIR` for downstream multi-instance test
harnesses.

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
- Flake evaluation and dry-runs: passing for both `swap-module` and `swap-ui`.
- Full package build: passing after the cold Delivery dependency graph
  completed (subsequent rebuilds are cache hits — the M2 changes only
  re-trigger swap-ffi rebuild and the C++ wrapper rebuild, not the
  Delivery dependency stack).
- Unit tests: passing — `swap-module` test suite now covers the per-swap
  coordination wrapper surface (`subscribe_swap_requires_runtime`,
  `unsubscribe_swap_requires_runtime`, `publish_swap_accept_requires_runtime`,
  `fetch_swap_events_returns_empty_shape_without_runtime`,
  `run_maker_emits_hashlock_in_eth_lock_detected`). 16/16 tests pass.
- Same-artifact runtime smoke: `QT_QPA_PLATFORM=offscreen nix run . -- --help` passes.
- Cross-node Delivery receipt proof: passing for M1 offer discovery. Two
  isolated Basecamp instances loaded the rebuilt LGX packages, maker
  published on `/atomic-swaps/1/offers/json`, taker logged `messageReceived`,
  and taker `fetchOffers` returned the offer.
- Per-swap Delivery coordination proof: passing. After reinstalling the current
  portable LGX artifacts into both isolated Basecamp roots, the taker published
  `SwapAccept` on `/atomic-swaps/1/swap-<hashlock>/json`, the maker drained the
  per-swap event bucket, and both sides completed the same on-chain swap. Maker
  progress reached `EthLockDetected` -> `LezLocked` -> `PreimageRevealed` ->
  `EthClaimed` -> `AutoAcceptSwapCompleted`; taker progress reached
  `EthLocked` -> `LezLockDetected` -> `LezEscrowVerified` -> `LezClaimed`.
- Two-Basecamp scaffolding: `scripts/basecamp-instance.sh` plus
  `make basecamp-{init,run,paths,clean}-{maker,taker}` are in place. Both
  instances boot cleanly with isolated `--user-dir`, HOME, XDG, runtime
  (`/tmp/lbc-<name>`), and wallet paths against the Nix-built
  `bin-macos-app` Basecamp on macOS. Verified by parallel boot of maker +
  taker — each loads `capability_module` and `package_manager` from its
  own user-dir without socket-path collisions. GUI-driven swap flow still
  needs a human to drive the maker/taker tabs.
- LGX install path: the launcher now installs `#lgx-portable` (variant
  `darwin-arm64`) via a manual `extract_lgx_variant` helper that bypasses
  `lgpm install`, because host `lgpm` and `bin-macos-app` Basecamp's
  embedded PackageManagerLib disagree on `LGPM_PORTABLE_BUILD` (see
  upstream finding above). Maker boot with the portable LGX produces zero
  warnings/errors and no `Cannot load unknown module` / `Module not found`
  messages, vs. the pre-fix boot which logged
  `[warning] Module not found in known modules: swap` /
  `Failed to load core dependency "swap" for "swap_ui"` as soon as the
  user opened the swap_ui tab.

## Evidence required before upstream/downstream filing

- Exact command run and result.
- Redacted config used for `createNode`.
- Whether `onInit` fired in the same Nix-built artifact.
- Delivery `start` / `subscribe` result.
- Cross-node receipt evidence via `messageReceived`, not just `send` success. Captured for M1 offer discovery in Basecamp runtime logs; redact before sharing because Basecamp logs include full method args.
- Cold-build timing and whether each heavy derivation came from cache or built locally.
- For #18, whether local multi-instance verification needed `portsShift`.
- For #26, the raw event timestamp shape if reproduced.
- For M2 cross-node proof: confirmation that both sides used identical
  Nix-built LGX artifacts after the per-swap topic wiring was added,
  the maker's `coordinationActiveHashlock` matched the taker's, and
  the maker's `coordinationEventsJson` contained a `SwapAccept` whose
  `hashlock`, `eth_swap_id`, `taker_lez_account`, and `taker_eth_address`
  fields agreed with what the taker actually published.

## Runtime proof notes

- Two Basecamp instances were launched with isolated `LOGOS_DATA_DIR`, HOME, XDG, runtime, and wallet paths. The setup is now codified in `scripts/basecamp-instance.sh` and the `basecamp-*` Make targets, which use the more recent `--user-dir` Basecamp flag (no `Dev` suffix) and force `XDG_RUNTIME_DIR=/tmp/lbc-<name>` to stay under the macOS Unix-socket path budget.
- `delivery_module`, `swap`, and `swap_ui` LGX packages were installed into both isolated Basecamp roots.
- Delivery offer discovery used the public Delivery preset (`mode: Core`, `preset: logos.dev`) with per-instance `portsShift`; no embedded Waku rendezvous node or `waku-bindings` path was used.
- `make infra` is still required for the existing ETH/LEZ swap flow after offer discovery. During this run it regenerated `.env` and `.env.taker` with Anvil on `ws://localhost:60531` and LEZ sequencer on `http://127.0.0.1:3040/`.
- M1 succeeded: maker offer publish produced Delivery send/propagation logs, taker received the offer via `messageReceived`, and taker displayed the discovered offer.
- M2 succeeded after current LGX reinstall: per-swap maker/taker coordination
  is exposed on `/atomic-swaps/1/swap-<hashlock>/json` through
  `subscribeSwap` / `unsubscribeSwap` / `publishSwapAccept` /
  `fetchSwapEvents`, and the swap UI triggers those calls from existing
  maker/taker progress events. The underlying Rust orchestrator still drives
  state from on-chain watchers, so M2 remains non-blocking off-chain context,
  but the live two-Basecamp flow confirmed the Delivery channel receives the
  taker's `SwapAccept` while the on-chain swap completes.

## Redaction reminder

Before filing upstream, downstream, or repo issues, redact private keys, node keys, account data, private multiaddrs, RPC URLs, and raw private payloads.
