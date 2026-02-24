# Atomic Swaps PoC

Cross-chain atomic swap between LEZ (lambda) and Ethereum using hash time-locked contracts (HTLCs). Maker sells LEZ for ETH — both sides are trustless with timeout refunds if either party disappears.

## How It Works

```
Maker                                          Taker
  |                                              |
  |  1. Generate secret preimage                 |
  |  2. Lock LEZ (hashlock = SHA256(preimage))   |
  |─────────── share hashlock ──────────────────>|
  |                                              |  3. Verify LEZ escrow
  |                                              |  4. Lock ETH (same hashlock)
  |  5. Claim ETH (reveals preimage on-chain)    |
  |                                              |  6. Claim LEZ (using revealed preimage)
  |                                              |
  v              Both sides complete              v
```

**Safety guarantees:**
- If the taker never locks ETH, the maker's LEZ timelock expires and they refund.
- If the maker never claims ETH, the taker's ETH timelock expires and they refund.
- The taker verifies the LEZ escrow (state, amount) before locking any ETH.

## Quick Start (Local Demo)

Run a full end-to-end swap locally using the Qt6 UI, with all infrastructure started in one command.

### Prerequisites

- **Rust** (edition 2024)
- **Foundry** (`forge`, `anvil`) — for contracts and local Ethereum
- **Docker** — for nwaku (Waku messaging node)
- **Qt6** (6.5+) with QtQuick — for the UI
- **CMake** — for building the UI
- **Access to `logos-blockchain/lssa`** on GitHub — LEZ SDK dependencies

### Steps

```bash
# 1. Start nwaku (messaging node)
make nwaku

# 2. Start all infrastructure (Anvil + LEZ sequencer + deploy contracts)
#    Writes .env (maker) and .env.taker (taker) automatically
make infra

# 3. Build and configure the UI (first time only)
make configure

# 4. Open maker and taker windows side by side
make run-maker   # launches UI with maker config
make run-taker   # launches UI with taker config

# 5. In the maker window:
#    - Click "Publish Offer" (broadcasts via messaging)
#    - Click "Start Swap" (locks LEZ, waits for taker)

# 6. In the taker window:
#    - Click "Discover Offers" (finds the maker's offer)
#    - Click an offer to select it
#    - Click "Start Taker" (verifies escrow, locks ETH, waits for preimage, claims LEZ)

# 7. Both windows show "Swap Completed" with matching preimage
```

### Stopping

```bash
# Ctrl-C the `make infra` terminal to stop Anvil + LEZ sequencer
make nwaku-stop   # stop the nwaku Docker container
```

## Architecture

```
┌─────────────────────────────────┐
│     Qt6 UI / swap-cli (bin)     |  <- user interface
├─────────────────────────────────┤
│    swap-ffi (C bridge / cdylib) |  <- FFI layer for UI
├─────────────────────────────────┤
│   Swap orchestration library    |  <- maker/taker/refund flows
├─────────────────────────────────┤
│  Chain monitoring + Messaging   |  <- ETH events + LEZ state + nwaku
├──────────────┬──────────────────┤
│  alloy (ETH) |  nssa_core (LEZ) |
└──────────────┴──────────────────┘
```

| Directory | What |
|---|---|
| `contracts/` | Solidity HTLC contract (Foundry). `lock()`, `claim()`, `refund()` with SHA-256 hashlock. |
| `programs/lez-htlc/` | LEZ HTLC program (Risc0 zkVM). Same Lock/Claim/Refund logic, escrow stored in PDA. |
| `src/eth/` | `EthClient` (alloy) + ETH event watcher (WebSocket subscription). |
| `src/lez/` | `LezClient` (nssa_core sequencer RPC) + LEZ escrow watcher (polling). |
| `src/swap/` | Orchestration: `run_maker()`, `run_taker()`, `refund_lez()`, `refund_eth()`. |
| `src/messaging/` | Waku messaging client for offer discovery (nwaku REST API). |
| `src/cli/` | CLI commands: maker, taker, refund, status, demo, infra. |
| `swap-ffi/` | C FFI bridge (cdylib) exposing swap functions to the Qt6 UI. |
| `ui/` | Qt6/QML application with Config, Maker, Taker, and Refund tabs. |

## Build

```bash
# Build contracts
cd contracts && forge build

# Build the CLI
cargo build --bin swap-cli

# Build with demo/infra commands
cargo build --features demo

# Build the FFI bridge
cd swap-ffi && cargo build

# Configure + build the Qt6 UI
make configure && make build
```

## Test

```bash
# Run all tests
cargo test

# Run with log output
RUST_LOG=info cargo test -- --nocapture

# Run a specific test
cargo test test_atomic_swap_happy_path -- --nocapture
```

### Test breakdown

| Suite | Tests | What's covered |
|---|---|---|
| Unit (`src/`) | 5 | Timelock validation, PDA derivation determinism |
| ETH integration | 4 | Lock/read, lock/claim, lock/refund (time-forwarded), event watcher |
| LEZ integration | 6 | Transfer, lock/escrow, lock/claim, lock/refund, wrong preimage rejected, watcher |
| E2E swap | 5 | Happy path, maker timeout, taker timeout, missing escrow, insufficient amount |
| Messaging | 1 | Publish + fetch offers via nwaku (requires Docker) |

## Makefile Targets

| Target | Description |
|---|---|
| `make configure` | CMake configure for the Qt6 UI (first time only) |
| `make build` | Build the Qt6 UI |
| `make run-maker` | Build + launch UI with maker config (`.env`) |
| `make run-taker` | Build + launch UI with taker config (`.env.taker`) |
| `make demo` | Run the CLI demo (automated end-to-end swap) |
| `make infra` | Start all infrastructure with color-coded logs |
| `make nwaku` | Start nwaku via Docker Compose |
| `make nwaku-stop` | Stop nwaku |
| `make contracts` | Build Solidity contracts |
| `make clean` | Clean UI build |

## `make infra`

Starts all backend services in one terminal with color-coded output:

- **[anvil]** (yellow) — local Ethereum node (auto-funded accounts)
- **[lez]** (cyan) — LEZ sequencer (in-process, auto-funded accounts)
- **[nwaku]** (magenta) — checked at startup (must be running via `make nwaku`)

Automatically:
- Deploys EthHTLC contract to Anvil
- Deploys LEZ HTLC program to the sequencer
- Writes `.env` (maker config) and `.env.taker` (taker config)
- Prints a summary box with all addresses and ports
- Cleans up on Ctrl-C

## Qt6 UI

The UI has four tabs:

**Config** — All swap parameters pre-filled from `.env`. Edit values live before starting a swap. Includes optional nwaku URL for messaging.

**Maker** — Two-step flow when messaging is enabled:
1. "Publish Offer" broadcasts the swap offer via nwaku
2. "Start Swap" locks LEZ and waits for the taker

When messaging is disabled, a single "Start Maker" button runs the full flow.

**Taker** — "Discover Offers" fetches available offers from nwaku. Click an offer to auto-fill the hashlock, then "Start Taker" runs the taker flow. Manual hashlock entry is also supported.

**Refund** — Manual recovery for expired HTLCs. Auto-populated from the last swap result:
- LEZ Refund (maker) — needs hashlock, enforced off-chain (10m default)
- ETH Refund (taker) — needs swap ID, enforced on-chain (5m default)

Each window shows a role badge (green MAKER / blue TAKER) and the window title includes the role.

## CLI Usage

```bash
# Show all commands
swap-cli --help
```

### Maker flow

```bash
# Generate preimage, lock LEZ, watch for ETH, claim ETH
swap-cli maker

# Use a specific preimage (for coordinated testing)
swap-cli maker --preimage <64-char-hex>
```

Prints the hashlock to stdout — share it with the taker.

### Taker flow

```bash
# Verify LEZ escrow, lock ETH, watch for claim, claim LEZ
swap-cli taker --hashlock <64-char-hex>
```

### Refund (manual recovery)

```bash
# Refund LEZ after timelock expiry
swap-cli refund lez --hashlock <64-char-hex>

# Refund ETH after timelock expiry
swap-cli refund eth --swap-id <64-char-hex>
```

### Inspect state

```bash
# Check LEZ escrow state
swap-cli status --hashlock <64-char-hex>

# Check ETH HTLC state
swap-cli status --swap-id <64-char-hex>

# Check both
swap-cli status --hashlock <hex> --swap-id <hex>
```

### Global flags

```bash
--json              # Output as JSON (for scripting)
--env-file <path>   # Override .env file path (default: .env)
```

### Configuration

Every config value can be set via `.env` (or `--env-file`) **or** as a CLI flag. CLI flags override env vars. See `.env.example` for all options.

## Design Decisions

**SHA-256 for hashlock** — not keccak. Required for cross-chain compatibility since LEZ uses `risc0_zkvm::sha`.

**LEZ timelock is off-chain** — LSSA programs have no access to block height or timestamp. The orchestration library checks wall-clock time before submitting refund transactions. The on-chain LEZ program allows refund unconditionally when called by the authorized depositor.

**LEZ escrow funding is two-step** — LSSA balance rules prevent programs from debiting non-owned accounts. The orchestration library first calls Lock (claims the PDA and stores escrow metadata), then transfers funds to the PDA in a separate transaction.

**Messaging is optional** — The swap protocol works without nwaku (manual hashlock exchange). When nwaku is configured, offers are broadcast and discovered automatically. The taker FFI layer polls for the maker's LEZ escrow before starting the swap.

**FFI bridge uses hardcoded header** — `swap-ffi/build.rs` writes `swap_ffi.h` from a template instead of using cbindgen, because cbindgen 0.27 cannot parse `#[unsafe(no_mangle)]` (Rust 2024 edition).

**QThreadPool stack size** — Set to 8MB in `main.cpp`. The default ~512K is insufficient for alloy/tungstenite/TLS WebSocket handshake chain which is deeply recursive.

## Project Status

- [x] Ethereum HTLC smart contract
- [x] LEZ HTLC program (Risc0 zkVM)
- [x] Swap orchestration library
- [x] Standalone CLI (`swap-cli`)
- [x] E2E tests (happy path, timeouts, rejections, balance verification)
- [x] One-command demo (`make demo`)
- [x] Qt6 UI with FFI bridge (`make run-maker` / `make run-taker`)
- [x] Messaging integration (nwaku offer discovery)
- [x] Infrastructure command (`make infra`)
- [ ] Logos Core standalone app (see below)

## Logos Core Standalone App

The current Qt6 UI calls LEZ (nssa_core) and Ethereum (alloy) directly via the Rust FFI bridge. This is a plain Qt6 application — it does **not** use the Logos Core module system.

A proper [Logos Core standalone app](https://ecosystem.logos.co/engineering/application_essentials/logos_core_devex#standalone-app) would embed Logos modules via liblogos SDKs and retrieve them from the Package Manager at build time. Converting would require:

1. Wrapping swap logic as a Logos module (or using the existing `logos-blockchain-module` for LEZ wallet ops)
2. Using liblogos SDKs instead of direct nssa_core/alloy calls
3. An Ethereum module for Logos Core (does not exist yet)

For the PoC, direct chain access is sufficient and avoids blocking on module system availability.
