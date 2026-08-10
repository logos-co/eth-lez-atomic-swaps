# Golden-path canary (stage 1: in-repo)

A continuously-run pipeline that turns protocol, package, and release-catalog
stages into assertions for the LEZ ↔ ETH atomic-swaps stack: boot a localnet →
prove a valid tx reaches finality → prove a deliberately-invalid tx is loudly
rejected → build the shippable modules (darwin-arm64, linux-amd64,
linux-arm64) → verify the live release catalog and confirm the module install
artifact.

This is supporting regression coverage, not end-user Basecamp acceptance. In
particular, the `swap` leg runs the headless CLI demo and never launches
Basecamp. Basecamp UI acceptance must install the modules and exercise them in
the real pinned Basecamp host; see the repository README's
[**Manual Basecamp Run**](../README.md#manual-basecamp-run) section and the
Basecamp-native smoke work tracked in
[PR #90](https://github.com/logos-co/eth-lez-atomic-swaps/pull/90).

> **Stage 1** lives inside `logos-co/eth-lez-atomic-swaps` on purpose: zero new
> repos, zero new permissions. It graduates to its own repo later (see
> [Graduation plan](#graduation-plan)).

## The legs

Each leg is a self-contained script under `canary/` that emits exactly one
machine-readable result line:

```
CANARY_RESULT {"leg":"chain","status":"red","evidence":"…","duration_s":42}
```

| Leg | Script | What it proves | Cost / platform |
|-----|--------|----------------|-----------------|
| **chain** | `leg-chain.sh` | On a LEZ localnet: a **valid** typed transfer is accepted, included, and (funded) actually moves balance; and a **deliberately-invalid** `bare-u128` transfer is **loudly rejected**. | localnet + v0.2.2 toolchain |
| **swap** | `leg-swap.sh` | A headless **two-peer protocol swap** completes: both CLI peers report `Completed` with the **same preimage** (the atomicity invariant). Wraps `make demo`; does not test Basecamp. | heaviest: localnet + risc0 + Anvil |
| **modules** | `leg-modules.sh` | Both Basecamp modules still build to a portable `.lgx`: `nix build .#lgx-portable` for `swap-module` and `swap-ui`. | **darwin-arm64, linux-amd64, linux-arm64** (see below) |
| **catalog** | `leg-catalog.sh` | The **live** release catalog chain is intact: `logos-repo.json → index.json →` each `.lgx` asset URL, with names/versions cross-checked against each module's `metadata.json`. | cheap, network-only, any OS |
| **release-content** | `leg-release-content.sh` | A **published release artifact actually contains the code its version claims**: downloads the released `.lgx`, extracts it, and asserts committed content markers (`release-content-expectations.json`) in **every shipped variant**, plus the release tag's **ancestry**. Catches a release dispatched on a **stale ref** — it happened twice (swap_ui 0.3.0 without the History tab, 0.3.3 without PR #94's trial-feedback flow) and the catalog leg structurally cannot see it (it never downloads a byte). Also runs synchronously on each `release: published` via `verify-release.yml`. | cheap, network-only, any OS |

Run them all (or a subset) with the orchestrator, which prints a summary table
and writes a status JSON:

```sh
canary/run-all.sh                          # all legs
canary/run-all.sh catalog release-content  # the cheap+fast subset
```

## Red-light policy — a failing leg is a *signal*, not a canary bug

The canary is **born with a legitimate red light.** The chain leg asserts that a
malformed instruction is *loudly rejected*; today the LEZ v0.2.2 sequencer
**silently accepts and drops it**
([logos-blockchain/logos-execution-zone#640](https://github.com/logos-blockchain/logos-execution-zone/issues/640)).
That assertion fails **on purpose** — it is the canary doing its job, surfacing a
real upstream bug the atomic-swaps team lost hours to during the v0.2.2
migration (fixed on our side in commit `df93c67` by switching to the typed
`authenticated_transfer_core::Instruction::Transfer { amount }`).

So the exit codes distinguish **"the ecosystem changed"** from **"the canary
broke"**:

| Status | Exit | Meaning | CI treatment |
|--------|------|---------|--------------|
| `pass` | 0 | The leg proved its property. | green |
| `red` | 10 | An **expected ecosystem red light** — a known upstream bug reproduced (e.g. #640). | surfaced, **not** a hard failure |
| `fail` | 20 | The assertion failed **unexpectedly** — a real regression to investigate. | fail |
| `broken` | 30 | The canary **could not run** the check (no localnet, missing toolchain, network down). | fix the canary/infra |

`run-all.sh` exits with the worst severity seen. A `red` light must never page
someone as if it were `broken` or `fail`: when #640 is fixed upstream, the chain
leg flips to `pass` and the canary tells you the golden path got *better*.

## Running the chain leg locally (the money shot)

The chain leg's heavy lifting is `canary/chain-probe`, a standalone Rust binary
pinned to the **same v0.2.2 (`d6e4ae69`) LEZ deps** as the app, so its client
speaks the same wire/program-id version as the sequencer and the public testnet.
It:

1. checks sequencer health and **program-id compatibility** (client v0.2.2 vs the
   running sequencer's pin — a mismatch means the localnet is a different LEZ
   version and the experiment would be confounded, so it reports `broken`);
2. funds two debug-genesis accounts from the vault and initializes them under
   `authenticated_transfer`;
3. submits a **valid** typed transfer → asserts accepted + included + balance
   moved;
4. submits the **`bare-u128`** transfer (#640's exact payload) → asserts it is
   loudly rejected.

Point it at any running localnet:

```sh
# fastest path: reuse a prebuilt sequencer_service + the checked-in debug config
CANARY_START_LOCALNET=1 canary/run-all.sh chain
# or target an already-running sequencer:
CANARY_LEZ_RPC=http://127.0.0.1:3040 canary/run-all.sh chain
```

`canary/lib/localnet.sh` boots a **throwaway** localnet without touching any
developer's `.scaffold` state: it runs a prebuilt `sequencer_service` (found via
`$CANARY_SEQ_BIN`, a sibling scaffold cache, or this repo's own cache) against
the checked-in debug genesis config in a fresh `/tmp` home, in `RISC0_DEV_MODE`,
on a free port. If no prebuilt binary exists it falls back to
`logos-scaffold localnet start` (canonical, but requires the full toolchain).

## CI

`.github/workflows/canary.yml` — nightly cron + `workflow_dispatch`:

- **`catalog`** on `ubuntu-latest`: cheap, portable, always on.
- **`modules`** — a `fail-fast: false` matrix across `macos-latest`
  (darwin-arm64), `ubuntu-latest` (linux-amd64), and `ubuntu-24.04-arm`
  (linux-arm64): `nix build .#lgx-portable` on each. `swap-module`/`swap-ui`
  carry real pinned hashes for all three (issue #32 / PR #53); a system
  outside those three (e.g. x86_64-darwin) still reports `broken` — see
  `swap-module/flake.nix`.
- **`localnet-legs`** (chain + swap): **opt-in** via `workflow_dispatch`, and
  `continue-on-error`. Honest reason: booting a localnet needs the
  `sequencer_service` binary built from the LEZ v0.2.2 repo (a 20–30 min cold
  Rust build), and the swap leg also needs risc0 + Anvil + the app's HTLC guest.
  GitHub-hosted runners are ephemeral, so a cold toolchain build every night is
  wasteful and flaky. **TODO(self-hosted):** move this job to a self-hosted
  `[macos, arm64]` runner that keeps `.scaffold/lez-cache` warm (or ships a
  prebuilt `sequencer_service` the canary reuses — `lib/localnet.sh` already
  supports both), then enable it by default.

All jobs cache aggressively (magic-nix-cache for the nix store, `rust-cache` for
cargo) and upload a `canary-status-*.json` artifact; a final `summary` job rolls
them into one job-summary table via `canary/summarize.py`.

## Canary channel — install a branch build in Basecamp in minutes

`.github/workflows/canary-channel.yml` is a **fast, unofficial install channel**
for branch builds. It lets you test an unreleased branch in the real Basecamp
host **~15-25 min** after CI compiles it, instead of the full official-release
path (**~2.5 h**: two `release-swap*.yml` dispatches, a three-variant matrix
each, an index rebuild, and a human version bump).

**Use it:**

1. **Dispatch** `canary-channel.yml` on your branch (Actions → Canary channel →
   Run workflow). Inputs: `ref` (branch/SHA, default `master`), `platform`
   (default `darwin-arm64` — single-platform is the speed win; also
   `linux-amd64`, `linux-arm64`, `all`), `modules` (`both` | `swap` | `swap_ui`).
2. The run **builds** the requested module(s) with the same
   `nix build .#lgx-portable` the release uses, **byte-scans** each `.lgx`
   against `release-content-expectations.json`'s highest markers **before
   publishing anything** (a build that doesn't contain the code it claims fails
   the job), then **publishes** to the rolling `canary` prerelease alongside a
   `canary-index.json` shaped exactly like the official `index.json`.
3. In Basecamp, **add a second repository** pointing at the canary descriptor's
   raw URL:
   ```
   https://raw.githubusercontent.com/logos-co/eth-lez-atomic-swaps/master/logos-repo-canary.json
   ```
   Then **update the modules from it.** The job summary prints the exact release
   URL, the assets published, and this line.

**Never point public testers at this channel.** It ships unstable, unsigned,
in-place-replaced branch builds.

**Version scheme (deliberate):** the canary publishes as the sentinel
`0.99.<run-number>`, a plain dotted-integer string — *not* a semver prerelease
like `0.4.3-canary.<sha>`. The sentinel sorts above every real `0.4.x` under
both strict-semver and the loose "extract the integers" comparator this repo's
own `leg-catalog.sh` uses, so Basecamp **always** offers the canary as the
newest available regardless of what's installed; `<run-number>` increases every
dispatch so re-runs always re-offer. A `0.4.3-canary.<sha>` prerelease would
instead sort *below* the eventual real `0.4.3` and inject stray integers from a
hex shortsha into a loose comparator. The real provenance (ref + short SHA) is
kept in the asset filename, each index entry's `publisherRef`, and the summary.

**Two local scripts back the workflow** (both runnable on any `.lgx`, no
network):

| Script | Role |
|--------|------|
| `canary/canary-content-scan.py` | Local-file twin of `leg-release-content.sh`: byte-scans a built `.lgx` against the module's highest `release-content-expectations.json` markers, over **every variant present** (a single-platform canary carries one). Gates the publish. |
| `canary/canary-index.py` | `emit-entry` turns one `.lgx` + its URL into an index fragment; `assemble` groups fragments into a full schemaV2 `canary-index.json`. |

**First-run limitation:** `workflow_dispatch` only lists a workflow that exists
on the **default branch**, so the first canary dispatch is possible only *after*
this workflow merges to `master`. (A future `pull_request` label trigger could
canary a branch pre-merge — out of scope here.)

## Graduation plan

Stage 1 proves the concept in-repo. Next:

1. **Own repo** (`logos-co/golden-path-canary`): move `canary/` out, keep the
   `chain-probe` crate, add the app + toolchain as pinned inputs. The legs are
   already repo-relative and status-JSON-driven, so the lift is mechanical.
2. **Release-gate wiring**: make the `catalog` + `modules` legs a required check
   before publishing a new `.lgx` release — the canary becomes the gate that
   stops a broken catalog or unbuildable module from shipping.
3. **Self-hosted runner** with a warm `.scaffold` so the localnet legs run
   nightly by default (see the CI TODO above).
4. **Discord webhook** — **TODO (needs a secret).** On a status *transition*
   (e.g. chain leg `red → pass` when #640 lands, or any leg → `fail`/`broken`),
   POST the summary table to a Discord channel. Requires a `DISCORD_WEBHOOK_URL`
   repo/org secret; left as a documented TODO so no secret is invented here. The
   status JSON already carries everything the webhook payload needs.
