# Cold-Clone Verification Checklist

Run through this on a clean machine (or a fresh directory) to verify the README works end-to-end. If any step fails, the README needs fixing before the demo.

## Prerequisites

- [ ] Rust 1.85+ installed (`rustc --version`)
- [ ] Foundry installed (`forge --version`)
- [ ] CMake 3.21+ installed (`cmake --version`)
- [ ] Qt 6.5+ installed (`qmake6 --version` or check `brew info qt@6`)
- [ ] Docker running (`docker info`)

## Clone & Build

```bash
git clone --recurse-submodules https://github.com/logos-co/eth-lez-atomic-swaps.git
cd eth-lez-atomic-swaps
```

- [ ] Clone succeeds with submodules (no errors)
- [ ] `ls programs/lez-htlc/` shows the LEZ HTLC program (submodule populated)

## Contracts

```bash
cd contracts && forge build && forge test && cd ..
```

- [ ] `forge build` compiles without errors
- [ ] `forge test` — all tests pass

## Infrastructure

```bash
make infra
```

- [ ] Anvil starts on port 8545
- [ ] LEZ sequencer starts on port 8080
- [ ] nwaku starts (Docker containers up)
- [ ] EthHTLC contract deployed (address printed)
- [ ] `.env` and `.env.taker` files written

**Keep `make infra` running.** Open a new terminal for the rest.

## CLI Demo (fastest check)

```bash
make demo
```

- [ ] Maker locks LEZ (tx hash printed)
- [ ] Taker locks ETH (swap ID printed)
- [ ] Maker claims ETH (preimage revealed)
- [ ] Taker claims LEZ
- [ ] Both sides report "Swap Complete"

## Standalone UI

```bash
make configure
```

- [ ] CMake configure succeeds (finds Qt6, swap-ffi builds)

```bash
make run-maker
```

- [ ] Maker UI window opens with Config/Maker/Taker/Refund tabs
- [ ] Config panel shows values from `.env`

In a new terminal:

```bash
make run-taker
```

- [ ] Taker UI window opens

**Run the swap:**

1. Maker: Publish Offer -> Start Swap
2. Taker: Discover Offers -> select offer -> Start Taker
3. - [ ] Both UIs show "Swap Complete" with tx hashes

## logos-app Plugin (if logos-app is available)

```bash
make plugin-build
make plugin-run
```

- [ ] Plugin builds without errors
- [ ] logos-app launches with the atomic swap module loaded
- [ ] Swap flow works same as standalone UI

## Cleanup

```bash
# Ctrl-C on make infra terminal
make nwaku-stop
```

- [ ] Docker containers stopped
- [ ] No orphan processes on ports 8545, 8080, 8645

## Common Failures to Watch For

| Symptom | Likely cause |
|---|---|
| `make infra` hangs | Docker not running |
| CMake can't find Qt6 | Missing `CMAKE_PREFIX_PATH` (macOS) or `qt6-base-dev` (Linux) |
| `make demo` connection refused | `make infra` not running in another terminal |
| Swap hangs at "waiting for ETH lock" | `.env` / `.env.taker` mismatch (stale from previous `make infra` run) |
| LEZ lock silently fails | Nonce conflict — wait a block, retry |
| Submodule directory empty | Cloned without `--recurse-submodules`; run `git submodule update --init --recursive` |
