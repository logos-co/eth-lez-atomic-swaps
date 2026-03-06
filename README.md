# Atomic Swaps PoC

Cross-chain atomic swap between LEZ and Ethereum using hash time-locked contracts (HTLCs). Maker sells LEZ for ETH — both sides are trustless with timeout refunds.

```
Maker                                          Taker
  |  1. Generate preimage, lock LEZ             |
  |─────────── share hashlock ────────────────>|
  |                                 2. Verify LEZ escrow, lock ETH
  |  3. Claim ETH (reveals preimage)            |
  |                                 4. Claim LEZ (using preimage)
```

## Screenshots

**Standalone UI** (maker + taker side-by-side):

![Standalone UI](docs/standalone-ui.gif)

**logos-app plugin:**

| Config | Maker | Taker | Refund |
|--------|-------|-------|--------|
| ![Config](docs/config.png) | ![Maker](docs/maker.png) | ![Taker](docs/taker.png) | ![Refund](docs/refund.png) |

![logos-app plugin](docs/logos-app-plugin.gif)

## Quick Start

**Prerequisites:** Rust 1.85+, Foundry, CMake 3.21+, Qt 6.5+, Docker.

<details><summary><b>macOS</b></summary>

```bash
brew install qt@6 cmake
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
curl -L https://foundry.paradigm.xyz | bash && foundryup
```
Also install [Docker Desktop](https://docs.docker.com/desktop/install/mac-install/). If CMake can't find Qt6: `export CMAKE_PREFIX_PATH="$(brew --prefix qt@6)"`
</details>

<details><summary><b>Linux (Ubuntu/Debian)</b></summary>

```bash
# Ubuntu / Debian
sudo apt install cmake qt6-base-dev qt6-declarative-dev docker.io docker-compose-plugin
# Fedora
sudo dnf install cmake qt6-qtbase-devel qt6-qtdeclarative-devel docker-compose 
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
curl -L https://foundry.paradigm.xyz | bash && foundryup
```
Qt 6.5+ required — Ubuntu 24.10+ ships it. For older distros use [aqtinstall](https://github.com/miurahr/aqtinstall) or the [Qt online installer](https://www.qt.io/download-qt-installer).
</details>

```bash
git clone --recurse-submodules https://github.com/logos-co/eth-lez-atomic-swaps.git
cd eth-lez-atomic-swaps
```

> Already cloned without `--recurse-submodules`? Run `git submodule update --init --recursive`.

Make sure Docker is running, then start local services (Anvil, LEZ sequencer, nwaku), deploy contracts, and write `.env` files:

```bash
make infra                # keeps running — Ctrl-C to stop
```

Then open a new terminal and pick an interface:

**Standalone UI**
```bash
make configure            # first time only — builds FFI bridge + cmake configure
make run-maker            # open maker UI
make run-taker            # open taker UI (in new terminal)
```

**logos-app plugin**

<details><summary><b>Setting up logos-app</b></summary>

```bash
git clone https://github.com/logos-co/logos-app.git
cd logos-app
nix build            # builds the app via flake.nix — produces result/bin/logos-app
```

The Makefile expects logos-app at `~/Developer/status/logos-app`. If yours is elsewhere, override the path:

```bash
make plugin-build LOGOS_APP_INTERFACES=<path-to-logos-app>/app/interfaces
make plugin-run LOGOS_APP_BIN=<path-to-logos-app>/result/bin/logos-app
```
</details>

```bash
make plugin-build         # builds FFI bridge + IComponent plugin
make plugin-run           # launch logos-app with the swap plugin loaded
```

**Maker**: Publish Offer → Start Swap → wait for taker.
**Taker**: Discover Offers → select offer → Start Taker → swap completes.

Stop with `Ctrl-C` on `make infra`, then `make nwaku-stop` to clean up Docker.

## Architecture

```
┌──────────────────┬──────────────────┐
│  Qt6 UI (ui/)    │ Logos Core module │
│  standalone app  │ (logos-module/)   │
├──────────────────┴──────────────────┤
│       swap-ffi (C bridge / cdylib)  │
├─────────────────────────────────────┤
│      Swap orchestration library     │
├─────────────────────────────────────┤
│     Chain monitoring + Messaging    │
├─────────────────┬───────────────────┤
│   alloy (ETH)   │   nssa_core (LEZ) │
└─────────────────┴───────────────────┘
```

| Directory | Description |
|---|---|
| `contracts/` | Solidity HTLC (Foundry) — `lock()`, `claim()`, `refund()` with SHA-256 hashlock |
| `programs/lez-htlc/` | LEZ HTLC program (Risc0 zkVM) — same logic, escrow in PDA |
| `src/` | Orchestration library — ETH/LEZ clients, watchers, messaging, maker/taker/refund flows |
| `swap-ffi/` | C FFI bridge exposing swap functions to the Qt6 UI |
| `ui/` | Qt6/QML standalone app — Config, Maker, Taker, Refund tabs |
| `logos-module/` | Logos Core / logos-app plugin — same UI as embeddable plugin (two modes) or standalone app |
| `tests/` | Integration tests |
| `scripts/` | Local setup scripts (Anvil, contract deploy, `.env` generation) |

## Commands

| Command | Description |
|---|---|
| `make infra` | Start Anvil (local Ethereum), LEZ sequencer, nwaku; deploy HTLC contracts on both chains; write `.env` files |
| `make configure` | Build the Rust FFI bridge + run cmake configure for the Qt6 app (first time only) |
| `make build` | Build the Qt6 standalone UI |
| `make run-maker` / `run-taker` | Launch the maker/taker UI window, loading config from `.env` / `.env.taker` |
| `make demo` | Run the full swap headlessly in one terminal — no UI needed, good sanity check |
| `make contracts` | Build Solidity contracts via Foundry |
| `make nwaku` / `nwaku-stop` | Start/stop nwaku Docker containers |
| `make plugin-build` | Build the Rust FFI bridge + IComponent plugin for logos-app |
| `make plugin-run` | Install plugin and launch logos-app with the swap module loaded |
| `make logos-module-build` / `logos-module-run` | Build / run Logos Core module as standalone app |
| `make clean` | Clean Qt6 UI build artifacts |
| `cargo test` | Run all Rust tests (orchestration library + integration) |

**CLI** (config via `.env` or CLI flags — see `.env.example`):

```bash
swap-cli maker [--preimage <hex>]       # generate preimage, lock LEZ, claim ETH
swap-cli taker --hashlock <hex>         # verify escrow, lock ETH, claim LEZ
swap-cli refund lez --hashlock <hex>    # refund LEZ after timelock
swap-cli refund eth --swap-id <hex>     # refund ETH after timelock
swap-cli status --hashlock <hex>        # inspect escrow state
swap-cli infra                          # start Anvil + LEZ sequencer + nwaku, deploy, write .env
```

## Design Notes

- **SHA-256 hashlock** (not keccak) — cross-chain compatibility with LEZ's `risc0_zkvm::sha`
- **LEZ timelock is off-chain** — LSSA programs lack timestamp access; orchestration checks wall-clock time
- **LEZ escrow is two-step** — Lock (claim PDA + metadata) then Transfer (fund PDA), due to LSSA balance rules
- **Messaging is optional** — works without nwaku via manual hashlock exchange