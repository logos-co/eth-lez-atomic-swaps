# Atomic Swaps PoC

Cross-chain atomic swap between LEZ and Ethereum using hash time-locked contracts (HTLCs).

This repo includes:

- a headless local demo
- a CLI for maker, taker, status, and refund flows
- an optional [`logos-basecamp`](https://github.com/logos-co/logos-basecamp) UI app, built via [`logos-module-builder`](https://github.com/logos-co/logos-module-builder)

## How the swap works

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

## First-time quickstart

If you are new to this repo, follow this exact order:

1. Clone the repo with submodules.
2. Install the required toolchains.
3. Run `make setup`.
4. Run `make demo` for the fastest successful end-to-end swap.
5. Run `make infra` if you want to drive maker and taker yourself.

`make setup` must finish successfully before `make infra` or most other flows.

### Supported platforms

- Apple Silicon macOS (`arm64`)
- Linux `x86_64`
- Linux `aarch64`

Intel macOS is not supported because upstream does not publish a `logos-blockchain-circuits` bundle for `macos-x86_64`.

### Required for the CLI and local demo

- Rust 1.85+ via [rustup](https://rustup.rs/)
- [Foundry](https://book.getfoundry.sh/getting-started/installation) (`forge`, `anvil`)
- GNU `make`
- a C/C++ toolchain
- [`logos-scaffold`](https://github.com/logos-co/logos-scaffold) on your `PATH`
- the RISC Zero toolchain installed with `rzup install rust`

Notes:

- The first full build can take 5-10 minutes because it compiles `libwaku` and the LEZ guest artifacts.
- Docker or Podman is not required for the normal local flow.

### Optional for the Basecamp UI

- [Nix](https://nixos.org/) with flakes enabled

The UI is built via [`logos-module-builder`](https://github.com/logos-co/logos-module-builder), which provides Qt 6, the C++ SDK, and all build tooling through Nix. No host CMake or Qt install is needed.

### 1. Clone

```bash
git clone --recurse-submodules https://github.com/logos-co/eth-lez-atomic-swaps.git
cd eth-lez-atomic-swaps
```

If you already cloned without submodules:

```bash
git submodule update --init --recursive
```

### 2. Install prerequisites

<details><summary><b>macOS (Apple Silicon)</b></summary>

```bash
xcode-select --install
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
curl -L https://foundry.paradigm.xyz | bash && foundryup
curl -L https://risczero.com/install | bash
rzup install rust
```

Install `logos-scaffold` from a local clone:

```bash
git clone https://github.com/logos-co/logos-scaffold.git
cd logos-scaffold
cargo install --path .
```

If you want the optional UI as well, install Nix with flakes:

```bash
sh <(curl -L https://nixos.org/nix/install)
mkdir -p ~/.config/nix && echo "experimental-features = nix-command flakes" >> ~/.config/nix/nix.conf
```

The workspace [`.cargo/config.toml`](.cargo/config.toml) already contains the macOS `aarch64` linker flags needed by `libwaku`.

</details>

<details><summary><b>Linux</b></summary>

For Ubuntu or Debian:

```bash
sudo apt install build-essential make
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
curl -L https://foundry.paradigm.xyz | bash && foundryup
curl -L https://risczero.com/install | bash
rzup install rust
```

For Fedora:

```bash
sudo dnf install gcc gcc-c++ make
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
curl -L https://foundry.paradigm.xyz | bash && foundryup
curl -L https://risczero.com/install | bash
rzup install rust
```

Install `logos-scaffold` from a local clone:

```bash
git clone https://github.com/logos-co/logos-scaffold.git
cd logos-scaffold
cargo install --path .
```

If you want the optional UI as well, install Nix with flakes:

```bash
sh <(curl -L https://nixos.org/nix/install --daemon)
mkdir -p ~/.config/nix && echo "experimental-features = nix-command flakes" >> ~/.config/nix/nix.conf
```

</details>

### 3. Run one-time setup

From the repo root:

```bash
make setup
```

What this does:

- downloads `logos-blockchain-circuits` into `.scaffold/circuits`
- runs `logos-scaffold setup`
- creates the local LEZ checkout and wallet under `.scaffold/`

You do not need `logos-scaffold init`. This repo already ships a checked-in [`scaffold.toml`](scaffold.toml) with the expected relative paths.

If you want to inspect the generated LEZ wallet accounts:

```bash
logos-scaffold wallet list --long
```

### 4. Fastest smoke test: run the built-in demo

```bash
make demo
```

This is the quickest way to verify your machine is set up correctly. It:

- starts the local LEZ network if needed
- deploys the Ethereum HTLC to Anvil
- runs both maker and taker
- completes a full swap without the UI

### 5. Interactive local stack

If you want to run each side yourself, start the infrastructure first and leave it running:

```bash
make infra
```

`make infra` starts:

- Anvil
- the LEZ localnet
- an embedded Waku rendezvous node
- contract deployment
- `.env` and `.env.taker` generation

Use `Ctrl-C` in that terminal to stop everything cleanly.

Then open two more terminals in the repo root.

Maker:

```bash
cargo run --bin swap-cli -- --env-file .env maker
```

Taker:

```bash
cargo run --bin swap-cli -- --env-file .env.taker taker
```

After `cargo build --release`, you can replace `cargo run --bin swap-cli --` with `./target/release/swap-cli`.

## CLI reference

Common commands:

```bash
cargo run --bin swap-cli -- --env-file .env maker
cargo run --bin swap-cli -- --env-file .env.taker taker
cargo run --bin swap-cli -- --env-file .env status --swap-id <hex>
cargo run --bin swap-cli -- --env-file .env status --hashlock <hex>
cargo run --bin swap-cli -- --env-file .env refund eth --swap-id <hex>
cargo run --bin swap-cli -- --env-file .env refund lez --hashlock <hex>
```

If you are not using the local stack from `make infra`, start from [`.env.example`](.env.example) and provide your own RPC endpoints, keys, contract address, and LEZ account details.

## Basecamp UI (optional)

The UI is a [logos-basecamp](https://github.com/logos-co/logos-basecamp) app, built via [`logos-module-builder`](https://github.com/logos-co/logos-module-builder). It is split into two Logos modules:

- **`swap-module/`** — `type: "core"` universal C++ module wrapping `swap-ffi`. Built with `mkLogosModule` + `logos-cpp-generator`. The 13 public methods on `SwapImpl` (the pure-C++ impl class) are auto-exposed as a typed `Swap` client class for other modules / UIs to call.
- **`swap-ui/`** — `type: "ui_qml"` Basecamp app with a process-isolated C++ backend (Qt Remote Objects, `.rep` interface) and a QML view. Calls into `swap` via the generated `Swap` client.

Both flakes are standalone — each builds inside its own subdirectory. Their
`flake.lock` files are intentionally kept local/ignored in this repo so the PR
diff stays focused on source changes; regenerate them locally with `nix build`
when needed.

### First-time UI build

```bash
make swap-vendor-ffi                                  # build libswap_ffi and copy into swap-module/lib/
make swap-module-build                                 # nix build the core swap module
make swap-ui-build                                     # nix build the UI (depends on swap-module via path:)
```

`swap-module/lib/libswap_ffi.{dylib,so}` is a local platform artifact and is
ignored by default. Until `swap-ffi` is built natively inside the Nix flake,
local Nix builds may need that artifact force-added in a throwaway working tree
so the flake source can see it; do not include platform binaries in normal PRs.

### Run the UI

```bash
make swap-ui-run     # launches in logos-standalone-app with the QML inspector on :3768
```

### Smoke-test the UI

`mkLogosQmlModule` auto-detects [`swap-ui/tests/smoke.mjs`](swap-ui/tests/smoke.mjs), but the pinned `logos-standalone-app` `mkPluginTest` runner does not bundle module dependencies yet. Use the dependency-bundling `apps.default` runner instead:

```bash
cd swap-ui
nix build .#test-framework -o result-mcp
nix run . -- --help >/dev/null
system=$(nix eval --raw --impure --expr builtins.currentSystem)
runner=$(nix eval --raw ".#apps.${system}.default.program")
LOGOS_QT_MCP=$(realpath result-mcp) QT_QPA_PLATFORM=offscreen \
  node tests/smoke.mjs --ci "$runner" --verbose
```

In sandboxed agent environments, run the smoke command unsandboxed so Logos can create its local IPC sockets.

To install into Basecamp:

```bash
cd swap-module && nix build .#lgx
cd ../swap-ui   && nix build .#lgx
lgpm install ./result/*.lgx                           # for both
```

Then launch Basecamp; the swap UI shows as a tab and auto-loads its `swap` core dependency.

### Migration status

This UI is an early scaffold. The rich legacy UI from previous iterations of this repo (config panel, maker/taker/refund views, progress steppers) needs to be ported tab-by-tab into [`swap-ui/src/qml/`](swap-ui/src/qml/) as the [`swap_ui.rep`](swap-ui/src/swap_ui.rep) interface grows to cover the property/slot surface they need. Long-running flows in [`swap-module/src/swap_impl.h`](swap-module/src/swap_impl.h) (`runMaker`, `runTaker`, `runMakerLoop`) currently block their dispatcher thread; they should become non-blocking before this is more than a demo.

## Project layout

| Path | Purpose |
|---|---|
| [`scaffold.toml`](scaffold.toml) | Local LEZ checkout, wallet, and localnet configuration |
| `contracts/` | Solidity HTLC contract built with Foundry |
| `programs/lez-htlc/` | LEZ HTLC program built with RISC Zero |
| `src/` | Orchestration, clients, messaging, maker/taker/refund CLI flows |
| `swap-ffi/` | Rust C-FFI cdylib (`libswap_ffi.{dylib,so}`) — consumed by `swap-module` |
| `swap-module/` | Universal C++ core module (Logos `type: "core"`) wrapping `swap-ffi`. Built via `logos-module-builder`. |
| `swap-ui/` | Basecamp UI app (Logos `type: "ui_qml"`) calling `swap` over Qt Remote Objects. |
| `tests/` | Integration tests for the Rust orchestrator |

The headless CLI flow (`swap-cli`, `make demo`, `make infra`, …) is independent of the UI and works without Nix.

## Common make targets

| Command | What it does |
|---|---|
| `make setup` | Download circuits and run `logos-scaffold setup` |
| `make infra` | Start local infra, deploy contracts, and write `.env` files |
| `make demo` | Run a full headless swap |
| `make test` | Build contracts, start localnet, run `cargo test`, stop localnet |
| `make contracts` | Run `forge build` inside `contracts/` |
| `make localnet-start` | Start the LEZ localnet |
| `make localnet-stop` | Stop the LEZ localnet |
| `make swap-vendor-ffi` | Build `swap-ffi` and copy `libswap_ffi.{dylib,so}` into `swap-module/lib/` |
| `make swap-module-build` | Build `swap-module/` via Nix (requires Nix flakes) |
| `make swap-ui-build` | Build `swap-ui/` via Nix |
| `make swap-ui-run` | Launch `swap-ui` in `logos-standalone-app` |

## Architecture

```text
+--------------------------------------------------------+
| logos-basecamp                                         |
|  +--------------------------------------------------+  |
|  | swap-ui (ui_qml)         │  swap (core)          |  |
|  |  QML view  ---QRO--->    │  C++ universal impl   |  |
|  |  (Basecamp process)      │  (logos_host process) |  |
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

## Design notes

- SHA-256 is used for the hashlock so both chains share the same primitive.
- The taker locks first, so the ETH timelock is longer and the LEZ timelock is shorter.
- LEZ timelocks are enforced on-chain; local wall-clock checks are just for UX.
- Waku messaging runs in-process through `libwaku`; there is no separate Docker service.

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

## Maintainer notes

- Bump `CIRCUITS_VERSION` in the [`Makefile`](Makefile) when the `lssa` revision in [`Cargo.toml`](Cargo.toml) needs a newer published `logos-blockchain-circuits` release.
- Bump `[repos.lez].pin` in [`scaffold.toml`](scaffold.toml) when intentionally moving to a different LEZ revision.
