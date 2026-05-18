# Atomic Swaps PoC

Cross-chain atomic swap between LEZ and Ethereum using hash time-locked contracts (HTLCs).

This repo includes:

- a Basecamp UI app for the default manual maker/taker flow
- a headless local demo
- a CLI for maker, taker, status, and refund flows

## Default: Manual Basecamp Run

For local manual testing, use two isolated Basecamp instances: one maker and one taker.

```bash
make setup
make swap-lgx-build
make basecamp-init-maker
make basecamp-init-taker
```

Then start the local chain infrastructure and keep it running:

```bash
make infra
```

In two more terminals, launch the Basecamp instances:

```bash
make basecamp-run-maker
make basecamp-run-taker
```

What each phase does:

| Command | Why it is needed |
|---|---|
| `make setup` | Downloads `logos-blockchain-circuits`, runs `logos-scaffold setup`, and prepares `.scaffold/` wallet/localnet state. |
| `make swap-lgx-build` | Builds installable LGX packages for the `swap` core module and `swap_ui` app. |
| `make basecamp-init-maker` / `make basecamp-init-taker` | Creates isolated Basecamp instances under `.basecamp/` and installs the LGX packages into each one. |
| `make infra` | Starts Anvil and the LEZ localnet, deploys the ETH HTLC contract, and writes `.env` / `.env.taker`. Keep this running. |
| `make basecamp-run-maker` / `make basecamp-run-taker` | Launches the two Basecamp windows with the correct role and env file. |

Re-run `make swap-lgx-build` and both `make basecamp-init-*` targets after changing the module, UI, or Delivery package inputs so each Basecamp instance gets the updated LGX packages.

Use `Ctrl-C` in the `make infra` terminal to stop the local stack. Remove local Basecamp instance state with:

```bash
make basecamp-clean
```

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
- [`logos-scaffold`](https://github.com/logos-co/logos-scaffold) on your `PATH`
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

Install `logos-scaffold` from a local clone:

```bash
git clone https://github.com/logos-co/logos-scaffold.git
cd logos-scaffold
cargo install --path .
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

`make setup` must finish successfully before `make infra` or most other flows. It downloads circuits into `.scaffold/circuits`, runs `logos-scaffold setup`, and creates the local LEZ checkout and wallet under `.scaffold/`.

You do not need `logos-scaffold init`. This repo already ships a checked-in [`scaffold.toml`](scaffold.toml) with the expected relative paths.

To inspect generated LEZ wallet accounts:

```bash
logos-scaffold wallet list --long
```

## Basecamp UI Notes

The UI is a [logos-basecamp](https://github.com/logos-co/logos-basecamp) app, built via [`logos-module-builder`](https://github.com/logos-co/logos-module-builder). It is split into two Logos modules:

- **`swap-module/`**: `type: "core"` universal C++ module wrapping `swap-ffi`. The pure-C++ `SwapImpl` methods are exposed as a typed `Swap` client class for other modules / UIs.
- **`swap-ui/`**: `type: "ui_qml"` Basecamp app with a process-isolated C++ backend (Qt Remote Objects, `.rep` interface) and a QML view. It calls into `swap` via the generated `Swap` client.

Both flakes are standalone and build inside their own subdirectories. Their `flake.lock` files are intentionally kept local/ignored so PR diffs stay focused on source changes.

The two-instance flow uses [`scripts/basecamp-instance.sh`](scripts/basecamp-instance.sh) to create isolated `--user-dir`, HOME, XDG dirs, runtime dirs, and wallets under `.basecamp/maker/` and `.basecamp/taker/`. The runtime/socket dir is forced to `/tmp/lbc-<name>/` to avoid the macOS Unix-socket path limit.

Inspect resolved paths for each instance with:

```bash
make basecamp-paths-maker
make basecamp-paths-taker
```

## Build Verification

These targets verify standalone Nix flake builds. They do not install anything into Basecamp and are not the normal manual app run path.

```bash
make swap-module-build
make swap-ui-build
```

`make swap-module-build` builds the `swap-module/` flake and compiles `swap-ffi` from tracked Rust source. `make swap-ui-build` builds the `swap-ui/` flake, which depends on `swap-module` via `path:../swap-module`.

`swap-module/lib/libswap_ffi.{dylib,so}` is a local platform artifact and is ignored by default. Do not force-add it for Nix builds; `swap-module/flake.nix` builds `swap-ffi` from source.

For quick standalone UI smoke testing outside Basecamp:

```bash
make swap-ui-run
```

This launches the dependency-bundling `logos-standalone-app` runner with the QML inspector on `:3768`. It is not the default manual Basecamp path.

## Headless Demo And CLI Usage

For a quick automated end-to-end swap without the UI:

```bash
make demo
```

`make demo` starts local infrastructure as needed, deploys the Ethereum HTLC to Anvil, runs both maker and taker, and completes a full swap headlessly.

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

Single integration test flow:

```bash
make localnet-start
NSSA_WALLET_HOME_DIR=.scaffold/wallet cargo test --test <file> <name> -- --nocapture
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
| [`scaffold.toml`](scaffold.toml) | Local LEZ checkout, wallet, and localnet configuration |
| `contracts/` | Solidity HTLC contract built with Foundry |
| `programs/lez-htlc/` | LEZ HTLC program built with RISC Zero |
| `src/` | Orchestration, chain clients, maker/taker/refund CLI flows |
| `swap-ffi/` | Rust C-FFI cdylib (`libswap_ffi.{dylib,so}`), consumed by `swap-module` |
| `swap-module/` | Universal C++ core module (Logos `type: "core"`) wrapping `swap-ffi` |
| `swap-ui/` | Basecamp UI app (Logos `type: "ui_qml"`) calling `swap` over Qt Remote Objects |
| `tests/` | Integration tests for the Rust orchestrator |

The headless CLI flow (`swap-cli`, `make demo`, `make infra`) is independent of the UI and works without Nix.

## Common Make Targets

| Command | What it does |
|---|---|
| `make setup` | Download circuits and run `logos-scaffold setup` |
| `make infra` | Start local infra, deploy contracts, and write `.env` files |
| `make demo` | Run a full headless swap |
| `make test` | Build contracts, start localnet, run `cargo test`, stop localnet |
| `make contracts` | Run `forge build` inside `contracts/` |
| `make localnet-start` | Start the LEZ localnet |
| `make localnet-stop` | Stop the LEZ localnet |
| `make swap-vendor-ffi` | Build `swap-ffi` and copy `libswap_ffi.{dylib,so}` into `swap-module/lib/` for ad hoc non-Nix testing |
| `make swap-module-build` | Verify the `swap-module/` Nix flake build |
| `make swap-ui-build` | Verify the `swap-ui/` Nix flake build |
| `make swap-lgx-build` | Build installable LGX packages for Basecamp manual testing |
| `make basecamp-init-maker` | Create/update the isolated maker Basecamp instance and install LGX packages |
| `make basecamp-init-taker` | Create/update the isolated taker Basecamp instance and install LGX packages |
| `make basecamp-run-maker` | Launch the maker Basecamp instance |
| `make basecamp-run-taker` | Launch the taker Basecamp instance |
| `make basecamp-clean` | Remove local maker/taker Basecamp instance state |
| `make swap-ui-run` | Launch `swap-ui` in `logos-standalone-app` for smoke testing only |

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
|                          | alloy  | | nssa     |       |
|                          | (ETH)  | | (LEZ)    |       |
|                          +--------+ +----------+       |
+--------------------------------------------------------+
```

## Design Notes

- SHA-256 is used for the hashlock so both chains share the same primitive.
- The taker locks first, so the ETH timelock is longer and the LEZ timelock is shorter.
- LEZ timelocks are enforced on-chain; local wall-clock checks are just for UX.
- Offer discovery and per-swap coordination for the Basecamp UI run through `logos-delivery-module`; the Rust orchestrator remains focused on on-chain ETH/LEZ state.

For more detail on the messaging side, see [delivery-dogfooding.md](delivery-dogfooding.md).

## Troubleshooting

- `logos-scaffold: command not found`
  Ensure `logos-scaffold` is installed and that `~/.cargo/bin` is on your `PATH`.
- `missing lez at .scaffold/lez-cache/repos/lez/...`
  `make setup` did not finish successfully. Install `logos-scaffold` if needed, then rerun `make setup`.
- `Risc Zero Rust toolchain not found. Try running rzup install rust`
  Install RISC Zero and run `rzup install rust`, then rerun the command that failed.
- Git pull blocked by untracked `scaffold.toml`
  Older clones sometimes had that file gitignored. Move it aside, pull again, then compare your old copy with the checked-in [`scaffold.toml`](scaffold.toml).

## Maintainer Notes

- Bump `CIRCUITS_VERSION` in the [`Makefile`](Makefile) when the `lssa` revision in [`Cargo.toml`](Cargo.toml) needs a newer published `logos-blockchain-circuits` release.
- Bump `[repos.lez].pin` in [`scaffold.toml`](scaffold.toml) when intentionally moving to a different LEZ revision.
