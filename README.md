# Atomic Swaps PoC

Cross-chain atomic swap between LEZ (lambda) and Ethereum using hash time-locked contracts (HTLCs). Maker sells LEZ for ETH — both sides are trustless with timeout refunds if either party disappears.

## How It Works

```
Maker                                          Taker
  │                                              │
  │  1. Generate secret preimage                 │
  │  2. Lock LEZ (hashlock = SHA256(preimage))   │
  │─────────── share hashlock ──────────────────>│
  │                                              │  3. Verify LEZ escrow
  │                                              │  4. Lock ETH (same hashlock)
  │  5. Claim ETH (reveals preimage on-chain)    │
  │                                              │  6. Claim LEZ (using revealed preimage)
  │                                              │
  ▼              Both sides complete              ▼
```

**Safety guarantees:**
- If the taker never locks ETH, the maker's LEZ timelock expires and they refund.
- If the maker never claims ETH, the taker's ETH timelock expires and they refund.
- The taker verifies the LEZ escrow (state, amount) before locking any ETH.

## Architecture

```
┌─────────────────────────────────┐
│         swap-cli (bin)          │  ← clap CLI, thin wrapper
├─────────────────────────────────┤
│   Swap orchestration library    │  ← maker/taker/refund flows
├─────────────────────────────────┤
│      Chain monitoring           │  ← ETH event watcher + LEZ state watcher
├──────────────┬──────────────────┤
│  alloy (ETH) │  nssa_core (LEZ) │
└──────────────┴──────────────────┘
```

| Directory | What |
|---|---|
| `contracts/` | Solidity HTLC contract (Foundry). `lock()`, `claim()`, `refund()` with SHA-256 hashlock. |
| `programs/lez-htlc/` | LEZ HTLC program (Risc0 zkVM). Same Lock/Claim/Refund logic, escrow stored in PDA. |
| `src/eth/` | `EthClient` (alloy) + ETH event watcher (WebSocket subscription). |
| `src/lez/` | `LezClient` (nssa_core sequencer RPC) + LEZ escrow watcher (polling). |
| `src/swap/` | Orchestration: `run_maker()`, `run_taker()`, `refund_lez()`, `refund_eth()`. |
| `src/cli/` | CLI commands: maker, taker, refund, status. |
| `src/bin/cli.rs` | Entry point for `swap-cli`. |

## Prerequisites

- **Rust** (edition 2024)
- **Foundry** (`forge`) — for building/testing contracts
- **Access to `logos-blockchain/lssa`** on GitHub — LEZ SDK dependencies are pulled via git

## Build

```bash
# Build contracts
cd contracts && forge build

# Build the CLI
cargo build --bin swap-cli

# Build the LEZ HTLC guest program
cd programs/lez-htlc/methods && cargo build
```

## Test

```bash
# Run all tests (20 total)
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
| E2E swap | 5 | See below |

**E2E swap tests** (spin up local Anvil + LEZ sequencer, deploy both contracts):

| Test | Scenario |
|---|---|
| `test_atomic_swap_happy_path` | Full maker + taker flow completes. Verifies LEZ balances (exact), ETH contract drained, taker ETH decreased, maker only spent gas. |
| `test_maker_refunds_on_timeout` | Taker never shows. Maker's LEZ timelock expires, auto-refunds. LEZ balance fully restored. |
| `test_taker_refunds_on_timeout` | Maker never claims. Taker's ETH timelock expires, refunds ETH. Contract balance returns to zero. |
| `test_taker_rejects_missing_escrow` | No LEZ escrow exists. Taker returns `InvalidState` before locking any ETH. |
| `test_taker_rejects_insufficient_escrow_amount` | Maker locks less LEZ than agreed. Taker rejects before locking ETH. |

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

Every config value can be set via `.env` (or `--env-file`) **or** as a CLI flag. CLI flags override env vars.

```bash
# Ethereum
ETH_RPC_URL=wss://sepolia.infura.io/ws/v3/YOUR_KEY
ETH_PRIVATE_KEY=0x...
ETH_HTLC_ADDRESS=0x...

# LEZ
LEZ_SEQUENCER_URL=http://localhost:8080   # default if omitted
LEZ_SIGNING_KEY=<32-byte-hex>
LEZ_HTLC_PROGRAM_ID=<32-byte-hex>

# Swap parameters
LEZ_AMOUNT=1000
ETH_AMOUNT=1000000000000000

# Timelocks — relative, in minutes from now (defaults: LEZ=10, ETH=5)
# LEZ_TIMELOCK_MINUTES=10
# ETH_TIMELOCK_MINUTES=5

# Counterparty
ETH_RECIPIENT_ADDRESS=0x...
LEZ_TAKER_ACCOUNT_ID=<32-byte-hex>

# Optional
# POLL_INTERVAL_MS=2000
```

Or pass directly:

```bash
swap-cli --eth-rpc-url wss://... --eth-private-key 0x... maker
```

## Design Decisions

**SHA-256 for hashlock** — not keccak. Required for cross-chain compatibility since LEZ uses `risc0_zkvm::sha`.

**LEZ timelock is off-chain** — LSSA programs have no access to block height or timestamp. The orchestration library checks wall-clock time before submitting refund transactions. The on-chain LEZ program allows refund unconditionally when called by the authorized depositor.

**LEZ escrow funding is two-step** — LSSA balance rules prevent programs from debiting non-owned accounts. The orchestration library first calls Lock (claims the PDA and stores escrow metadata), then transfers funds to the PDA in a separate transaction.

## Project Status

- [x] Ethereum HTLC smart contract
- [x] LEZ HTLC program (Risc0 zkVM)
- [x] Swap orchestration library
- [x] Standalone CLI (`swap-cli`)
- [x] E2E tests (happy path, timeouts, rejections, balance verification)
- [ ] Logos Core UI (primary interface — Qt6 plugin / Rust SDK)
