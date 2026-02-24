# Atomic Swaps PoC

Cross-chain atomic swap between LEZ and Ethereum using hash time-locked contracts (HTLCs). Maker sells LEZ for ETH — both sides are trustless with timeout refunds.

```
Maker                                          Taker
  |  1. Generate preimage, lock LEZ             |
  |─────────── share hashlock ────────────────>|
  |                                 3. Verify LEZ escrow, lock ETH
  |  4. Claim ETH (reveals preimage)            |
  |                                 5. Claim LEZ (using preimage)
```

## Quick Start

**Prerequisites:** Rust 1.85+, Foundry, CMake 3.21+, Qt 6.5+, Docker.

<details><summary>macOS</summary>

```bash
brew install qt@6 cmake
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
curl -L https://foundry.paradigm.xyz | bash && foundryup
```
Also install [Docker Desktop](https://docs.docker.com/desktop/install/mac-install/). If CMake can't find Qt6: `export CMAKE_PREFIX_PATH="$(brew --prefix qt@6)"`
</details>

<details><summary>Linux (Ubuntu/Debian)</summary>

```bash
sudo apt install cmake qt6-base-dev qt6-declarative-dev docker.io docker-compose-plugin
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
curl -L https://foundry.paradigm.xyz | bash && foundryup
```
Qt 6.5+ required — Ubuntu 24.10+ ships it. For older distros use [aqtinstall](https://github.com/miurahr/aqtinstall) or the [Qt online installer](https://www.qt.io/download-qt-installer).
</details>

```bash
git clone --recurse-submodules https://github.com/logos-co/eth-lez-atomic-swaps.git
cd eth-lez-atomic-swaps

make configure            # build Rust FFI bridge + cmake configure (first time only)
make infra                # start nwaku + Anvil + LEZ sequencer, deploy contracts, write .env files
# in new terminals:
make run-maker            # open maker UI
make run-taker            # open taker UI
```

`make infra` starts local Anvil and LEZ sequencer instances and writes `.env` / `.env.taker` automatically — no manual config needed. Make sure Docker is running before you start.

**Maker**: Publish Offer → Start Swap → wait for taker.
**Taker**: Discover Offers → select offer → Start Taker → swap completes.

Stop with Ctrl-C on `make infra`, then `make nwaku-stop` to clean up Docker.

> Already cloned without `--recurse-submodules`? Run `git submodule update --init --recursive`.

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
| `src/` | Orchestration library — ETH/LEZ clients, watchers, maker/taker/refund flows |
| `src/messaging/` | Waku messaging client for offer discovery (nwaku REST API) |
| `swap-ffi/` | C FFI bridge exposing swap functions to the Qt6 UI |
| `ui/` | Qt6/QML standalone app — Config, Maker, Taker, Refund tabs |
| `logos-module/` | Logos Core module — same UI as embeddable plugin or standalone app |

## Commands

| Command | Description |
|---|---|
| `make configure` / `make build` / `make clean` | Qt6 UI build lifecycle (auto-builds `swap-ffi`) |
| `make infra` | Start all services, deploy contracts, write `.env` files |
| `make run-maker` / `make run-taker` | Launch UI with maker/taker config |
| `make demo` | Automated CLI demo (no UI needed) |
| `make contracts` | Build Solidity contracts |
| `make nwaku` / `make nwaku-stop` | Start/stop nwaku Docker containers |
| `make logos-module-build` / `logos-module-run` | Build / run Logos Core module (standalone) |
| `cargo test` | Run all tests |

**CLI** (config via `.env` or CLI flags — see `.env.example`):

```bash
swap-cli maker [--preimage <hex>]       # generate preimage, lock LEZ, claim ETH
swap-cli taker --hashlock <hex>         # verify escrow, lock ETH, claim LEZ
swap-cli refund lez --hashlock <hex>    # refund LEZ after timelock
swap-cli refund eth --swap-id <hex>     # refund ETH after timelock
swap-cli status --hashlock <hex>        # inspect escrow state
```

## Design Notes

- **SHA-256 hashlock** (not keccak) — cross-chain compatibility with LEZ's `risc0_zkvm::sha`
- **LEZ timelock is off-chain** — LSSA programs lack timestamp access; orchestration checks wall-clock time
- **LEZ escrow is two-step** — Lock (claim PDA + metadata) then Transfer (fund PDA), due to LSSA balance rules
- **Messaging is optional** — works without nwaku via manual hashlock exchange
