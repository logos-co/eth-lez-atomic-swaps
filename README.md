# Atomic Swaps PoC

Cross-chain atomic swap between LEZ and Ethereum using hash time-locked contracts (HTLCs).

This repo includes:

- a Basecamp UI app for the default manual maker/taker flow
- a headless local demo
- a CLI for maker, taker, status, and refund flows

## Default: Local Checks

Run setup once from the repo root:

```bash
make setup
```

`make setup` wraps `lgs setup` in [`scripts/scaffold-setup.sh`](scripts/scaffold-setup.sh), which bridges two gaps scaffold has with the pinned LEZ v0.2.0 repo layout (wallet crate under `lez/`, no preconfigured debug wallet account — upstream [Scaffold #240](https://github.com/logos-co/scaffold/issues/240)): it retries setup with a layout symlink and seeds the default wallet address `lgs run`'s topup step needs. Use `make setup` rather than plain `lgs setup` until #240 lands in the adopted pin.

For the headless demo swap and the test suite, use:

```bash
make demo
make test
```

`make demo` runs `lgs run --profile demo`: scaffold builds the project, ensures the localnet is up, and tops up the wallet, then the profile's `post_deploy` hook runs the app's demo binary, which deploys the LEZ HTLC program, starts Anvil, deploys the Ethereum HTLC, and completes a full swap headlessly. `make test` runs `lgs run --profile test` (same pipeline, with `cargo test` as the hook). Both profiles set `deploy = false` in [`scaffold.toml`](scaffold.toml) — scaffold's own deploy step expects `methods/guest/src/bin` and is redundant here because the app deploys its LEZ program itself ([Scaffold #237](https://github.com/logos-co/scaffold/issues/237), fixed by [PR #239](https://github.com/logos-co/scaffold/pull/239) and adopted at the pinned scaffold commit). `lgs run` leaves the localnet running when the hook exits (`stop_on_exit` is a pending upstream ask, [Scaffold #172](https://github.com/logos-co/scaffold/issues/172)), so both Make targets stop it on exit themselves.

## Manual Basecamp Run

For local manual testing, run two isolated Basecamp peers: one maker and one taker. This flow is scaffold-native — `lgs` owns the module build, the portable Basecamp/LGPM build, and the per-profile install.

Build the module artifacts and set up the portable Basecamp stack once:

```bash
make setup
lgs basecamp build
lgs basecamp setup
lgs basecamp install
```

Then start the local chain infrastructure and keep it running:

```bash
make infra
```

In two more terminals, launch the Basecamp peers:

```bash
lgs basecamp launch maker
lgs basecamp launch taker
```

This works the same on macOS and Linux. On macOS the pinned `bin-macos-app` Basecamp ignores XDG isolation and loads its modules from `LOGOS_DATA_DIR`, which must be absolute; since [Scaffold PR #238](https://github.com/logos-co/scaffold/pull/238) (adopted at the pinned scaffold commit) `lgs basecamp launch` computes and sets that absolute per-profile path itself, so the former committed `scripts/basecamp-launch.sh` bridge is gone.

What each phase does:

| Command | Why it is needed |
|---|---|
| `make setup` | Runs `lgs setup` through the [v0.2.0 bridge](scripts/scaffold-setup.sh): fetches `logos-blockchain-circuits` into `.scaffold/lez-cache/circuits` (driven by the `[circuits]` block in [`scaffold.toml`](scaffold.toml)), creates the local LEZ checkout and wallet under `.scaffold/`, and seeds the default wallet address. |
| `lgs basecamp build` | Runs the aggregate module build, producing the `swap`, `swap_ui`, and `delivery_module` LGX artifacts under `.scaffold/basecamp/{lgx,portable}/`. |
| `lgs basecamp setup` | Builds the portable `bin-macos-app` Basecamp (`a746cdbc` / v0.1.1) and the `cli-portable` LGPM (`e5c25989`), then seeds the two Basecamp profiles. |
| `lgs basecamp install` | Installs the three `#lgx-portable` packages (`delivery_module` + `swap` as modules, `swap_ui` as a plugin) into each profile via `lgpm cli-portable`. The portable Basecamp and `lgpm cli-portable` agree on the bare `darwin-arm64` variant, so the install completes with zero variant errors. |
| `make infra` | Starts Anvil and the LEZ localnet, deploys the ETH HTLC contract, and writes `.env` / `.env.taker`. Keep this running. |
| `lgs basecamp launch maker` / `lgs basecamp launch taker` | Launches the two Basecamp windows with the correct role, env file, and (on macOS) an absolute per-profile `LOGOS_DATA_DIR`. |

Re-run `lgs basecamp build` and `lgs basecamp install` after changing the module, UI, or Delivery package inputs so each profile gets the updated LGX packages.

`make infra`, Anvil startup, and the Ethereum HTLC deployment remain app-owned because scaffold does not model an Anvil co-process yet. Use `Ctrl-C` in the `make infra` terminal to stop the local stack.

### Why `[modules.swap]` uses a `git+file:` ref

`scaffold.toml` declares the two project modules with `git+file:` flake refs:

```toml
[modules.swap]
flake = "git+file:.?dir=swap-module#lgx"

[modules.swap_ui]
flake = "git+file:.?dir=swap-ui#lgx"
```

`swap-module` and `swap-ui` are same-repo nested sub-flakes: `swap-ui` depends on `swap-module` via `path:../swap-module`, and `swap-module` builds `swap-ffi` from `path:..` (the repo root). Scaffold's `normalize_flake_ref` absolutizes a `path:./sub` ref to `path:/abs/sub`; Nix then copies that sub-flake to the store standalone, so its transitive `path:..` escapes the copied tree and the build fails with a `/nix/store` path error. A `git+file:.?dir=<sub>` ref is passed through untouched by scaffold — each sub-flake's source becomes the whole committed repo, so the internal `path:..` / `path:../swap-module` references stay in-tree. The flakes themselves are unchanged, so a direct `nix build .#lgx` inside either subdirectory still works.

**Operational implication:** a `git+file:.` ref hashes the whole committed git tree, so any tracked-file edit changes the tree hash. The next `lgs basecamp build` / `lgs basecamp install` therefore rebuilds `swap` and `swap_ui` (the `swap-module` build copies the whole repo as the `swap-ffi` source; a cold rebuild is ~10 min). Batch tracked edits and rebuild once.

## Prerequisites

Supported platforms:

- Apple Silicon macOS (`arm64`)
- Linux `x86_64`
- Linux `aarch64`

Intel macOS is not supported because upstream does not publish a `logos-blockchain-circuits` bundle for `macos-x86_64`.

Required for the default Basecamp UI flow:

- Rust via [rustup](https://rustup.rs/); this repo pins Rust `1.93.0` in [`rust-toolchain.toml`](rust-toolchain.toml)
- [Foundry](https://book.getfoundry.sh/getting-started/installation) (`forge`, `anvil`)
- GNU `make`
- a C/C++ toolchain
- [`logos-scaffold`](https://github.com/logos-co/logos-scaffold) on your `PATH` from commit `6789ec04b2ad256186a5894710c419b42d16e479` (adds the `deploy = false` run-profile toggle and the macOS `lgs basecamp launch` `LOGOS_DATA_DIR` fix on top of the `[circuits]` schema, `lgs basecamp build`, and `lgpm cli-portable` install path this repo depends on)
- the RISC Zero toolchain installed with `rzup install rust`
- [Nix](https://nixos.org/) with flakes enabled

macOS Apple Silicon:

```bash
xcode-select --install
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
curl -L https://foundry.paradigm.xyz | bash && foundryup
curl -L https://risczero.com/install | bash
rzup install rust
sh <(curl -L https://nixos.org/nix/install)
mkdir -p ~/.config/nix && echo "experimental-features = nix-command flakes" >> ~/.config/nix/nix.conf
```

Linux:

```bash
# Ubuntu / Debian
sudo apt install build-essential make

# Fedora
sudo dnf install gcc gcc-c++ make

curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
curl -L https://foundry.paradigm.xyz | bash && foundryup
curl -L https://risczero.com/install | bash
rzup install rust
sh <(curl -L https://nixos.org/nix/install --daemon)
mkdir -p ~/.config/nix && echo "experimental-features = nix-command flakes" >> ~/.config/nix/nix.conf
```

Install `logos-scaffold` and `lgs` from the supported upstream commit:

```bash
git clone https://github.com/logos-co/logos-scaffold.git
cd logos-scaffold
git checkout 6789ec04b2ad256186a5894710c419b42d16e479
cargo install --path . --locked --bins
```

The workspace [`.cargo/config.toml`](.cargo/config.toml) contains the macOS `aarch64` linker flags used by the Rust/LEZ build.

## Clone And Setup

Clone with submodules:

```bash
git clone --recurse-submodules https://github.com/logos-co/eth-lez-atomic-swaps.git
cd eth-lez-atomic-swaps
```

If you already cloned without submodules:

```bash
git submodule update --init --recursive
```

Run setup once from the repo root:

```bash
make setup
```

`make setup` must finish successfully before `make infra` or most other flows. It runs `lgs setup` through the [v0.2.0 bridge](scripts/scaffold-setup.sh) (see [Local Checks](#default-local-checks)): fetching circuits into `.scaffold/lez-cache/circuits` (from the `[circuits]` block in `scaffold.toml`), running the scaffold LEZ setup, creating the local LEZ checkout and wallet under `.scaffold/`, and seeding the default wallet address.

You do not need `lgs init`. This repo already ships a checked-in [`scaffold.toml`](scaffold.toml) with the expected relative paths.

To inspect generated LEZ wallet accounts:

```bash
lgs wallet list --long
```

## Basecamp UI Notes

The UI is a [logos-basecamp](https://github.com/logos-co/logos-basecamp) app, built via [`logos-module-builder`](https://github.com/logos-co/logos-module-builder). It is split into two Logos modules:

- **`swap-module/`**: `type: "core"` universal C++ module wrapping `swap-ffi`. The pure-C++ `SwapImpl` methods are exposed as a typed `Swap` client class for other modules / UIs.
- **`swap-ui/`**: `type: "ui_qml"` Basecamp app with a process-isolated C++ backend (Qt Remote Objects, `.rep` interface) and a QML view. It calls into `swap` via the generated `Swap` client.

Both flakes are standalone and build inside their own subdirectories. Their `flake.lock` files are intentionally kept local/ignored so PR diffs stay focused on source changes.

The two-peer flow uses scaffold's `[basecamp.profiles.*]` blocks in [`scaffold.toml`](scaffold.toml) (`maker` and `taker`) to give each peer isolated XDG dirs, runtime dir, wallet, log, and env file under `.scaffold/basecamp/profiles/<profile>/`. Each profile's runtime dir is forced to `/tmp/lgs-<profile>/` to stay under the macOS Unix-socket path limit.

Inspect the resolved scaffold / circuits / module / profile state with:

```bash
lgs doctor --json
```

## Build Verification

To build the module LGX artifacts without installing them into Basecamp:

```bash
lgs basecamp build
```

`lgs basecamp build` runs the aggregate Nix build for all `[modules.*]` and writes the LGX artifacts under `.scaffold/basecamp/{lgx,portable}/`. It compiles `swap-ffi` from tracked Rust source; `swap-ui` depends on `swap-module` via `path:../swap-module`, resolved through the `git+file:` ref (see [Why `[modules.swap]` uses a `git+file:` ref](#why-modulesswap-uses-a-gitfile-ref)).

`swap-module/lib/libswap_ffi.{dylib,so}` is a local platform artifact and is ignored by default. Do not force-add it for Nix builds; `swap-module/flake.nix` builds `swap-ffi` from source.

For ad hoc non-Nix iteration on `swap-module/` (standalone CMake, clangd, IDEs):

```bash
cd swap-module && nix develop
```

The dev shell pre-builds `swap-ffi` via the flake, symlinks `libswap_ffi.{dylib,so}` into `swap-module/lib/` (so `CMakeLists.txt`'s `find_library(swap_ffi …)` resolves it), and exports `DYLD_LIBRARY_PATH` / `LD_LIBRARY_PATH` / `CMAKE_LIBRARY_PATH` / `CMAKE_INCLUDE_PATH` plus `CMAKE_EXPORT_COMPILE_COMMANDS=ON` so clangd picks up `compile_commands.json` after a `cmake` configure.

For quick standalone UI smoke testing outside Basecamp:

```bash
lgs basecamp run swap_ui
```

This runs `swap_ui` in the dependency-bundling `logos-standalone-app` runner. It is not the two-peer Basecamp path.

## Headless Demo And CLI Usage

For a quick automated end-to-end swap without the UI:

```bash
make demo
```

`make demo` runs `lgs run --profile demo`: scaffold owns build + localnet + wallet topup, then the profile hook runs the app's demo binary (`cargo run --features demo -- demo --no-localnet`), which deploys the LEZ HTLC program, starts app-owned Anvil, deploys the Ethereum HTLC, and completes a full swap headlessly. The Make target stops the localnet when the run finishes — see the note under [Local Checks](#default-local-checks).

For manual CLI use, start the infrastructure and leave it running:

```bash
make infra
```

Then open two more terminals in the repo root:

```bash
cargo run --bin swap-cli -- --env-file .env maker
cargo run --bin swap-cli -- --env-file .env.taker taker
```

Common CLI commands:

```bash
cargo run --bin swap-cli -- --env-file .env maker
cargo run --bin swap-cli -- --env-file .env.taker taker
cargo run --bin swap-cli -- --env-file .env status --swap-id <hex>
cargo run --bin swap-cli -- --env-file .env status --hashlock <hex>
cargo run --bin swap-cli -- --env-file .env refund eth --swap-id <hex>
cargo run --bin swap-cli -- --env-file .env refund lez --hashlock <hex>
```

If you are not using the local stack from `make infra`, start from [`.env.example`](.env.example) and provide your own RPC endpoints, keys, contract address, and LEZ account details.

## Tests

Full test flow:

```bash
make test
```

`make test` builds the contracts, then runs `lgs run --profile test`: scaffold builds the project, ensures the localnet, and tops up the wallet before the profile hook runs `cargo test`; the Make target stops the localnet afterwards — see the note under [Local Checks](#default-local-checks).

Single integration test flow:

```bash
make localnet-start
LEE_WALLET_HOME_DIR=.scaffold/wallet cargo test --test <file> <name> -- --nocapture
make localnet-stop
```

Lint and format:

```bash
cargo fmt --all
cargo clippy --all-targets --all-features -- -D warnings
```

## How The Swap Works

```text
Taker                                          Maker
  1. Generate preimage + hashlock
  2. Lock ETH with a longer timelock
  --------------------------------------------> sees ETH lock
                                     3. Verify ETH lock, then lock LEZ
                                        with a shorter timelock
  4. Claim LEZ, which reveals the preimage
                                     5. Read the preimage and claim ETH
```

If one side stops responding, the timelocks allow refunds.

## Screenshots

**Legacy logos-app plugin:**

| Config | Maker | Taker | Refund |
|--------|-------|-------|--------|
| ![Config](docs/config.png) | ![Maker](docs/maker.png) | ![Taker](docs/taker.png) | ![Refund](docs/refund.png) |

![logos-app plugin](docs/logos-app-plugin.gif)

## Project Layout

| Path | Purpose |
|---|---|
| [`scaffold.toml`](scaffold.toml) | Scaffold config: LEZ checkout, wallet, localnet, circuits, `[modules.*]`, and `[basecamp.profiles.*]` |
| `contracts/` | Solidity HTLC contract built with Foundry |
| `programs/lez-htlc/` | LEZ HTLC program built with RISC Zero |
| `src/` | Orchestration, chain clients, maker/taker/refund CLI flows |
| `swap-ffi/` | Rust C-FFI cdylib (`libswap_ffi.{dylib,so}`), consumed by `swap-module` |
| `swap-module/` | Universal C++ core module (Logos `type: "core"`) wrapping `swap-ffi` |
| `swap-ui/` | Basecamp UI app (Logos `type: "ui_qml"`) calling `swap` over Qt Remote Objects |
| `tests/` | Integration tests for the Rust orchestrator |

The headless CLI flow (`swap-cli`, `make demo`, `make infra`) is independent of the UI and works without Nix.

## Common Commands

Scaffold-native (`lgs`) commands are the primary flow for setup, module builds, and the Basecamp peers:

| Command | What it does |
|---|---|
| `make setup` | Run `lgs setup` via the v0.2.0 bridge: fetch circuits, create the LEZ checkout + wallet, seed the default wallet address |
| `lgs basecamp build` | Build the `swap` / `swap_ui` / `delivery_module` LGX artifacts |
| `lgs basecamp setup` | Build the portable Basecamp + LGPM and seed the profiles |
| `lgs basecamp install` | Install the `#lgx-portable` packages into each profile via `lgpm cli-portable` |
| `lgs basecamp launch <profile>` | Launch a Basecamp peer (`maker` / `taker`) |
| `lgs basecamp run swap_ui` | Run `swap_ui` standalone in `logos-standalone-app` for smoke testing |
| `lgs doctor --json` | Report resolved scaffold / circuits / module / profile state |

The retained Makefile targets wrap the scaffold-native flows and own the app-specific Ethereum/localnet orchestration scaffold does not model yet:

| Command | What it does |
|---|---|
| `make infra` | Start Anvil + the LEZ localnet, deploy contracts, and write `.env` files |
| `make demo` | Run a full headless swap via `lgs run --profile demo`, then stop the localnet |
| `make test` | Build contracts, run `lgs run --profile test` (`cargo test` as the hook), then stop the localnet |
| `make contracts` | Run `forge build` inside `contracts/` |
| `make localnet-start` / `make localnet-stop` | Start / stop the LEZ localnet |
| `cd swap-module && nix develop` | Enter the swap-module dev shell: pre-builds `swap-ffi`, stages `libswap_ffi.{dylib,so}` into `swap-module/lib/`, and exports `DYLD_LIBRARY_PATH` / `LD_LIBRARY_PATH` / `CMAKE_LIBRARY_PATH` / `CMAKE_EXPORT_COMPILE_COMMANDS` for ad hoc non-Nix CMake / clangd / IDE work |

The former `make demo-makefile` / `make test-makefile` fallbacks (direct cargo + manual localnet lifecycle) are gone — `lgs run` owns build, localnet, and wallet topup for both flows (see the note under [Local Checks](#default-local-checks)). The legacy `make swap-lgx-build`, `make swap-module-build`, `make swap-ui-build`, `make swap-ui-run`, and `make basecamp-{init,run,clean}-*` build/launch targets were retired earlier in favor of the `lgs basecamp` commands above, and the macOS launch bridge `scripts/basecamp-launch.sh` is gone as of the scaffold pin bump (see [Manual Basecamp Run](#manual-basecamp-run)).

## Architecture

```text
+--------------------------------------------------------+
| logos-basecamp                                         |
|  +--------------------------------------------------+  |
|  | swap-ui (ui_qml)         |  swap (core)          |  |
|  |  QML view  ---QRO--->    |  C++ universal impl   |  |
|  |  (Basecamp process)      |  (logos_host process) |  |
|  +-------------------------------------+------------+  |
|                                        |               |
|                                        | links         |
|                                        v               |
|                          +------------------------+    |
|                          | libswap_ffi (cdylib)   |    |
|                          +------------------------+    |
|                                        |               |
|                                        v               |
|                          +------------------------+    |
|                          | swap-orchestrator      |    |
|                          | (Rust src/)            |    |
|                          +-----+-------+----------+    |
|                                |       |               |
|                                v       v               |
|                          +-----+--+ +--+-------+       |
|                          | alloy  | | lee      |       |
|                          | (ETH)  | | (LEZ)    |       |
|                          +--------+ +----------+       |
+--------------------------------------------------------+
```

## Documentation

- [FURPS+](FURPS.md) — Functional and non-functional requirements (v0.1, v0.2)
- [ADR](ADR.md) — Architecture Decision Records (v0.1, v0.2)

## Design Notes

- SHA-256 is used for the hashlock so both chains share the same primitive.
- The taker locks first, so the ETH timelock is longer and the LEZ timelock is shorter.
- LEZ timelocks are enforced on-chain; local wall-clock checks are just for UX.
- Offer discovery and per-swap coordination for the Basecamp UI run through `logos-delivery-module`; the Rust orchestrator remains focused on on-chain ETH/LEZ state.

For more detail on the messaging side, see [delivery-dogfooding.md](delivery-dogfooding.md).

## Troubleshooting

- `lgs: command not found` (or `logos-scaffold: command not found`)
  Ensure `logos-scaffold` is installed and that `~/.cargo/bin` is on your `PATH`.
- `missing lez at .scaffold/lez-cache/repos/lez/...`
  `make setup` did not finish successfully. Install `logos-scaffold` if needed, then rerun `make setup`.
- `Risc Zero Rust toolchain not found. Try running rzup install rust`
  Install RISC Zero and run `rzup install rust`, then rerun the command that failed.
- Git pull blocked by untracked `scaffold.toml`
  Older clones sometimes had that file gitignored. Move it aside, pull again, then compare your old copy with the checked-in [`scaffold.toml`](scaffold.toml).
- Maker fails with an escrow-funding error (or, taker-side, the swap waits then errors)
  The LEZ HTLC lock is two transactions — a Lock instruction, then a funds transfer to the escrow PDA. The maker's `lock()` confirms the transfer landed and returns a hard error (after a bounded ~300s poll) if it didn't; the taker's watcher logs rate-limited warnings while an escrow sits unfunded. The usual cause: the maker wallet holds fewer than the swap's 1000 LEZ, so the sequencer rejects the transfer (`Guest panicked: Sender has insufficient balance`, visible in `.scaffold/logs/sequencer.log`). The headless demo (`make demo`) now funds the maker itself with a bounded faucet-claim loop before locking, so this mainly affects the manual Basecamp flow. Remedy there: top up the maker (`LEE_WALLET_HOME_DIR=.scaffold/wallet <lez wallet binary> pinata claim --to <maker account>` — each claim credits 150 LEZ; repeat until the maker holds ≥ 1000) and rerun. Root-caused 2026-07-21; the original silent-infinite-spin failure mode was fixed by the funding-confirmation change that ships in this repo.

## Maintainer Notes

- Bump `[circuits].version` in [`scaffold.toml`](scaffold.toml) when the `lssa` revision in [`Cargo.toml`](Cargo.toml) needs a newer published `logos-blockchain-circuits` release; `lgs setup` fetches the matching bundle.
- Bump `[repos.lez].pin` in [`scaffold.toml`](scaffold.toml) when intentionally moving to a different LEZ revision.
