# Atomic Swaps PoC

Cross-chain atomic swap between LEZ and Ethereum using hash time-locked contracts (HTLCs). Maker sells LEZ for ETH — both sides are trustless with timeout refunds.

```
Taker                                          Maker
  |  1. Generate preimage + hashlock            |
  |  2. Lock ETH (long timelock)                |
  |─────────── ETH Locked event ─────────────>|
  |                                 3. Verify ETH lock, lock LEZ (short timelock)
  |  4. Claim LEZ (reveals preimage)            |
  |                                 5. Learn preimage, claim ETH
```

## Screenshots

**logos-app plugin:**

| Config | Maker | Taker | Refund |
|--------|-------|-------|--------|
| ![Config](docs/config.png) | ![Maker](docs/maker.png) | ![Taker](docs/taker.png) | ![Refund](docs/refund.png) |

![logos-app plugin](docs/logos-app-plugin.gif)

## Getting started

These steps are what you need to clone the repo and run the stack locally.

### Requirements

- **OS / CPU:** Apple Silicon macOS or 64-bit Linux (`aarch64` or `x86_64`). **Intel macOS is not supported** for circuits: upstream does not ship a `macos-x86_64` `logos-blockchain-circuits` bundle (see `Makefile` / `make setup`).
- **Rust** 1.85+ ([rustup](https://rustup.rs/))
- **Foundry** (`forge`, `anvil`) — [Foundry book](https://book.getfoundry.sh/getting-started/installation)
- **CMake** 3.21+ and **Qt** 6.5+ (for the optional `logos-module` / `swap-ffi` UI build)
- **GNU make** and a **C/C++ toolchain** (first build compiles Nim `libwaku` in-process; allow roughly 5–10 minutes, then cached)
- **[`logos-scaffold`](https://github.com/logos-co/logos-scaffold)** on your `PATH` (install from a clone of that repo: `cargo install --path .` — puts `logos-scaffold` and `lgs` in `~/.cargo/bin`)

**Docker / Podman:** Not required for `libwaku` messaging or for a typical `make setup` / `make infra` flow. [`logos-scaffold doctor`](https://github.com/logos-co/logos-scaffold) may still warn that a container runtime is missing; install Docker or Podman if tooling or LEZ workflows you use expect it.

### Clone

```bash
git clone --recurse-submodules https://github.com/logos-co/eth-lez-atomic-swaps.git
cd eth-lez-atomic-swaps
```

Already cloned without submodules? Run:

```bash
git submodule update --init --recursive
```

### System packages

<details><summary><b>macOS</b></summary>

```bash
brew install qt@6 cmake
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
curl -L https://foundry.paradigm.xyz | bash && foundryup
```

If CMake cannot find Qt6: `export CMAKE_PREFIX_PATH="$(brew --prefix qt@6)"`. Homebrew’s `qt` formula (Qt 6.x) also works; point `CMAKE_PREFIX_PATH` at `$(brew --prefix qt)` if you use that instead.

The workspace [`.cargo/config.toml`](.cargo/config.toml) supplies macOS `aarch64` linker flags `libwaku` needs.

</details>

<details><summary><b>Linux (Ubuntu / Debian / Fedora)</b></summary>

```bash
# Ubuntu / Debian
sudo apt install cmake qt6-base-dev qt6-declarative-dev build-essential
# Fedora
sudo dnf install cmake qt6-qtbase-devel qt6-qtdeclarative-devel gcc gcc-c++ make
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
curl -L https://foundry.paradigm.xyz | bash && foundryup
```

Qt 6.5+ required — Ubuntu 24.10+ ships it. For older distros use [aqtinstall](https://github.com/miurahr/aqtinstall) or the [Qt online installer](https://www.qt.io/download-qt-installer).

</details>

### One-time setup and local infra

This repo ships a checked-in [`scaffold.toml`](scaffold.toml) with **relative** paths: LEZ is cloned under `.scaffold/lez-cache/` (gitignored), next to `.scaffold/wallet` and `.scaffold/circuits`. You do **not** need `logos-scaffold init` on a fresh clone. Run `make` targets from the repository root so those paths resolve correctly.

```bash
make setup    # downloads logos-blockchain-circuits into .scaffold/circuits, runs logos-scaffold setup (LEZ + wallet)
make infra    # Anvil, LEZ localnet, embedded waku rendezvous, deploy contracts, write .env / .env.taker — Ctrl-C stops everything
```

`make setup` exports `LOGOS_BLOCKCHAIN_CIRCUITS` to the project-local circuits directory so builds do not use `~/.logos-blockchain-circuits/`.

`make infra` uses `logos-scaffold` for LEZ (`localnet start`, `wallet topup`, `localnet stop` on exit). Anvil, Solidity deploy, and the in-process waku rendezvous node are driven by this repo’s orchestrator; rendezvous multiaddr is written into `.env` / `.env.taker` as `WAKU_BOOTSTRAP_MULTIADDR`.

> The Logos messaging node (`libwaku`) is embedded in-process — there is no separate Docker service for waku. See [delivery-dogfooding.md](delivery-dogfooding.md) for integration notes.

### Run a swap (after `make infra`)

**Maker:** Publish Offer → Start Swap → wait for taker ETH lock → lock LEZ → wait for preimage → claim ETH.  
**Taker:** Discover Offers → select offer → Start Taker → generate preimage, lock ETH → wait for LEZ lock → claim LEZ.

After **`make setup`**, list LEZ accounts from the repo root (`logos-scaffold` reads [`scaffold.toml`](scaffold.toml)):

```bash
logos-scaffold wallet list --long
```

### Headless demo and tests

```bash
make demo     # full swap without UI
make test     # forge build + localnet + cargo test + localnet stop
```

### CLI (same binary as `cargo run`)

From the repo root, prefer:

```bash
cargo run --bin swap-cli -- maker
cargo run --bin swap-cli -- taker
cargo run --bin swap-cli -- infra
cargo run --bin swap-cli -- demo
cargo run --bin swap-cli -- refund lez --hashlock <hex>
cargo run --bin swap-cli -- refund eth --swap-id <hex>
```

Or with two shells and `.env` files after `make infra`:

```bash
env $(grep -v '^\#' .env | xargs) cargo run --bin swap-cli -- maker
env $(grep -v '^\#' .env.taker | xargs) cargo run --bin swap-cli -- taker
```

After `cargo build --release`, you can run `./target/release/swap-cli` instead.

Configuration: [`.env.example`](.env.example).

## logos-app plugin (UI)

The optional UI runs inside [logos-app](https://github.com/logos-co/logos-app) as an IComponent plugin — see **Screenshots** above. Building it needs Nix (for logos-app) and Qt that matches logos-app (see below).

<details><summary><b>First-time logos-app setup</b></summary>

```bash
git clone https://github.com/logos-co/logos-app.git
cd logos-app
nix build            # produces result/bin/logos-app
```

The [`Makefile`](Makefile) defaults `LOGOS_APP_INTERFACES` and `LOGOS_APP_BIN` to `~/Developer/status/logos-app`. Override if yours differs:

```bash
make plugin-build LOGOS_APP_INTERFACES=<path-to-logos-app>/app/interfaces
make plugin-run-maker LOGOS_APP_BIN=<path-to-logos-app>/result/bin/logos-app
```

Plugin CMake uses Nix Qt paths hardcoded in the Makefile (`NIX_QTBASE`, …). If your Nix store hashes differ, run `nix build` in logos-app, then refresh those variables (e.g. `nix path-info .#logos-app --recursive | grep qt`).

</details>

```bash
make plugin-run-maker     # build + install plugin, launch logos-app as maker (loads .env)
make plugin-run-taker     # second terminal — taker (loads .env.taker)
```

Two logos-app instances (maker and taker). On macOS the plugin installs under `~/Library/Application Support/Logos/LogosAppNix/plugins/lez_atomic_swap/`.

**Mechanics:** `make plugin-install` copies `lez_atomic_swap.dylib` and `libswap_ffi.dylib` into the plugin directory; logos-app loads the plugin, which registers a `SwapBackend` QML object. Env vars come from `.env` / `.env.taker`.

## Architecture

```
┌─────────────────────────────────────┐
│  logos-app plugin (logos-module/)    │
├─────────────────────────────────────┤
│       swap-ffi (C bridge / cdylib)  │
├─────────────────────────────────────┤
│      Swap orchestration library     │
├─────────────────────────────────────┤
│     Chain monitoring + Messaging    │
├─────────────────┬───────────────────┤
│   alloy (ETH)   │   nssa_core (LEZ) │
└─────────────────┴───────────────────┘
```

| Path | Description |
|---|---|
| [`scaffold.toml`](scaffold.toml) | Logos scaffold config (LEZ pin, wallet dir, localnet); committed with relative paths |
| `contracts/` | Solidity HTLC (Foundry) — `lock()`, `claim()`, `refund()` with SHA-256 hashlock |
| `programs/lez-htlc/` | LEZ HTLC program (Risc0 zkVM) |
| `src/` | Orchestration — ETH/LEZ clients, watchers, messaging, maker/taker/refund |
| `swap-ffi/` | C FFI for Qt6 UI |
| `logos-module/` | logos-app IComponent plugin (Qt6/QML) |
| `tests/` | Integration tests |

## Make targets

| Command | Description |
|---|---|
| `make setup` | Circuits tarball + `logos-scaffold setup` (LEZ + wallet under `.scaffold/`) |
| `make infra` | Anvil, LEZ localnet, waku rendezvous, deploy, write `.env` files |
| `make demo` | Headless full swap |
| `make test` | Contracts + localnet + `cargo test` + stop localnet |
| `make contracts` | `forge build` in `contracts/` |
| `make localnet-start` / `localnet-stop` | LEZ localnet via `logos-scaffold` |
| `make plugin-build` | Configure + build Qt plugin (Nix Qt paths in Makefile) |
| `make plugin-run-maker` / `plugin-run-taker` | Install plugin and launch logos-app |

## Design notes

- **SHA-256 hashlock** (not keccak) — cross-chain alignment with LEZ’s `risc0_zkvm::sha`
- **Taker locks first** — taker holds the preimage; longer ETH timelock, shorter LEZ timelock after ETH lock is seen
- **LEZ timelock** — enforced with `TimestampValidityWindow` (LSSA); orchestrator wall-clock checks are UX-only
- **LEZ escrow** — two-step lock/transfer for LSSA balance rules
- **Scaffold wallet** — LEZ keys under `.scaffold/wallet`; orchestration reads signing material from disk
- **Messaging** — in-process `libwaku` via [waku-bindings](https://github.com/logos-messaging/logos-delivery-rust-bindings); swap logic still works without gossip if hashlock is exchanged manually. See [delivery-dogfooding.md](delivery-dogfooding.md)

## Troubleshooting

- **Pull blocked by untracked `scaffold.toml`:** Older clones gitignored that file. Run `mv scaffold.toml scaffold.toml.bak`, pull, then compare with the committed [`scaffold.toml`](scaffold.toml) if you had custom `cache_root` or LEZ pin values.
- **`logos-scaffold: command not found`:** Install the CLI and ensure `~/.cargo/bin` is on your `PATH` (or open a new shell after `rustup` / Foundry installers).

## Maintainer notes

- Bump **`CIRCUITS_VERSION`** in the [`Makefile`](Makefile) when the **lssa** git revisions in [`Cargo.toml`](Cargo.toml) require a newer published `logos-blockchain-circuits` bundle.
- Bump **`[repos.lez].pin`** (and matching `path` suffix under `cache_root`) in [`scaffold.toml`](scaffold.toml) when intentionally moving to a different [LEZ](https://github.com/logos-blockchain/logos-execution-zone/) revision; keep it consistent with what `logos-scaffold` and this repo expect.
