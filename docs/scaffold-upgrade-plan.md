# Scaffold Upgrade Plan

Captures the upstream filing queue + project-internal cleanup queue produced by
the `scaffold.toml` 0.1.1 → 0.2.0 upgrade pass on this repo. Use this as the
resume point.

For mental model + per-entry rationale, see
[`docs/scaffold-upstream-tracker.md`](./scaffold-upstream-tracker.md).

## Current state

**All 19 tracker entries are now either tracked upstream, closed/merged, or retired (TR-18).** Upstream filing queue is empty; the remaining work is project-internal cleanup as upstream lands.

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
| `[run.profiles.{test,demo}]` partial adoption done in Phase 1 of [eth-lez-atomic-swaps#27](https://github.com/logos-co/eth-lez-atomic-swaps/issues/27); validation recorded in [`docs/scaffold-phase-1-validation.md`](./scaffold-phase-1-validation.md) |  |  |
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
| **Add `[run.profiles.{test,demo}]` partial** | Done | Completed in Phase 1 of [eth-lez-atomic-swaps#27](https://github.com/logos-co/eth-lez-atomic-swaps/issues/27). `demo` uses `cargo run --features demo -- demo --no-localnet` so scaffold owns the LEZ run pipeline while Anvil/Ethereum deployment remain app-owned. |
| **PR #26 review/merge** | ~10 min | Already landed; review the diff. Force-update if needed. |

### Blocked on upstream (wait for tracker entries to land)

| Cleanup | Unblocked by |
|---|---|
| Delete `make circuits` (68 lines) | TR-07 |
| Delete `make swap-lgx-build` | TR-10 + TR-14 |
| Delete `scripts/basecamp-instance.sh` + `make basecamp-{init,run,clean}-*` | TR-03 + TR-04 + TR-05 + TR-08 + TR-12 + TR-16 + TR-17 |
| Gut `src/cli/infra.rs` + delete `make infra` | TR-06 |
| Delete `make test` / `make demo` entirely | TR-06 + TR-07 + TR-19 |

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
