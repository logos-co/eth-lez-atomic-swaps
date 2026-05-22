# Scaffold Upstream Tracker

Backlog of candidate issues / PRs to file against
[`github.com/logos-co/scaffold`](https://github.com/logos-co/scaffold). Each
entry is grounded in concrete pain encountered while upgrading
`eth-lez-atomic-swaps` from scaffold `v0.1.1` to the 0.2.0 schema against
`logos-scaffold` master `d50caf4f86f2a913f9dc9985fb2a80f06a3e30d8`
(41 commits ahead of `v0.1.1`).

Related upstream work already filed:

- [logos-co/scaffold#170](https://github.com/logos-co/scaffold/issues/170) —
  cut `v0.2.0` tag + bi-weekly release cadence + acceptance criteria (TR-01,
  subsumes TR-02 via its CI-guardrail criterion).
- [logos-co/scaffold#169](https://github.com/logos-co/scaffold/pull/169) —
  narrow SPel public-pin fix (commit-only pin), near landing; does not affect
  this project's pinned divergences.

## How to read this doc

Every entry follows the same template:

- **TL;DR** — one-sentence summary; read this if you're skimming.
- **Why this hurts us** — concrete file/line where the pain lives.
- **Suggested fix** — the smallest scaffold change that would let us delete that pain.
- **Priority** — P0 / P1 / P2 (see legend).
- **Details** — full rationale, mechanics, history; skip unless you want the receipts.

**Priority legend:**

| | Meaning |
|---|---|
| **P0** | Blocks any downstream basecamp project today. File first. |
| **P1** | Large quality-of-life win; would let this project delete substantial hand-rolled code. |
| **P2** | Ergonomics / docs; nice-to-have. |

---

## The mental model (why most of these exist)

Almost every entry in this tracker traces back to one architectural fact about Logos modules. Read this once and the rest of the doc makes sense.

There are **two compile-time-incompatible stacks** for running Basecamp + Logos modules:

```diagram
     COMPILE-TIME FLAG: LOGOS_PORTABLE_BUILD
                  (per-binary)
            │                       │
       ╭────┴────╮             ╭────┴─────╮
       │   OFF   │             │    ON    │
       │  "dev"  │             │"portable"│
       ╰────┬────╯             ╰────┬─────╯
            │                       │
            ▼                       ▼
    DEV STACK                   DISTRIBUTED STACK
   ─────────────────         ─────────────────────────
   host:    #app             host:     #bin-macos-app | #bin-appimage
   install: lgpm             install:  manual extract (lgpm refuses)
   modules: nix build .#lgx  modules:  nix build .#lgx-portable

   reads variant             reads variant
   "<host>-dev"              "<host>"

   ✅ scaffold supports      ❌ scaffold has gaps
      this today                (most of this tracker)
```

**Dev stack** = the binary scaffold builds for local development. Fast, Nix-managed, depends on `/nix/store` paths at runtime. What `lgs basecamp setup` / `install` / `launch` natively work with.

**Distributed stack** = the binary end users actually run. Self-contained (Qt bundled, no `/nix/store` deps), platform-native packaging (`.app` on macOS via `bin-macos-app`, AppImage on Linux via `bin-appimage`). What this project ships and dogfoods against.

This project is on the distributed stack because dogfooding fidelity is the entire point. Every "scaffold doesn't quite work for us" entry below is a consequence of that choice meeting scaffold's dev-stack-first assumptions.

## Glossary

Terms used throughout this doc:

| Term | Meaning here |
|---|---|
| **Flake** | A Nix unit of reproducible build configuration. Inputs (other flakes, pinned by SHA) + outputs (packages, apps, dev shells). |
| **Pin / SHA** | A specific git commit a flake or `scaffold.toml` depends on, hard-coded so behaviour never drifts between machines. |
| **LGX** | A `.lgx` tarball with `manifest.json` + `variants/<host>/` dirs. The Logos module package format. |
| **Variant** | The string a host uses to pick the right `variants/<X>/` dir inside an LGX. `darwin-arm64-dev` (dev stack) or `darwin-arm64` (distributed stack). |
| **`lgpm`** | The Logos package manager CLI (`logos-package-manager`). Installs `.lgx` files into a host's module/plugin dirs. Speaks dev variants only. |
| **XDG basedir** | Linux/Unix per-user-state convention: `XDG_CONFIG_HOME`, `XDG_DATA_HOME`, `XDG_CACHE_HOME`, `XDG_RUNTIME_DIR`. Per-profile XDG roots = how two Basecamp instances run side-by-side without sharing identity/messages/keys. |
| **`sun_path` 104-byte budget** | macOS's hard cap on Unix-domain-socket path length. liblogos socket names like `<runtime>/logos_token_<module>_<pid>` overflow any deep `XDG_RUNTIME_DIR`. |
| **`[modules.*]`** | scaffold 0.2.0 TOML block declaring which module flakes belong to the project. Read by `lgs basecamp install` / `build-portable` / `doctor`. |
| **`[run.profiles.*]`** | scaffold 0.2.0 TOML block declaring named "build → localnet → topup → deploy → post-deploy hook" pipelines. Selected with `lgs run --profile <name>`. |
| **Post-deploy hook** | A shell command scaffold runs after `lgs deploy` succeeds, with env vars like `SCAFFOLD_PROGRAM_ID_<name>`, `NSSA_WALLET_HOME_DIR`, `SEQUENCER_URL` auto-injected. |
| **`risc0` guest program** | The `programs/lez-htlc/methods/guest/` Rust binary that compiles to a RISC-V image and runs inside the RISC Zero ZK-VM. `lgs deploy` ships this to the LEZ sequencer. |
| **`cdylib`** | A Rust dynamic library (`.dylib`/`.so`) exposing a C ABI. This project's `swap-ffi` produces one; `swap-module` (universal C++) links against it. |
| **`Anvil`** | A local Ethereum dev node (part of Foundry). `make infra` runs it in-process via `alloy::node_bindings::Anvil`. |

---

## Table of contents

| ID | Title | Priority |
|---|---|---|
| [TR-01](#tr-01) | Cut a `v0.2.0` release tag | P0 |
| [TR-02](#tr-02) | Sweep every hardcoded SHA for public reachability | P0 |
| [TR-03](#tr-03) | Align `bin-macos-app` and `lgpm` on the same `LGPM_PORTABLE_BUILD` mode | P0 |
| [TR-04](#tr-04) | `lgs basecamp launch` exposes macOS `XDG_RUNTIME_DIR` short-path override | P1 |
| [TR-05](#tr-05) | Per-profile env files in `[modules.*]` / `[basecamp.profiles.*]` | P1 |
| [TR-06](#tr-06) | `lgs run` gains pre-localnet / co-process hooks (for Anvil) | P1 |
| [TR-07](#tr-07) | `[circuits]` schema + `lgs setup` auto-fetch | P1 |
| [TR-08](#tr-08) | Per-platform `[repos.basecamp].attr` map (`bin-macos-app` darwin, `bin-appimage` linux) | P1 |
| [TR-09](#tr-09) | `lgs run --watch` debounce + glob filters | P2 |
| [TR-10](#tr-10) | `lgs basecamp build-portable` should optionally also build `#lgx` | P2 |
| [TR-11](#tr-11) | Document `[modules.*]` is hand-editable without `lgs basecamp modules` | P2 |
| [TR-12](#tr-12) | `lgs basecamp launch --log-file` (or default tee) | P2 |
| [TR-13](#tr-13) | Document `--user-dir` vs scaffold's XDG isolation | P2 |
| [TR-14](#tr-14) | `lgs basecamp build` (default `#lgx` variant, granular per module) | P1 |
| [TR-15](#tr-15) | `lgs basecamp run <module> [--host standalone\|basecamp]` | P1 |
| [TR-16](#tr-16) | `lgs basecamp paths <profile>` (debug introspection) | P2 |
| [TR-17](#tr-17) | Configurable basecamp profile names (`maker`/`taker` instead of `alice`/`bob`) | P1 |
| [TR-19](#tr-19) | `lgs run` `stop_on_exit` + `pre_localnet` hook stages | P1 |
| [TR-20](#tr-20) | `lgs basecamp develop <module>` for verb-set symmetry | P2 |

(TR-18 was retired — see [Retired entries](#retired-entries) at the bottom.)

---

## TR-01

### Cut a `v0.2.0` release tag — P0

**Status.** ✅ Filed as [logos-co/scaffold#170](https://github.com/logos-co/scaffold/issues/170). Scoped broader than the original ask — issue body covers v0.2.0 tag + bi-weekly cadence + acceptance criteria (matching SPEL/LEZ releases, `lez-template` rename decision, CI guardrails for version mismatches and pin reachability).

**TL;DR.** `v0.1.1` has no `basecamp` subcommand at all. Every Logos basecamp project pins scaffold by master SHA in its README. Tag a release so projects can pin a version string.

**Why this hurts us.** [`README.md` lines 102-109](../README.md#L102-L109) hard-codes the install SHA `d50caf4f...`.

**Suggested fix.** Tag `v0.2.0` capturing the 0.2.0 schema + `lgs basecamp` + `lgs run` surfaces.

<details>
<summary>Details</summary>

Every downstream consumer pins scaffold from master today because the 0.2.0 `scaffold.toml` schema migrator (`src/config.rs` parser, `src/migrate.rs`) and the `lgs basecamp{setup,modules,install,launch,build-portable,doctor,docs}` + `lgs run` + `[run.profiles.*]` surfaces are all post-`v0.1.1`. Pinning master SHAs is fragile and undermines `cargo install --locked`'s usefulness. A `v0.2.0` tag captures the actual usable surface.

</details>

---

## TR-02

### Sweep every hardcoded SHA for public reachability — P0

**Status.** ✅ Subsumed by [logos-co/scaffold#170](https://github.com/logos-co/scaffold/issues/170). Its acceptance criteria explicitly include *"CI verifies scaffold's hardcoded default pins are public-reachable"* — no separate issue needed. PR [#169](https://github.com/logos-co/scaffold/pull/169) is the companion narrow code-fix that triggered the broader filing.

**TL;DR.** PR #169 fixed one default SHA. Add CI that catches all the others before a fresh clone hits them.

**Why this hurts us.** A downstream that removes a local pin override (e.g. this project's intentional LEZ/spel divergences) immediately depends on whatever scaffold's defaults resolve to. If those SHAs aren't public-reachable, fresh clones break.

**Suggested fix.** A `cargo xtask` (or GitHub Action) that runs `git ls-remote` against every default SHA in `src/constants.rs` and fails CI if any is unreachable.

<details>
<summary>Details</summary>

`/tmp/logos-scaffold/src/constants.rs` contains, at minimum:

- `DEFAULT_BASECAMP_PIN = "a746cdbc521f72ee22c5a4856fd17a9802bb9d69"`
- LEZ default (currently `35d8df0d...`)
- Spel default (post-#169 fix)
- A `BASECAMP_DEPENDENCIES` table with several module pins (e.g. the `delivery_module` default `1fde156629...` that `lgs basecamp doctor` flagged as drift for this project).

Any of these going dark in a public-reachability sense silently breaks a fresh `lgs new <name>` invocation. The fix is mechanical (a CI loop) and one-off.

</details>

---

## TR-03

### Align `bin-macos-app` and `lgpm` on the same `LGPM_PORTABLE_BUILD` mode — P0

**TL;DR.** `bin-macos-app` (the released AppImage-style Basecamp) accepts only `#lgx-portable`. The host-installed `lgpm` CLI accepts only `#lgx`. So `lgs basecamp install` is unusable on the distributed stack — we have to bypass `lgpm` with a hand-rolled shell extractor.

**Why this hurts us.**
- 60+ lines of `extract_lgx_variant` in [`scripts/basecamp-instance.sh` lines 113-178](../scripts/basecamp-instance.sh#L113-L178) — exists solely to bypass `lgpm` on macOS.
- Full root-cause in [`scripts/basecamp-instance.sh` lines 199-250](../scripts/basecamp-instance.sh#L199-L250).
- Also captured in [`delivery-dogfooding.md`: "bin-macos-app / lgpm variant flavor mismatch"](../delivery-dogfooding.md#L241-L325).

**Suggested fix.** Pick one:
- Compile `bin-macos-app` and `lgpm` with the same `LGPM_PORTABLE_BUILD` setting in the Logos release pipeline, OR
- Teach `lgpm install` to derive the install variant from the consumer's PackageManagerLib build mode (not from the package name), so either `#lgx` or `#lgx-portable` works in either host.

Also: have `bin-macos-app`'s PackageManagerLib log a loud error when it scans a `manifest.json` with no matching `main[*]` variant — today the module is silently dropped, and failure only surfaces downstream as `Cannot load unknown module`.

<details>
<summary>Details</summary>

The mechanics, as built today:

- **`bin-macos-app`** (released Basecamp on macOS) is compiled with `LGPM_PORTABLE_BUILD=ON`. Its embedded `PackageManagerLib::platformVariantsToTry()` ([source](https://github.com/logos-co/logos-package-manager/blob/main/src/package_manager_lib.cpp#L832-L838)) returns the bare host variant (`darwin-arm64`). Modules whose `manifest.json` `main` key has a `-dev` suffix are silently dropped.
- **`lgpm`** (the host CLI from `logos-co/logos-package-manager#lgpm`) is compiled with `LGPM_PORTABLE_BUILD=OFF`. Same function instead appends `-dev` to every variant, so it accepts only `<host>-dev` archives.

There is no single `.lgx` artefact that satisfies both. The distributed stack picks `#lgx-portable` (matches the host) and then has nowhere to install it from — `lgpm` refuses, scaffold's `lgs basecamp install` is built on `lgpm`, so the whole install path collapses.

Until this is fixed, `lgs basecamp install` / `launch` is unusable on the distributed stack. This is why this project's adoption of `lgs basecamp` is currently declarative-only: `[modules.*]` entries seeded for `lgs basecamp doctor` drift detection, but install/launch still hand-rolled.

</details>

---

## TR-04

### `lgs basecamp launch` exposes macOS `XDG_RUNTIME_DIR` short-path override — P1

**TL;DR.** macOS caps Unix-domain-socket paths at 104 bytes. liblogos socket names overflow any deep `XDG_RUNTIME_DIR`. `lgs basecamp launch` puts XDG dirs under the project root by default — guaranteed to break on any `/Users/<user>/Developer/...` checkout.

**Why this hurts us.** Forced to set `XDG_RUNTIME_DIR=/tmp/lbc-<name>` and `TMPDIR=$XDG_RUNTIME_DIR` in [`scripts/basecamp-instance.sh` lines 60-65, 263-265, 278-290](../scripts/basecamp-instance.sh#L60-L290). Captured in [`delivery-dogfooding.md`: "short-path requirement for Basecamp runtime sockets is undocumented"](../delivery-dogfooding.md#L213-L239).

**Suggested fix.** Either:
- Have `lgs basecamp launch` auto-set `XDG_RUNTIME_DIR` (and `TMPDIR`) under a short path like `/tmp/lgs-<profile>/` on macOS, OR
- Accept a `[basecamp.runtime_dir]` override and document the 104-byte budget in `docs/basecamp-module-requirements.md`.

<details>
<summary>Details</summary>

liblogos creates token sockets named `<XDG_RUNTIME_DIR>/logos_token_<module>_<pid>`. macOS's `sockaddr_un.sun_path` field caps at 104 bytes. A typical project under `/Users/<user>/Developer/<project>/.scaffold/basecamp/profiles/alice/run` plus the socket name suffix easily blows the budget; Basecamp aborts module loading with `[SubprocessContainer] Unix socket path too long (122 >= 104)`.

Every macOS basecamp consumer must independently re-discover this. The error message is clear, but the requirement isn't documented anywhere in the Basecamp / liblogos integration journey.

</details>

---

## TR-05

### Per-profile env files in `[modules.*]` / `[basecamp.profiles.*]` — P1

**TL;DR.** This project's two-Basecamp dogfooding needs maker to load `.env` and taker to load `.env.taker`. Scaffold has no way to declare that mapping; the launcher script hard-codes it.

**Why this hurts us.** Env-file selection plus `SWAP_UI_AUTO_ENV_FILE` / `SWAP_UI_AUTO_ROLE` wiring lives in [`scripts/basecamp-instance.sh` lines 252-291](../scripts/basecamp-instance.sh#L252-L291).

**Suggested fix.** Add a `[basecamp.profiles.<name>]` table:

```toml
[basecamp.profiles.alice]
env_file = ".env"
env = { SWAP_UI_AUTO_ROLE = "maker" }

[basecamp.profiles.bob]
env_file = ".env.taker"
env = { SWAP_UI_AUTO_ROLE = "taker" }
```

`lgs basecamp launch <profile>` sources `env_file` (if set) and applies `env` over the existing XDG/`LOGOS_PROFILE` block.

<details>
<summary>Details</summary>

Composes with TR-17 (configurable profile names) so the names in the table can be `maker`/`taker` rather than `alice`/`bob`. The role-to-env mapping is project-specific configuration scaffold shouldn't bake in, but the *mechanism* to express it has to live somewhere.

</details>

---

## TR-06

### `lgs run` gains pre-localnet / co-process hook stages (for Anvil) — P1

**TL;DR.** `lgs run` covers the LEZ-side of `make infra` perfectly, but can't manage an in-process Anvil daemon that needs to be alive *during* the post-deploy hook (so the hook can read Anvil's ephemeral RPC URL and private keys).

**Why this hurts us.** [`src/cli/infra.rs` lines 48-200](../src/cli/infra.rs#L48-L200) is 152 lines of hand-rolled orchestration that lives outside scaffold purely because `lgs run` doesn't model long-lived sibling processes.

**Suggested fix.** Either:
- Document a "co-process" pattern in `lgs run` where a `pre_localnet` (or `pre_deploy`) hook starts a daemon and its stdout `KEY=VALUE` lines are exported into the rest of the pipeline, OR
- Add a `[run.pre_localnet]` hook stage with explicit env-export semantics.

<details>
<summary>Details</summary>

The project's `infra` binary does Anvil startup, EthHTLC contract deploy, LEZ HTLC deploy, two-file `.env` writer, and blocks on `Ctrl-C` to keep Anvil alive. The reason none of that fits a one-shot post-deploy hook today: Anvil's WebSocket URL and signer keys are only known after Anvil spawns, and the hook is one-shot.

A successful design would let this project shrink `src/cli/infra.rs` to a small Anvil-only daemon plus a TOML-declared post-deploy `.env` template that consumes `SCAFFOLD_PROGRAM_ID_lez_htlc_program`, `NSSA_WALLET_HOME_DIR`, `SEQUENCER_URL`, and the daemon-exported Anvil env vars.

</details>

---

## TR-07

### `[circuits]` schema + `lgs setup` auto-fetch — P1

**TL;DR.** Every LEZ project needs to fetch a matching `logos-blockchain-circuits` release bundle and point `LOGOS_BLOCKCHAIN_CIRCUITS` at it. This project does 68 lines of platform-detect + curl in the Makefile. Scaffold should own this.

**Why this hurts us.** [`Makefile` lines 11-68](../Makefile#L11-L68) — `circuits` target with manual platform mapping, version pinning, tarball download, version check, and export.

**Suggested fix.** Add `[circuits]` to `scaffold.toml`:

```toml
[circuits]
version = "v0.4.2"
# Optional: explicit URL override for air-gapped / mirrored installs.
# url_template = "https://example.com/circuits/{version}/{platform}.tar.gz"
```

`lgs setup` (or a dedicated `lgs circuits fetch`):

- Detects host platform.
- Downloads into `.scaffold/circuits/` if `VERSION` mismatches.
- Refuses for unsupported platforms (e.g. macOS Intel) with the same actionable message the Makefile prints today.
- `lgs setup`/`lgs build`/`lgs run` auto-export `LOGOS_BLOCKCHAIN_CIRCUITS`.

`lgs doctor` grows a `circuits` check that verifies the on-disk version matches the configured pin.

<details>
<summary>Details</summary>

The default behaviour of "everything lands in `~/.logos-blockchain-circuits/`" is actively hostile to working on two LEZ projects at different pins; the project's Makefile workaround isolates it under `.scaffold/circuits/` and exports the env var so cargo + scaffold + child processes all see the same dir.

Every LEZ project re-implements this. The fix is purely additive.

</details>

---

## TR-08

### Per-platform `[repos.basecamp].attr` map (`bin-macos-app` darwin, `bin-appimage` linux) — P1

**TL;DR.** `lgs basecamp setup`/`launch` hard-codes the basecamp flake's `app` attr (the dev-stack binary). Distributed-stack projects need `bin-macos-app` on darwin and `bin-appimage` on linux. This is a cross-platform need, not macOS-special.

**Why this hurts us.** Per-platform binary selection in [`scripts/basecamp-instance.sh` lines 71-95, 252-291](../scripts/basecamp-instance.sh#L71-L291) and the `BASECAMP_PACKAGE` bash switch.

**Suggested fix.** Allow `[repos.basecamp].attr` to be a per-platform map:

```toml
[repos.basecamp]
source = "github:logos-co/logos-basecamp"
pin = "<rev>"
attr.aarch64-darwin = "bin-macos-app"
attr.x86_64-darwin = "bin-macos-app"
attr.aarch64-linux = "bin-appimage"
attr.x86_64-linux = "bin-appimage"
```

`lgs basecamp setup`/`launch` resolves the right attr per host, removing the bash-side switch entirely.

<details>
<summary>Details</summary>

The distributed stack is cross-platform; both `bin-macos-app` and `bin-appimage` are built around the same `#portable` derivation (compiled with `LOGOS_PORTABLE_BUILD=ON`). The macOS-specific piece is only the `.app` bundle wrapping; the underlying portable/dev split is identical on both OSes.

Together with TR-03 (lgpm variant alignment), TR-08 is what would let `lgs basecamp install` / `launch` natively support the distributed stack.

</details>

---

## TR-09

### `lgs run --watch` debounce + glob filters — P2

**TL;DR.** Today `--watch` re-runs the full pipeline on any filesystem change with a 500ms debounce. A `[run.watch]` glob filter would let users scope re-deploys to just inputs that change a guest binary hash.

**Why this hurts us.** Indirect — would matter once this project adopts `lgs run`. Edits to orchestrator Rust (`src/`) shouldn't trigger guest-program re-deploys.

**Suggested fix.** Add `[run.watch].include` / `.exclude` glob lists and expose `--watch-debounce-ms` on the CLI.

<details>
<summary>Details</summary>

Default debounce coalesces a flurry of editor saves into one re-run, which is right. The missing piece is *what to watch*. Scoping to `programs/lez-htlc/**` would skip non-guest edits.

</details>

---

## TR-10

### `lgs basecamp build-portable` should optionally also build `#lgx` — P2

**TL;DR.** `build-portable` builds only `#lgx-portable`. The project's `make swap-lgx-build` builds both because the standalone-app smoke test (`make swap-ui-run`) loads `#lgx`. A `--variants` flag would let one command cover both.

**Why this hurts us.** [`Makefile` lines 122-131](../Makefile#L122-L131) — `swap-lgx-build` builds both variants for both modules.

**Suggested fix.** `lgs basecamp build --variants lgx,lgx-portable` (or compose with TR-14).

<details>
<summary>Details</summary>

Composes with TR-14 (granular per-module build). Together they let `swap-lgx-build` become `lgs basecamp build --variants all` and `make swap-module-build` become `lgs basecamp build --module swap`.

</details>

---

## TR-11

### Document `[modules.*]` is hand-editable without `lgs basecamp modules` — P2

**TL;DR.** The skill docs say "manual edits are preserved across re-runs" but don't explicitly state you can hand-author `[modules.*]` entries with no `lgs basecamp` invocation at all. This project did exactly that for drift detection; worth blessing as a supported pattern.

**Why this hurts us.** This project's `scaffold.toml` lines 31-46 — declarative seeding for drift detection without invoking `lgs basecamp setup`. Worked but felt off-spec.

**Suggested fix.** One-paragraph addition to `docs/basecamp-module-requirements.md` covering the declarative-only / drift-detection use case.

<details>
<summary>Details</summary>

`lgs basecamp doctor` happily picked up the hand-authored entries and even flagged drift on `delivery_module` — the use case clearly works, it just isn't documented.

</details>

---

## TR-12

### `lgs basecamp launch --log-file` (or default tee) — P2

**TL;DR.** Debugging two simultaneous instances is much easier when each launch tees its output to a file. This project does `2>&1 | tee` in the bash launcher; scaffold's `launch` `exec`s and gives up the parent process.

**Why this hurts us.** [`scripts/basecamp-instance.sh` line 290](../scripts/basecamp-instance.sh#L290).

**Suggested fix.** Optional `--log-file` flag, or default to `.scaffold/basecamp/profiles/<profile>/basecamp.log`.

<details>
<summary>Details</summary>

Bundle with TR-04 / TR-05 / TR-08 / TR-16 — all are touching `lgs basecamp launch` profile semantics.

</details>

---

## TR-13

### Document `--user-dir` vs scaffold's XDG isolation — P2

**TL;DR.** Pure docs ask. `bin-macos-app` grew a first-class `--user-dir` flag that removed the old `LOGOS_DATA_DIR + Dev` suffix dance. Scaffold uses XDG-based isolation instead. Worth a short note explaining the relationship.

**Why this hurts us.** None — purely a doc clarity item. Captured in [`delivery-dogfooding.md`: "--user-dir flag cleanly isolates Basecamp instances"](../delivery-dogfooding.md#L418-L433).

**Suggested fix.** One-line note in `docs/basecamp-module-requirements.md` cross-referencing `--user-dir` and the rationale for scaffold's XDG-based isolation.

<details>
<summary>Details</summary>

Reduces churn for downstream multi-instance test harnesses that read both pieces of documentation and have to reconcile the two mechanisms.

</details>

---

## TR-14

### `lgs basecamp build` (default `#lgx` variant, granular per module) — P1

**TL;DR.** Today the only way to build a single module's `#lgx` (without installing it) is raw `nix build` — exactly the workflow we're trying to retire ("use scaffold, not shell/nix").

**Why this hurts us.** [`Makefile` lines 113-119](../Makefile#L113-L119) (`swap-module-build`, `swap-ui-build`) and [`Makefile` lines 122-131](../Makefile#L122-L131) (`swap-lgx-build`) all reach for raw `nix build` because no `lgs basecamp build` exists.

**Suggested fix.** `lgs basecamp build [--variant lgx|lgx-portable|all] [--module NAME]…` with defaults of `--variant all`, all modules in `[modules.*]`. Idempotent — just shells nix, no install side-effects.

<details>
<summary>Details</summary>

Today's build matrix:

- `lgs basecamp install` → builds + installs (all modules, all roles). Blocked by TR-03 on distributed stack.
- `lgs basecamp build-portable` → builds only `#lgx-portable`, only `role = "project"`, all of them. No per-module filter.
- No `lgs basecamp build` (default variant). No per-module filter.

The combined effect with TR-10: `swap-module-build` becomes `lgs basecamp build --module swap`, `swap-lgx-build` becomes `lgs basecamp build --variants all`.

</details>

---

## TR-15

### `lgs basecamp run <module> [--host standalone|basecamp]` — P1

**TL;DR.** UI module dev iteration loop = `nix run` the module inside `logos-standalone-app` (the dependency-bundling test runner from `logos-module-builder`). Scaffold has no equivalent — consumers drop back to raw `nix run` from the module's subdirectory.

**Why this hurts us.** [`Makefile` lines 134-136](../Makefile#L134-L136) (`swap-ui-run`).

**Suggested fix.** Add `lgs basecamp run <module> [--host standalone|basecamp]`:

- Default `--host standalone` for `type: "ui_qml"` modules.
- Resolves the module's flake from `[modules.<name>]`, runs `apps.<system>.default` (or a documented `apps.<system>.standalone` attr).
- For `--host basecamp`, equivalent to `launch <profile>` with the module preinstalled.
- Optional `[modules.<name>].standalone_app` schema field if a flake exposes the runner under a non-default app attr.

<details>
<summary>Details</summary>

Closes the "everything goes through `lgs`" loop for the UI dev cycle. Today every UI module project re-derives the same `cd <module> && nix run .#default` invocation.

</details>

---

## TR-16

### `lgs basecamp paths <profile>` (debug introspection) — P2

**TL;DR.** Print the resolved per-profile path manifest (XDG, runtime, wallet, log, env-file, basecamp binary). `lgs basecamp doctor` shows summary state; `paths` would give the full disk layout for a profile.

**Why this hurts us.** [`scripts/basecamp-instance.sh` lines 180-197](../scripts/basecamp-instance.sh#L180-L197) (`cmd_paths`).

**Suggested fix.** `lgs basecamp paths <profile> [--json]` printing something like:

```
xdg_config:  .scaffold/basecamp/profiles/alice/config
xdg_data:    .scaffold/basecamp/profiles/alice/data
xdg_cache:   .scaffold/basecamp/profiles/alice/cache
xdg_runtime: /tmp/lgs-alice                                 (short on macOS — see TR-04)
log:         .scaffold/basecamp/profiles/alice/basecamp.log (if TR-12 lands)
wallet:      .scaffold/wallet                               (project-shared, see [wallet])
basecamp:    <store-path>/bin/LogosBasecamp
```

<details>
<summary>Details</summary>

Bundle into the same PR as TR-04 / TR-05 / TR-08 / TR-12 — all touching `lgs basecamp launch` profile semantics.

</details>

---

## TR-17

### Configurable basecamp profile names (`maker`/`taker` instead of `alice`/`bob`) — P1

**TL;DR.** Scaffold hardcodes profiles to `alice`/`bob`. Domain-specific projects (atomic swaps = maker/taker, lending = borrower/lender, etc.) carry semantic role names everywhere else in their code — being forced to translate via "alice = maker, bob = taker" is needless cognitive overhead.

**Why this hurts us.** Every basecamp target carries `maker`/`taker` naming: [`scripts/basecamp-instance.sh` lines 45-69](../scripts/basecamp-instance.sh#L45-L69), [`Makefile` lines 4-6](../Makefile#L4-L6). The rest of the project's code, docs, env files (`.env` for maker, `.env.taker` for taker), and the swap-ui's `SWAP_UI_AUTO_ROLE` env var all use maker/taker.

**Suggested fix.** Allow profile names to be declared:

```toml
[basecamp.profiles.maker]
[basecamp.profiles.taker]
```

With defaults of `alice`/`bob` when omitted, preserving the current two-instance dogfooding contract for projects that haven't customised.

<details>
<summary>Details</summary>

Composes naturally with TR-05 (per-profile env files) — both touch the same `[basecamp.profiles.*]` table.

</details>

---

## TR-19

### `lgs run` `stop_on_exit` + `pre_localnet` hook stages — P1

**TL;DR.** Bucket 3 of the Makefile (`make test`, `make demo`) needs `lgs run --profile X` to (a) accept a pre-localnet hook for `forge build`, and (b) stop the localnet on completion so test runs don't leave a sequencer process behind.

**Why this hurts us.** [`Makefile` lines 79-88](../Makefile#L79-L88) (`test`, `demo`, `infra`). All three chain `circuits + contracts + lgs localnet` + a `cargo` invocation, and `test` manually stops the localnet after `cargo test` exits.

**Suggested fix.** Add to `[run.profiles.<name>]`:

```toml
[run.profiles.test]
pre_localnet = ["forge build --root contracts"]    # before localnet starts
post_deploy = ["cargo test"]                       # current spec
stop_on_exit = true                                # NEW — stop localnet when last hook returns
```

`pre_localnet` hooks run before step 3 of the pipeline (localnet start). `stop_on_exit` invokes `lgs localnet stop` after the last `post_deploy` hook returns (success or failure).

<details>
<summary>Details</summary>

Together with TR-06 (co-process hooks for Anvil) and TR-07 (`[circuits]` auto-fetch), this finishes the Bucket 3 story: `make test` becomes `lgs run --profile test` with zero shell glue, and `make demo` becomes `lgs run --profile demo` once the demo binary is gutted the same way `src/cli/infra.rs` gets gutted by TR-06.

Pre-localnet vs pre-deploy distinction matters: `forge build` needs to happen before sequencer startup so contract artefacts are on disk when `cargo test` reaches for them via `sol!()` macros.

</details>

---

## TR-20

### `lgs basecamp develop <module>` for verb-set symmetry — P2

**TL;DR.** Scaffold has (or will have) `lgs basecamp build`, `run`, `install`, `launch` — missing `develop`. Should be a thin wrapper around `nix develop` of the module's flake. Composes with LMB-01 upstream (logos-module-builder providing a default dev shell for universal modules wrapping Rust cdylibs).

**Why this hurts us.** Two-step IDE workflow today: `cd swap-module && nix develop` instead of `lgs basecamp develop swap`. Marginal individually; the verb-set symmetry argument is the real motivation — once `develop` exists, every common dev verb maps to `lgs basecamp <verb>` and consumers stop reaching for raw `nix`.

**Suggested fix.** `lgs basecamp develop <module>`:

- Resolves the module's flake from `[modules.<name>]`.
- Execs `nix develop <flake>#default` (or a documented `dev-shell` attr).
- Sets `LOGOS_PROFILE` / scaffold-managed env vars so any in-shell `lgs` invocations resolve correctly.

<details>
<summary>Details</summary>

Reference dev shell shape: this project's `swap-module/flake.nix` after [PR #26](https://github.com/logos-co/eth-lez-atomic-swaps/pull/26) — pre-builds the Rust cdylib, symlinks it into the module's `lib/`, sets `DYLD_LIBRARY_PATH` / `LD_LIBRARY_PATH` / `CMAKE_LIBRARY_PATH` / `CMAKE_INCLUDE_PATH` / `CMAKE_EXPORT_COMPILE_COMMANDS=ON`.

LMB-01 (upstream `logos-module-builder` ask, tracked separately) would make this shape the default for every universal C++ module wrapping a Rust cdylib. Once LMB-01 lands, `lgs basecamp develop <module>` becomes a pure UX wrapper with zero scaffold-side logic beyond flake resolution.

Compose order:

1. Land LMB-01 in `logos-module-builder` (default dev shell shape).
2. Land TR-20 in `scaffold` (the `develop` verb).
3. Consumer modules delete their custom shellHook overrides.

</details>

---

## Retired entries

### TR-18 — `lgs build --vendor-ffi` for local non-Nix cdylib staging

**Retired.** Originally proposed scaffold-side support for staging Rust cdylibs into a universal C++ module's `lib/` dir for non-Nix dev iteration (the pain point: `make swap-vendor-ffi`). After re-examination, the right layer is **Nix dev shells** (`devShells.<system>.swap-module` in `swap-module/flake.nix` that pre-builds `swap-ffi` and exposes it in `LIBRARY_PATH` + `compile_commands.json` for clangd), not a scaffold feature. Scaffold has no business reaching into module-internal cargo↔CMake glue. Tracked as a separate project-internal refactor; not an upstream ask.

---

## Tracker change-log

- **2026-05-20** — Initial draft (TR-01 … TR-13), created during the `eth-lez-atomic-swaps` scaffold 0.1.1 → 0.2.0 upgrade pass against scaffold master `d50caf4`.
- **2026-05-20** — Added TR-14 (`lgs basecamp build` granular), TR-15 (`lgs basecamp run` for standalone-app), TR-16 (`lgs basecamp paths`), TR-17 (configurable profile names), TR-18 (vendor-ffi — later retired) after re-evaluating Bucket 1 of the Makefile-deletion plan under the principle "if scaffold can do it, use scaffold; if scaffold can't, file upstream, don't write more shell."
- **2026-05-20** — Restructured for readability: added mental model + glossary + per-entry TL;DR/Why/Fix/Details template + TOC. Reframed TR-08 as cross-platform (not macOS-special). Retired TR-18 — Nix dev shell is the right layer for the swap-vendor-ffi problem, not scaffold. Added TR-19 (`stop_on_exit` + `pre_localnet` hook stages) to unblock Bucket 3 deletion (`make test`, `make demo`).
- **2026-05-20** — Added TR-20 (`lgs basecamp develop <module>` for verb-set symmetry), composes with the upstream LMB-01 ask against `logos-module-builder`.
- **2026-05-22** — TR-01 filed as [logos-co/scaffold#170](https://github.com/logos-co/scaffold/issues/170); its acceptance criteria also subsume TR-02. Companion PR [logos-co/scaffold#169](https://github.com/logos-co/scaffold/pull/169) (narrow SPel public-pin fix, commit-only pin per fryorcraken's review) is near landing. TR-03 is now the only unfiled P0.
