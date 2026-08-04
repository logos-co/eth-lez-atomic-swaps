# Delivery dogfooding — issue-filing triage

> **HISTORICAL / ARCHIVED:** This triage preserves dated evidence and may
> reference removed launch workarounds. Do not use it as current run
> instructions. Use the repository README's
> [**Manual Basecamp Run**](../README.md#manual-basecamp-run) flow;
> Basecamp-native automation is tracked in
> [PR #90](https://github.com/logos-co/eth-lez-atomic-swaps/pull/90).

This is the working triage for the findings in
[`delivery-dogfooding.md`](../delivery-dogfooding.md). Each row cross-checks
the claim against (a) the
[doc-packet at `c85b6bb`](https://github.com/logos-co/logos-docs/blob/c85b6bb6fec078f488fb2471a6851652f011ca9c/docs/messaging/journeys/use-the-logos-delivery-module-api-from-an-app.md)
and (b) the current state of this repo, then states the residual gap and the
intended filing destination.

Conventions:

- **Pinned references:** `delivery_module` v0.1.1 = `0c346c0c`, journey doc =
  `c85b6bb` (linked above).
- **Doc evidence** is either the journey doc, the v0.1.1 module README, or
  [`src/delivery_module_plugin.h`](https://github.com/logos-co/logos-delivery-module/blob/0c346c0c2ab2404c11a62cd6c385e806e8465434/src/delivery_module_plugin.h).
- **Code evidence** is the workaround/mitigation as shipped on this branch
  today (with file + line range so we can re-verify before filing).
- **Status legend:**
  - `READY` — finding stands; gather runtime evidence then file.
  - `WATCH` — keep tracking; do not file from this repo yet.
  - `FIXED` — fixed in this repo only; do **not** file, keep in dogfooding log
    so we don't regress.
  - `DOC-ONLY` — file against the journey-doc / doc-packet only.

---

## A. `logos-co/logos-delivery-module` (module API)

### A1. `#18` multi-instance port configuration

| | |
|---|---|
| **Claim** | Adapter accepts `delivery.portsShift` / `portsShift` so two local nodes don't collide; doc/issue acknowledges the gap |
| **Doc evidence** | Journey §5 + §6 already document `portsShift` as the workaround and link [`logos-delivery-module#18`](https://github.com/logos-co/logos-delivery-module/issues/18) as the planned env-var fix |
| **Code evidence** | [swap_delivery_adapter.cpp#L263-L264](//swap-module/src/swap_delivery_adapter.cpp#L263-L264) accepts `portsShift`; [swap_ui_plugin.cpp#L382](//swap-ui/src/swap_ui_plugin.cpp#L382) injects per-process random shift |
| **Gap** | Issue is **already filed upstream**. No new issue needed |
| **Status** | `WATCH` — add a +1 comment on #18 with our local-mitigation note if useful, otherwise no action |

### A2. `#26` `messageReceived` timestamp shape

| | |
|---|---|
| **Claim** | `messageReceived` timestamp is ns-since-epoch while other events expose ISO-8601 |
| **Doc evidence** | Journey §3 step 1 explicitly calls this out and links #26 as known-inconsistent |
| **Code evidence** | Adapter stamps offers with a local `timestamp_ms` instead of trusting `data[3]` ([swap_delivery_adapter.cpp ~L300+](//swap-module/src/swap_delivery_adapter.cpp)) |
| **Gap** | Already filed upstream as #26. No new issue |
| **Status** | `WATCH` — only file follow-up if we observe a different shape; the dogfooding finding can be reduced to a one-liner pointing at #26 |

### A3. `#27` watch item

| | |
|---|---|
| **Claim** | No runtime evidence reproduced from this integration |
| **Doc evidence** | n/a |
| **Code evidence** | n/a |
| **Gap** | Nothing to file |
| **Status** | `WATCH` — keep as-is |

### A4. First-class peer count / status API

| | |
|---|---|
| **Claim** | Peer count is not a first-class API; demo polls `getNodeInfo("Metrics")` and parses Prometheus; that path caused capability-token churn when tried from the embedded swap module |
| **Doc evidence** | Journey §3 step 1 documents only `connectionStateChanged` (status string + timestamp); the v0.1.1 README's *Module Interface* lists no `getPeerCount` / structured `getStatus` |
| **Code evidence** | Adapter exposes a flat `peer_count: 0` in `messagingStatus` ([swap_delivery_adapter.cpp#L536, #L808](//swap-module/src/swap_delivery_adapter.cpp#L536)); UI consumes it ([swap_ui_plugin.cpp#L1152](//swap-ui/src/swap_ui_plugin.cpp#L1152)). No `getNodeInfo("Metrics")` is called from this repo today |
| **Gap** | (i) module-API request for `getPeerCount()` / structured `getStatus()`; (ii) doc-packet should warn that `getNodeInfo("Metrics")` is a fragile bridge for downstream apps |
| **Status** | `READY` — file as API/UX request on `logos-delivery-module`; cross-link doc-packet PR |

### A5. Second-subscribe race-window for per-swap topics

| | |
|---|---|
| **Claim** | Doc shows the lifecycle for a single static topic; gives no guidance for "topic per logical session" where one side subscribes after the other already published |
| **Doc evidence** | Journey §3 steps 1–6 show `createNode → start → subscribe → send → unsubscribe → stop` for one statically-known topic. §6 troubleshooting #4 confirms `messageReceived` only fires post-`subscribe()`, but there is no late-subscriber recipe or store-and-forward bound |
| **Code evidence** | Adapter drains `fetchSwapEvents` immediately after `subscribeSwap` ([swap_delivery_adapter.cpp#L739+](//swap-module/src/swap_delivery_adapter.cpp#L739)); UI polls every 1s while a coordination is active ([swap_ui_plugin.cpp#L205-L210](//swap-ui/src/swap_ui_plugin.cpp#L205-L210)); taker publishes `SwapAccept` once ([swap_ui_plugin.cpp#L1588-L1632](//swap-ui/src/swap_ui_plugin.cpp#L1588-L1632)) |
| **Gap** | Doc-packet should add a worked "topic per logical session" pattern + clarify whether `messageSent`/`messagePropagated` request IDs are meant to drive idempotent retry or are observability-only |
| **Status** | `DOC-ONLY` — file against the journey doc with the suggested wording from the dogfooding entry |

### A6. No documented way to enumerate active subscriptions

| | |
|---|---|
| **Claim** | `subscribe(topic)` / `unsubscribe(topic)` exist; no `getSubscriptions()` or `subscribeIfNot`. Consumer must mirror state |
| **Doc evidence** | Journey §3 step 4 documents only `subscribe`; module README *Module Interface* lists no enumeration method; `delivery_module_plugin.h` exposes no such method either |
| **Code evidence** | Adapter keeps its own `hashlock → subscribed?` map and surfaces it as `swap_subscription_count` ([swap_delivery_adapter.cpp#L538](//swap-module/src/swap_delivery_adapter.cpp#L538)) |
| **Gap** | Real module-API ergonomics request — either `getSubscriptions()` or `subscribeIfNot` semantics |
| **Status** | `READY` — file on `logos-delivery-module` as an API request |

### A7. Idempotency of `subscribe` is undocumented

| | |
|---|---|
| **Claim** | Behaviour of `subscribe(topic)` called twice for the same topic is unspecified (no-op? error? second subscription?) |
| **Doc evidence** | Journey §3 step 4 does not state the contract; module README *Module Interface* doesn't either; `delivery_module_plugin.h` Doxygen for `subscribe` is silent on duplicates |
| **Code evidence** | Adapter conservatively skips redundant calls via the per-process map (see A6) |
| **Gap** | Doc-packet should state the contract for duplicate `subscribe`/`unsubscribe`; pairs naturally with A6 |
| **Status** | `DOC-ONLY` — combine with A6 filing or attach as a sibling doc PR |

---

## B. `logos-co/logos-basecamp` (host)

### B1. Module proxy logs include sensitive method arguments

| | |
|---|---|
| **Claim** | Basecamp's generated module proxy logs full method args, including secret-bearing config; forced a `fetchBalancesFromEnv(path)` workaround instead of reusing `fetchBalances(configJson)` |
| **Doc evidence** | Journey doc is silent on Basecamp's IPC logging behaviour |
| **Code evidence** | Workaround exists: [swap_impl.cpp#L320](//swap-module/src/swap_impl.cpp#L320) `fetchBalancesFromEnv`; UI calls it via [swap_ui_plugin.cpp#L760](//swap-ui/src/swap_ui_plugin.cpp#L760) |
| **Gap** | Basecamp needs an opt-in redaction / arg-filter mode for known secret config keys |
| **Status** | `READY` — file on `logos-basecamp` with **redacted** log excerpt. Also worth a doc-packet warning that generated module methods are not a safe boundary for secret args |

### B2. `bin-macos-app` / `lgpm` variant flavour mismatch

| | |
|---|---|
| **Claim** | `bin-macos-app` PackageManagerLib (built with `LGPM_PORTABLE_BUILD`) accepts only the bare `<host>` variant; host `lgpm` (built without) rejects portable LGX and only accepts `<host>-dev`. Default `#lgx` is silently dropped by Basecamp; `#lgx-portable` is rejected by `lgpm`. Failure is silent on the Basecamp side |
| **Doc evidence** | Journey §3 step 4 documents `nix build .#lgx` only and does not mention the portable variant or this mismatch; no warning about silent-drop behaviour |
| **Code evidence** | Launcher uses `#lgx-portable` and bypasses `lgpm install` via `extract_lgx_variant`: [basecamp-instance.sh#L137, #L205-L247](//scripts/basecamp-instance.sh#L137); `swap-module` and `swap-ui` flakes build both `#lgx` and `#lgx-portable` |
| **Gap** | (i) Align `bin-macos-app` and `lgpm` on the same `LGPM_PORTABLE_BUILD`, or have `lgpm install` accept either variant; (ii) Basecamp PackageManagerLib should log loudly when a module dir has a `manifest.json` whose `main[*]` keys never match the configured variant list (today silent → surfaces only as `Cannot load unknown module` later) |
| **Status** | `READY` — file (i) on `logos-co/logos-package-manager` and (ii) on `logos-co/logos-basecamp`; reference the same root cause in both. Also a doc-packet warning |

### B3. Short-path requirement for runtime sockets on macOS is undocumented

| | |
|---|---|
| **Claim** | macOS `sockaddr_un.sun_path` 104-byte limit causes Basecamp to abort with a clear error if `XDG_RUNTIME_DIR` is a deep path. Common embedding patterns (`<repo>/.basecamp/<name>/run`) trip this on any repo under `/Users/<user>/Developer/...` |
| **Doc evidence** | Journey doc and Basecamp embedding docs do not mention the budget |
| **Code evidence** | Launcher forces `XDG_RUNTIME_DIR=/tmp/lbc-<name>` (and `TMPDIR`): [basecamp-instance.sh#L25, #L284](//scripts/basecamp-instance.sh#L25) |
| **Gap** | Doc-packet should call out the budget with a concrete recommendation; Basecamp could optionally hash/shorten the runtime dir internally as a fallback |
| **Status** | `DOC-ONLY` (primary) + optional `READY` on Basecamp for the fallback. File doc-PR first |

### B4. `--user-dir` (positive highlight)

| | |
|---|---|
| **Claim** | First-class `--user-dir` / `LOGOS_USER_DIR` flag exists and is cleaner than `LOGOS_DATA_DIR` (which appends `Dev` in non-portable builds) for multi-instance test harnesses |
| **Doc evidence** | Journey doc does not currently mention `--user-dir` in §3 step 4 or anywhere in the multi-instance discussion |
| **Code evidence** | Launcher uses `--user-dir`: [basecamp-instance.sh#L18, #L184, #L269, #L290](//scripts/basecamp-instance.sh#L18) |
| **Gap** | Doc-packet should recommend `--user-dir` for downstream multi-instance test harnesses |
| **Status** | `DOC-ONLY` — file against journey doc |

### B5. Cold downstream build (~1h)

| | |
|---|---|
| **Claim** | Cold cache build of `zerokit`, `liblogosdelivery-dev`, `logos-delivery_module-module` took about an hour despite Logos Cachix being configured |
| **Doc evidence** | Journey §3 step 4 acknowledges Nix build time varies but says nothing about cache coverage of native deps |
| **Code evidence** | n/a — this is a build-system observation |
| **Gap** | Either CI should publish substitutes for those derivations on supported systems, or the doc should set expectations explicitly |
| **Status** | `READY` — file on `logos-delivery-module` (packaging/CI), but **re-time the build first** to ensure the situation hasn't already improved on a more recent Cachix push. Evidence required = the per-derivation cache-hit vs build log |

---

## C. Doc-packet only (`logos-co/logos-docs`)

These have no separate upstream-code component beyond the doc. Roll them into
a single PR against the journey doc if possible.

### C1. Named-module qmldir recommendation for UI plugins

| | |
|---|---|
| **Claim** | Inside `bin-macos-app` Basecamp's QML6 engine, `pragma Singleton` resolved via relative `import "."` silently produced `undefined`. Fix is the named-module qmldir pattern (matches `package_manager_ui` plugin) |
| **Doc evidence** | Doc-packet is silent on QML plugin packaging conventions |
| **Code evidence** | All 8 QML files import `SwapTheme` ([rg confirmed](//swap-ui/src/qml)); `SwapTheme/qmldir` + `SwapTheme/Theme.qml` are git-tracked |
| **Gap** | Doc-packet should recommend the named-module pattern and call out `bin-macos-app` as the reference target where relative-`.` fails silently |
| **Status** | `DOC-ONLY` |

### C2. `src = ./.` flakes silently skip untracked files

| | |
|---|---|
| **Claim** | LGX produced from `src = ./.` only includes git-tracked files; missing `qmldir`/`.conf`/generated files do not fail the build but break runtime |
| **Doc evidence** | Doc-packet does not warn |
| **Code evidence** | n/a |
| **Gap** | Doc-packet (LGX/Nix builder docs) should warn explicitly |
| **Status** | `DOC-ONLY` — pair with C1 |

### C3. Reinstall LGX into every isolated `--user-dir` after rebuild

| | |
|---|---|
| **Claim** | Successful Nix rebuild does not imply already-running / already-installed Basecamp instances pick up the new artifact |
| **Doc evidence** | Doc-packet doesn't describe a dogfooding/test-harness loop |
| **Code evidence** | n/a |
| **Gap** | Add the rebuild → reinstall → restart loop to the journey doc's multi-instance section |
| **Status** | `DOC-ONLY` — pair with B4 |

---

## D. Fixed in this repo (do not file — guard against regression)

These should stay in the dogfooding log as proof-of-completion, but generate
no GitHub issues.

| Finding | Verified state |
|---|---|
| D1. Old Waku bootstrap gate removed | `src/messaging/` gone; no `waku` feature; no `[patch.crates-io]` rln/core2 in [Cargo.toml](//Cargo.toml); no `vendor/` patches dir |
| D2. Maker loop retries sequencer failures on backoff | [src/swap/maker.rs#L260-L310](//src/swap/maker.rs#L260-L310) — every `AutoAcceptSwapFailed` arm now sleeps `base_config.poll_interval` before retrying. Open sub-question: should it ever escalate to terminal infra error? Capture as in-repo TODO, not a Delivery issue |
| D3. SwapTheme named-module qmldir | All 8 QML files use `import SwapTheme`; SwapTheme dir + qmldir git-tracked (see C1 code evidence) |
| D4. `swap-module/flake.lock` is tracked | `git ls-files` confirmed |
| D5. Two-Basecamp launcher exists | [scripts/basecamp-instance.sh](//scripts/basecamp-instance.sh) + `make basecamp-{init,run,paths,clean}-{maker,taker}` targets |
| D6. Hashlock-vs-topic validation before caching | [swap_delivery_adapter.cpp#L371-L376](//swap-module/src/swap_delivery_adapter.cpp#L371-L376) drops mismatched embedded hashlocks |

---

## Filing order suggestion

Once the runtime evidence below is captured, file in this order so each issue
can reference the previous:

1. **A6** (`getSubscriptions` / `subscribeIfNot`) — small, self-contained API ask.
2. **A4** (peer count / status API) — same repo, similar shape.
3. **B1** (Basecamp log redaction) — independent, needs a redacted log paste.
4. **B2** (variant mismatch) — file the lgpm-side and basecamp-side issues as a pair, cross-linked.
5. **B5** (cold build cache) — only after re-timing on the current Cachix state.
6. **Doc-packet PR** consolidating: A5, A7, B3, B4, C1, C2, C3, plus references back to A4/A6/B1/B2.

## Evidence we still need to collect before filing

Per the dogfooding log's existing
[*Evidence required* section](../delivery-dogfooding.md#L494-L509), every
`READY` row above needs:

- The exact CLI/build command and its result (redacted).
- For B5: per-derivation cache-hit vs build log.
- For B1: a redacted Basecamp log excerpt showing a secret-bearing method arg.
- For B2: a `nix build .#lgx` + `lgpm install` reproduction transcript on a
  clean `--user-dir`, plus the Basecamp boot log showing the silent drop.
- For A4 and A6: a 10-line reproduction module/script (can live in a gist
  rather than this repo) so the upstream maintainer doesn't have to install
  the whole swap stack.
