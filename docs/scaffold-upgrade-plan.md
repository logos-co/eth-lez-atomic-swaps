# Scaffold Upgrade Plan

Captures the upstream filing queue + project-internal cleanup queue produced by
the `scaffold.toml` 0.1.1 → 0.2.0 upgrade pass on this repo. Use this as the
resume point.

For mental model + per-entry rationale, see
[`docs/scaffold-upstream-tracker.md`](./scaffold-upstream-tracker.md).

## Current state

**All 19 tracker entries are now either tracked upstream, closed/merged, or retired (TR-18).** Upstream filing queue is empty; the remaining work is project-internal cleanup as upstream lands.

> **Phase 2 update (2026-07-21).** The project now builds against isolated
> `logos-scaffold` `7c52211a3f40a6ac5829905d4569712f414776ed`, which lands the
> `[circuits]` schema (TR-07), granular `lgs basecamp build` (TR-14), and the
> `lgpm cli-portable` install path. Concretely this means:
> - **Gate 1 (same-repo nested Nix path bug) closed** by using
>   `git+file:.?dir=swap-module#lgx` / `git+file:.?dir=swap-ui#lgx` module refs
>   in `scaffold.toml` (flakes unchanged; direct `nix build .#lgx` still works).
>   No Scaffold PR needed. Operational cost: any tracked-file edit changes the
>   `git+file:.` tree hash → next `lgs basecamp build`/`install` rebuilds
>   `swap` / `swap_ui` (~10 min cold).
> - **Circuits** are scaffold-owned via `[circuits] version = "0.4.2"`;
>   `lgs setup` fetches them to `.scaffold/circuits` (TR-07 landed).
> - **TR-03 RESOLVED** — `lgs basecamp install` installs the `#lgx-portable`
>   packages via `lgpm cli-portable` with zero variant errors; the
>   `extract_lgx_variant` workaround is obsolete. LGPM #14 needs no PR and will
>   be closed as stale post-merge.
> - **New finding (filed):** a macOS `lgs basecamp launch` `LOGOS_DATA_DIR` gap
>   (needs an absolute per-profile path; currently handled by the committed
>   app-owned launch bridge `scripts/basecamp-launch.sh`). Filed as
>   [scaffold#236](https://github.com/logos-co/scaffold/issues/236); fix up as
>   [PR #238](https://github.com/logos-co/scaffold/pull/238) (sets an absolute
>   `LOGOS_DATA_DIR` for the macOS portable stack; CI green, open/mergeable).
>   See the Phase 2 addendum in
>   [`docs/scaffold-upstream-tracker.md`](./scaffold-upstream-tracker.md#phase-2-addendum-2026-07-21).
> - **Correction:** `lgs run --profile demo` / `--profile test` do **not** work
>   in this repo — scaffold's deploy step hardcodes the deployable-program dir
>   as `methods/guest/src/bin`, but the guest program lives at
>   `programs/lez-htlc/methods/guest/`. The working headless paths are
>   `make demo-makefile` / `make test-makefile`. This supersedes the earlier
>   Phase-1 "`[run.profiles.{test,demo}]` partial adoption" note below; it is a
>   second new Scaffold ask (configurable program directory), now filed as
>   [scaffold#237](https://github.com/logos-co/scaffold/issues/237) with fix up
>   as [PR #239](https://github.com/logos-co/scaffold/pull/239) (adds
>   `deploy = false` on `[run]`/`[run.profiles.<name>]`, default true). Once #239
>   merges into a scaffold release we adopt, adding `deploy = false` to
>   `[run.profiles.demo]` / `[run.profiles.test]` re-enables `lgs run` here.
> - **Real atomic swap proven** via a clean standalone `make demo-makefile` on
>   2026-07-21: the full end-to-end swap completed (both peers `Completed`,
>   preimage revealed, ETH + LEZ claims). The GUI two-peer click-through remains
>   human-run; Basecamp module loading + delivery messaging are proven
>   separately (standalone `lgs basecamp run swap_ui` delivery-connect + offers
>   subscription, and the per-swap delivery coordination record in
>   [`delivery-dogfooding.md`](../delivery-dogfooding.md)).
>
> As a result the "Blocked on upstream" cleanups for circuits, `swap-lgx-build`,
> and the `basecamp-instance.sh` install path are now **unblocked** (see the
> updated table below).

> **Phase 2 close-out (2026-07-27).** The pin is now `logos-scaffold`
> `6789ec04b2ad256186a5894710c419b42d16e479` (scaffold master), which adds the
> merged [PR #238](https://github.com/logos-co/scaffold/pull/238) and
> [PR #239](https://github.com/logos-co/scaffold/pull/239). Adoption on this
> repo:
> - **`lgs basecamp launch` works on macOS without a bridge** — PR #238 makes
>   `launch` set an absolute per-profile `LOGOS_DATA_DIR`, so
>   `scripts/basecamp-launch.sh` is deleted; `lgs basecamp launch <profile>` is
>   the launch path on every platform.
> - **`lgs run` re-enabled** — `deploy = false` set on
>   `[run.profiles.{demo,test}]` (PR #239), so `make demo` / `make test` are now
>   the scaffold-native paths and `make demo-makefile` / `make test-makefile`
>   are deleted. The trailing localnet stop stays in the Make targets until
>   TR-19 / [scaffold#172](https://github.com/logos-co/scaffold/issues/172)
>   lands.
> - **`check-circuits` Makefile guard deleted** — superseded by scaffold
>   [PR #221](https://github.com/logos-co/scaffold/pull/221) (circuits
>   auto-materialize + `lgs doctor` check).
> - **Remaining app-owned glue:** `scripts/scaffold-setup.sh` (LEZ v0.2.0 repo
>   layout in `lgs setup`, tracked upstream as
>   [scaffold#240](https://github.com/logos-co/scaffold/issues/240) — during
>   this adoption it also grew a default-wallet seeding bridge, because the
>   v0.2.0 debug wallet config ships no `initial_accounts` and `lgs run`'s
>   mandatory topup step needs the `default_address=` wallet state scaffold's
>   setup cannot seed; the Makefile likewise exports `LEE_WALLET_HOME_DIR`
>   since scaffold subprocesses still export the older `NSSA_WALLET_HOME_DIR`
>   name);
>   `make infra` + Anvil orchestration (TR-06 /
>   [scaffold#172](https://github.com/logos-co/scaffold/issues/172)); and the
>   localnet-stop traps in `make demo` / `make test` (TR-19 /
>   [scaffold#172](https://github.com/logos-co/scaffold/issues/172)).
> - **Still no scaffold release tag** (TR-01 /
>   [scaffold#170](https://github.com/logos-co/scaffold/issues/170)) — the pin
>   remains a raw master SHA.

### Upstream filings (all 2026-05-22 unless noted)

| Tracker | Issue / PR | Repo |
|---|---|---|
| TR-01 + TR-02 (subsumed) | [#170](https://github.com/logos-co/scaffold/issues/170) | scaffold |
| TR-03 (primary) | [#14](https://github.com/logos-co/logos-package-manager/issues/14) | logos-package-manager |
| TR-03 (companion) | [#197](https://github.com/logos-co/logos-basecamp/issues/197) | logos-basecamp |
| U-A umbrella (TR-04, TR-05, TR-08, TR-12, TR-16, TR-17) | [#171](https://github.com/logos-co/scaffold/issues/171) | scaffold |
| U-B umbrella (TR-06, TR-19) | [#172](https://github.com/logos-co/scaffold/issues/172) | scaffold |
| U-C (TR-07) | [#173](https://github.com/logos-co/scaffold/issues/173) | scaffold |
| U-D umbrella (TR-10, TR-14, TR-15) | [#174](https://github.com/logos-co/scaffold/issues/174) | scaffold |
| TR-09 | [#175](https://github.com/logos-co/scaffold/issues/175) | scaffold |
| TR-20 | [#176](https://github.com/logos-co/scaffold/issues/176) | scaffold |
| TR-11 (doc PR) | [#177](https://github.com/logos-co/scaffold/pull/177) | scaffold |
| TR-13 (doc PR) | [#178](https://github.com/logos-co/scaffold/pull/178) | scaffold |

### Project-internal state

| Already done | In-flight | Pending decision |
|---|---|---|
| `scaffold.toml` upgraded to 0.2.0 schema + `[modules.*]` block added (swap, swap_ui, delivery_module) | [PR #26](https://github.com/logos-co/eth-lez-atomic-swaps/pull/26) — swap-vendor-ffi → Nix dev shell *(landed without approval — review needed)* | All Bucket 1 Makefile deletions |
| `docs/scaffold-upstream-tracker.md` — 19 entries (incl. TR-20), mental model, glossary, TOC | T-019e45fb — LMB-01 investigation (logos-module-builder upstream) | All Bucket 2 / 3 long-term deletions (wait on upstream) |
| `[run.profiles.{test,demo}]` blocks added to `scaffold.toml` in [PR #29](https://github.com/logos-co/eth-lez-atomic-swaps/pull/29) for Phase 1 of [eth-lez-atomic-swaps#27](https://github.com/logos-co/eth-lez-atomic-swaps/issues/27) — **but see the Phase 2 correction: `lgs run` is blocked by its hardcoded program dir, so these are declarative-only; use `make demo-makefile` / `make test-makefile`** — **update 2026-07-27: adopted with `deploy = false` at pin `6789ec04`; `make demo` / `make test` now run `lgs run`** |  |  |
| All 9 upstream filings done (see table above) | [logos-co/scaffold#169](https://github.com/logos-co/scaffold/pull/169) — narrow SPel public-pin fix (near landing) | All Bucket 2 / 3 long-term deletions (wait on upstream) |

## Upstream filing queue (scaffold)

Bundled where issues naturally compose. Each row has a copy-pasteable handoff
prompt sketch.

### P0 — status

| Tracker entry | Status | Notes |
|---|---|---|
| **TR-01** Cut `v0.2.0` tag | ✅ Filed as [#170](https://github.com/logos-co/scaffold/issues/170) | Scoped broader than original — adds bi-weekly cadence + acceptance criteria covering SPEL/LEZ matching releases, `lez-template` rename decision, and CI guardrails |
| **TR-02** Sweep default SHAs for public reachability | ✅ Subsumed by [#170](https://github.com/logos-co/scaffold/issues/170) | #170's acceptance criteria explicitly include "CI verifies scaffold's hardcoded default pins are public-reachable" — no separate issue needed |
| **TR-03** Align `bin-macos-app` and `lgpm` `LGPM_PORTABLE_BUILD` | ✅ Tracked cross-repo since 2026-05-22 | Primary [logos-package-manager#14](https://github.com/logos-co/logos-package-manager/issues/14) remains open; companion [logos-basecamp#197](https://github.com/logos-co/logos-basecamp/issues/197) is closed. Options A/B/C surfaced, manifest-mismatch loud-error sub-ask included. Awaiting maintainer pick on package-manager direction. |

Companion PR: [logos-co/scaffold#169](https://github.com/logos-co/scaffold/pull/169) — narrow SPel public-pin fix (commit-only pin), near landing.

#### Handoff prompt — TR-03
```
This is a cross-repo investigation. Clone logos-co/logos-basecamp +
logos-co/logos-package-manager to temp dirs. Read how LGPM_PORTABLE_BUILD is
wired in both repos. Draft an issue (in logos-package-manager probably, with
cross-link to logos-basecamp) proposing either:
  (a) aligning both binaries on the same build mode, OR
  (b) teaching lgpm install to derive variant from the consumer's
      PackageManagerLib build mode.
Surface design options with tradeoffs. Reference
docs/scaffold-upstream-tracker.md#tr-03 and delivery-dogfooding.md's variant
mismatch section in the eth-lez-atomic-swaps repo. Don't push or create the
issue without approval.
```

### P1 — file in batches (4 umbrella issues, ~10 entries)

| Umbrella issue | Tracker entries | Why bundle |
|---|---|---|
| **U-A: `[basecamp.profiles.*]` schema** ✅ [#171](https://github.com/logos-co/scaffold/issues/171) | TR-04, TR-05, TR-08, TR-12, TR-16, TR-17 | Filed 2026-05-22 as umbrella with six labeled sub-asks. Reviewer to call subsume-vs-split on overlap with [#163](https://github.com/logos-co/scaffold/issues/163) (ask 1) and [#89](https://github.com/logos-co/scaffold/issues/89) (ask 2). |
| **U-B: `lgs run` pipeline extensions** ✅ [#172](https://github.com/logos-co/scaffold/issues/172) | TR-06, TR-19 | Filed 2026-05-22 as umbrella with three labeled sub-asks (`pre_localnet`, coprocess hooks, `stop_on_exit`). Two coprocess design shapes surfaced for maintainer pick. |
| **U-C: `[circuits]` schema** ✅ [#173](https://github.com/logos-co/scaffold/issues/173) | TR-07 | Filed 2026-05-22. Body proposes `[circuits]` schema + `lgs setup` auto-fetch + `lgs doctor` check + auto-export of `LOGOS_BLOCKCHAIN_CIRCUITS`. |
| **U-D: `lgs basecamp` verb granularity** ✅ [#174](https://github.com/logos-co/scaffold/issues/174) | TR-10, TR-14, TR-15 | Filed 2026-05-22 as umbrella with three labeled sub-asks (`build`, `--variant` filter, `run <module>`). Verb-naming decision (`build` + flags vs extend `build-portable`) flagged for reviewer. |

#### Handoff prompt template (umbrella, e.g. U-A)
```
File an umbrella issue at logos-co/scaffold proposing a
`[basecamp.profiles.<name>]` schema that solves these P1 tracker entries
together: TR-04 (macOS XDG_RUNTIME_DIR short-path), TR-05 (per-profile env
files), TR-08 (per-platform basecamp attr), TR-12 (launch --log-file),
TR-16 (lgs basecamp paths <profile>), TR-17 (configurable profile names).
Source-of-truth is docs/scaffold-upstream-tracker.md in
/Users/danisharora099/Developer/status/eth-lez-atomic-swaps/. Draft the
umbrella body listing the entries with one-line summaries + file/line links to
the eth-lez-atomic-swaps pain points. Surface for approval before
`gh issue create`. Recommend whether to file as one umbrella + sub-issues or
as N separate linked issues based on what scaffold's existing issue
conventions look like (check open issues first).
```

(Same shape for U-B, U-C, U-D — swap the entries list and umbrella concept.)

### P2 — backlog (3 issues + 3 doc PRs)

| Item | Type | Notes |
|---|---|---|
| **TR-09** ✅ [#175](https://github.com/logos-co/scaffold/issues/175) | Issue | `lgs run --watch` debounce + globs — closed |
| **TR-11** ✅ [#177](https://github.com/logos-co/scaffold/pull/177) | Doc PR | Hand-authored `[modules.*]` tables blessed — merged |
| **TR-13** ✅ [#178](https://github.com/logos-co/scaffold/pull/178) | Doc PR | `--user-dir` vs XDG isolation cross-ref — merged |
| **TR-20** ✅ [#176](https://github.com/logos-co/scaffold/issues/176) | Issue | `lgs basecamp develop <module>` — closed |

## Project-internal cleanup queue (separate from upstream)

### Now-doable (no upstream blocker)

| Item | Effort | Handoff prompt sketch |
|---|---|---|
| **Bucket 1 deletions:** localnet-{start,stop}, swap-module-build, swap-ui-build, swap-ui-run, basecamp-paths-* | ~30 min | "Delete Bucket 1 Makefile targets per Bucket 1 analysis in docs/scaffold-upstream-tracker.md + this plan doc. Update README to point at `lgs localnet`/`nix build`/`nix run` invocations. Verify `make` with no args still lists remaining targets. Don't push without approval." |
| **Add `[run.profiles.{test,demo}]` partial** | ✅ **DONE (2026-07-27)** | Blocks added in Phase 1 of [eth-lez-atomic-swaps#27](https://github.com/logos-co/eth-lez-atomic-swaps/issues/27), but `lgs run --profile demo`/`test` fails here: scaffold's deploy step hardcodes `methods/guest/src/bin` while the guest program lives at `programs/lez-htlc/methods/guest/`. Filed as [scaffold#237](https://github.com/logos-co/scaffold/issues/237) with fix up as [PR #239](https://github.com/logos-co/scaffold/pull/239) (`deploy = false` on `[run]`/`[run.profiles.<name>]`); adding `deploy = false` to `[run.profiles.demo]`/`[run.profiles.test]` re-enables `lgs run` here once #239 lands in an adopted release. Working headless paths remain `make demo-makefile` / `make test-makefile`; the blocks are declarative-only until then. [PR #239](https://github.com/logos-co/scaffold/pull/239) merged and was adopted 2026-07-27 at pin `6789ec04`: `deploy = false` set on both profiles, `make demo` / `make test` now call `lgs run`, and the `-makefile` fallbacks are deleted. |
| **PR #26 review/merge** | ~10 min | Already landed; review the diff. Force-update if needed. |

### Cleanup status (updated Phase 2, 2026-07-21)

| Cleanup | Unblocked by | Status |
|---|---|---|
| Delete `make circuits` (68 lines); keep only the `CIRCUITS_DIR` + `LOGOS_BLOCKCHAIN_CIRCUITS` bridge | TR-07 | ✅ **Unblocked** — scaffold `7c52211` `[circuits]` + `lgs setup` fetch. Ready for the Makefile cleanup pass. |
| Delete `make swap-lgx-build` (+ `swap-module-build` / `swap-ui-build` / `swap-ui-run`) | TR-10 + TR-14 | ✅ **Unblocked** — replaced by `lgs basecamp build` / `lgs basecamp run swap_ui`. |
| Delete `scripts/basecamp-instance.sh` + `make basecamp-{init,run,clean}-*` | TR-03 + TR-04 + TR-05 + TR-08 + TR-12 + TR-16 + TR-17 | ✅ **DONE** — `basecamp-instance.sh` and all `basecamp-*` Makefile targets deleted; `lgs basecamp setup`/`install` own the install path (TR-03 resolved). The interim committed launch bridge `scripts/basecamp-launch.sh` (for the macOS `LOGOS_DATA_DIR` gap) was deleted 2026-07-27 after [scaffold PR #238](https://github.com/logos-co/scaffold/pull/238) merged and the pin moved to `6789ec04`. |
| Gut `src/cli/infra.rs` + delete `make infra` | TR-06 | Blocked — Anvil co-process hook not yet in scaffold. |
| Delete `make test` / `make demo` entirely | TR-06 + TR-07 + TR-19 | Partially done — `lgs run` adoption landed 2026-07-27 ([PR #239](https://github.com/logos-co/scaffold/pull/239) profiles + `deploy = false`); the thin Make wrappers remain only for the trailing localnet stop (TR-19 / [#172](https://github.com/logos-co/scaffold/issues/172)) and Anvil stays app-owned (TR-06); full deletion still blocked on [#172](https://github.com/logos-co/scaffold/issues/172). |

## Sequencing

```diagram
╭───────────────────────────────────────────────────────────────╮
│  DONE                                                         │
│  ────                                                         │
│  ✓ scaffold.toml 0.1.1 → 0.2.0 upgrade + [modules.*] seeded   │
│  ✓ Tracker + plan docs landed on master                       │
│  ✓ TR-01 filed (#170, also subsumes TR-02)                    │
│  ✓ PR #169 in flight (narrow SPel public-pin fix)             │
│  ✓ [run.profiles.{test,demo}] partial adopted for Phase 1      │
╰───────────────────────────────────────────────────────────────╯
                              │
                              ▼
╭───────────────────────────────────────────────────────────────╮
│  THIS WEEK                                                    │
│  ─────────                                                    │
│  1. File TR-03 upstream                         (1 handoff)   │
│  2. Bucket 1 Makefile deletions                 (1 handoff)   │
│  3. Review PR #26 (swap-vendor-ffi)             (manual)      │
│  4. LMB-01 investigation result review          (manual)      │
╰───────────────────────────────────────────────────────────────╯
                              │
                              ▼
╭───────────────────────────────────────────────────────────────╮
│  NEXT 2-4 WEEKS                                               │
│  ──────────────                                               │
│  5. File P1 umbrellas U-A, U-B, U-C, U-D        (4 handoffs)  │
│  6. File P2 backlog as time permits             (1-4 handoffs)│
╰───────────────────────────────────────────────────────────────╯
                              │
                              ▼
╭───────────────────────────────────────────────────────────────╮
│  AS EACH UPSTREAM ENTRY LANDS                                 │
│  ────────────────────────────                                 │
│  8. Delete the corresponding project-internal piece           │
│     (one PR per upstream landing, surgical)                   │
╰───────────────────────────────────────────────────────────────╯
```

## Out of scope (intentionally)

- **Switching to Path A (dev stack).** Off the table per dogfooding fidelity commitment.
- **TR-18.** Retired — Nix dev shells are the right layer, not scaffold.
- **Replacing `make contracts` / `make demo` Solidity orchestration with scaffold.** Foundry is not scaffold's domain.
- **Anything touching `[repos.lez].pin` or `[repos.spel].pin`** — intentional divergence; coordinate first.

## Cross-references

- [`docs/scaffold-upstream-tracker.md`](./scaffold-upstream-tracker.md) — full tracker, 19 entries, mental model + glossary
- [`delivery-dogfooding.md`](../delivery-dogfooding.md) — original dogfooding findings; some tracker entries cite specific sections
- [PR #26](https://github.com/logos-co/eth-lez-atomic-swaps/pull/26) — swap-vendor-ffi → Nix dev shell (landed)
- [logos-co/scaffold#169](https://github.com/logos-co/scaffold/pull/169) — narrow SPel public-pin fix (companion to TR-02)
- [logos-co/scaffold#170](https://github.com/logos-co/scaffold/issues/170) — v0.2.0 release tag + bi-weekly cadence (TR-01, subsumes TR-02)
- Thread T-019e4537-ee65-715f-9117-a126eb3b2e56 — the conversation that produced this plan
- Thread T-019e45fb-5eb1-74ea-8e25-612703031f87 — LMB-01 investigation (in-flight)
