# Development

Developer path: building from source with `lgs` (logos-scaffold). To just use
the app, see [community-install.md](community-install.md) instead.

## Prerequisites

- Apple Silicon macOS, or Linux `x86_64` / `aarch64` (Intel macOS is
  unsupported — no `logos-blockchain-circuits` bundle for it)
- Rust via [rustup](https://rustup.rs/) (this repo pins `1.93.0`),
  [Foundry](https://book.getfoundry.sh/getting-started/installation), GNU
  `make`, a C/C++ toolchain, [Nix](https://nixos.org/) with flakes, and the
  RISC Zero toolchain (`rzup install rust`)
- `logos-scaffold` (`lgs`) installed from the exact pinned commit — other
  builds fail in confusing ways:

```bash
git clone https://github.com/logos-co/logos-scaffold.git
cd logos-scaffold
git checkout 6789ec04b2ad256186a5894710c419b42d16e479
cargo install --path . --locked --bins
```

## Setup and checks

```bash
git clone --recurse-submodules https://github.com/logos-co/eth-lez-atomic-swaps.git
cd eth-lez-atomic-swaps
make setup   # lgs setup via scripts/scaffold-setup.sh (circuits, LEZ checkout, wallet)
make test    # contracts + lgs run --profile test (cargo test as the hook)
make demo    # full headless end-to-end swap, no UI
```

The first cold run is slow (~50 min across setup/build/install); later runs
reuse caches. `lgs doctor --json` reports the resolved scaffold state.

## Inner loop: run the working tree in Basecamp

```bash
make basecamp-dev              # build swap + swap_ui from the working tree,
                               # install into an isolated dev profile, launch Basecamp
make basecamp-dev ARGS=--skip-build   # relaunch without rebuilding
```

Each build is stamped `-dev.<sha>` so you can verify which build is live. See
[local-dev-harness.md](local-dev-harness.md).

## Two-peer manual flow (maker + taker)

```bash
make setup
lgs basecamp build && lgs basecamp setup && lgs basecamp install
make infra                     # Anvil + LEZ localnet + contracts; keep running
make basecamp-launch-maker     # separate terminal
make basecamp-launch-taker     # separate terminal
```

Always launch through the `make basecamp-launch-*` targets, not bare
`lgs basecamp launch`: Basecamp 0.2.x reads `LOGOS_USER_DIR`, which the
targets set to an absolute per-profile path — unbridged, both peers silently
share one data tree and the project modules never appear. Re-run
`lgs basecamp build` + `lgs basecamp install` after changing module inputs.

## Headless CLI

With `make infra` running:

```bash
cargo run --bin swap-cli -- --env-file .env maker
cargo run --bin swap-cli -- --env-file .env.taker taker
```

`swap-cli` also has `status` and `refund` subcommands. The generated `.env`
files use Anvil dev keys — never reuse them elsewhere.

## Why `[modules.*]` use `git+file:` refs

`swap-module` and `swap-ui` are nested sub-flakes that reference the repo root
via `path:..`. Scaffold absolutizes plain `path:` refs, which makes Nix copy
the sub-flake to the store standalone and breaks those references. A
`git+file:.?dir=<sub>` ref passes through untouched, so each sub-flake's
source is the whole committed repo. Implication: any tracked-file edit changes
the tree hash, and the next `lgs basecamp build` rebuilds both modules
(~10 min cold) — batch your edits.

## Lint

```bash
cargo fmt --all
cargo clippy --all-targets --all-features -- -D warnings
```

## Before you push

CI (`.github/workflows/ci.yml` + `build-modules.yml`) takes ~40 min end to
end — long enough that a stale-UI-text typo or a broken unit test only
surfaces after a full cycle. Most of that is checkable locally in seconds:

```bash
make preflight   # cargo check/test, forge test, node contract/unit tests,
                  # swap-ui-unit, qmllint, release-content map coverage —
                  # no nix build, no Basecamp launch
```

Cold (empty `target/`) it pays for compiling the Rust dependency tree once —
a few minutes. Warm, it's seconds. It fails on the first broken check and
prints which one.

Not covered: the swap-module/swap-ui nix builds (×3 platforms) and the real-
Basecamp UI smoke — there's no fast local form for those, see
`make preflight-full`'s header comment in the `Makefile` for how far that
gets and what's still a manual step.

To run it automatically before every `git push`:

```bash
make install-hooks   # one-time, opt-in: symlinks scripts/pre-push into .git/hooks
```

Skip it for one push with `git push --no-verify`; remove it by deleting the
symlink `git rev-parse --git-path hooks` points at.
