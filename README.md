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

```bash
make infra                # start nwaku + Anvil + LEZ sequencer, deploy contracts, write .env files
make configure            # cmake configure (first time only)
make run-maker            # open maker UI
make run-taker            # open taker UI
```

**Maker**: Publish Offer → Start Swap → wait for taker.
**Taker**: Discover Offers → select offer → Start Taker → swap completes.

Stop with Ctrl-C on `make infra`, then `make nwaku-stop` to clean up Docker.

## Architecture

```
┌─────────────────────────────────┐
│     Qt6 UI / swap-cli (bin)     │
├─────────────────────────────────┤
│    swap-ffi (C bridge / cdylib) │
├─────────────────────────────────┤
│   Swap orchestration library    │
├─────────────────────────────────┤
│  Chain monitoring + Messaging   │
├──────────────┬──────────────────┤
│  alloy (ETH) │  nssa_core (LEZ) │
└──────────────┴──────────────────┘
```

| Directory | Description |
|---|---|
| `contracts/` | Solidity HTLC (Foundry) — `lock()`, `claim()`, `refund()` with SHA-256 hashlock |
| `programs/lez-htlc/` | LEZ HTLC program (Risc0 zkVM) — same logic, escrow in PDA |
| `src/` | Orchestration library — ETH/LEZ clients, watchers, maker/taker/refund flows |
| `src/messaging/` | Waku messaging client for offer discovery (nwaku REST API) |
| `swap-ffi/` | C FFI bridge exposing swap functions to the Qt6 UI |
| `ui/` | Qt6/QML app — Config, Maker, Taker, Refund tabs |

## Commands

| Command | Description |
|---|---|
| `make infra` | Start all services, deploy contracts, write `.env` files |
| `make run-maker` / `make run-taker` | Launch UI with maker/taker config |
| `make demo` | Automated CLI demo (no UI needed) |
| `make configure` / `make build` / `make clean` | Qt6 UI build lifecycle |
| `make contracts` | Build Solidity contracts |
| `make nwaku` / `make nwaku-stop` | Start/stop nwaku Docker container |
| `cargo build --features demo` | CLI with demo/infra commands |
| `cd swap-ffi && cargo build` | FFI bridge (cdylib for Qt6 UI) |
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
- **8MB thread stack** — alloy/tungstenite TLS handshake overflows the default 512K QtConcurrent stack
- **Not a Logos Core app** — the UI calls LEZ/ETH directly via FFI, not through the [Logos Core module system](https://ecosystem.logos.co/engineering/application_essentials/logos_core_devex#standalone-app); converting would require liblogos SDKs, a swap Logos module, and an Ethereum module (which doesn't exist yet)

## Status

- [x] Ethereum HTLC smart contract
- [x] LEZ HTLC program (Risc0 zkVM)
- [x] Swap orchestration library + CLI
- [x] E2E tests (happy path, timeouts, rejections)
- [x] Qt6 UI with messaging (offer discovery via nwaku)
- [x] One-command infra (`make infra`) and demo (`make demo`)
- [ ] Logos Core standalone app
