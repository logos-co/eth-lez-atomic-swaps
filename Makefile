.PHONY: contracts demo infra \
       setup localnet-start localnet-stop test

.DEFAULT_GOAL := contracts

# --- logos-blockchain-circuits (project-local, isolated from ~/.logos-blockchain-circuits/) ---
# Circuits are fetched by `lgs setup` into $(CIRCUITS_DIR) (driven by the
# [circuits] block in scaffold.toml) and auto-materialized on demand by the
# scaffold commands that need them (scaffold PR #221); a missing/stale dir is
# diagnosed by `lgs doctor`, so no Makefile-side guard is needed. NOTE: at the
# LEZ v0.2.0 pin no client code reads LOGOS_BLOCKCHAIN_CIRCUITS any more
# (builtin ELFs are checked into the LEZ repo and embedded at build time); the
# export just keeps scaffold's own fetch short-circuited to the project-local
# dir. Candidate for removal once scaffold drops its [circuits] handling for
# v0.2.0+ LEZ pins (upstream scaffold#240 notes this; see
# docs/scaffold-upstream-tracker.md).
CIRCUITS_DIR := $(CURDIR)/.scaffold/lez-cache/circuits

# Exported so every recipe (cargo, logos-scaffold, and their children) uses the
# project-local circuits dir instead of ~/.logos-blockchain-circuits/.
export LOGOS_BLOCKCHAIN_CIRCUITS := $(CIRCUITS_DIR)

# The LEZ v0.2.0 wallet binary reads LEE_WALLET_HOME_DIR; scaffold's own
# subprocess env still exports the older NSSA_WALLET_HOME_DIR name
# (scaffold#240), so without this every wallet CLI child (e.g. `lgs run`'s
# topup step) would silently operate on ~/.lee/wallet instead of the project
# wallet. Exported here so every recipe and its children agree on the
# project-local wallet.
export LEE_WALLET_HOME_DIR := $(CURDIR)/.scaffold/wallet

contracts:
	cd contracts && forge build

# --- Scaffold (LEZ infrastructure) ---

# scripts/scaffold-setup.sh bridges logos-scaffold to the LEZ v0.2.0 repo
# layout (wallet crate moved under lez/) and seeds the default wallet address
# that `lgs run`'s topup step needs (scaffold#240) — see the script header.
setup:
	scripts/scaffold-setup.sh

localnet-start:
	logos-scaffold localnet start

localnet-stop:
	logos-scaffold localnet stop

# `test` / `demo` run scaffold-native `lgs run --profile ...`: the
# [run.profiles.*] blocks in scaffold.toml set deploy = false (scaffold#237,
# fixed by scaffold PR #239, adopted at the pinned scaffold commit), which
# skips scaffold's hardcoded methods/guest/src/bin deploy discovery — the
# app's demo binary deploys the LEZ HTLC program itself. `lgs run` leaves the
# localnet running after the post_deploy hook exits (`stop_on_exit` is a
# pending upstream ask, scaffold#172 / TR-19), so the trap keeps these
# one-shot targets self-contained. The former `demo-makefile` /
# `test-makefile` fallbacks (direct cargo + manual localnet lifecycle) are
# gone: the `lgs run` path is dogfooded and owns build + localnet + wallet
# topup + hook in one command.
test: contracts
	trap 'logos-scaffold localnet stop' EXIT INT TERM; lgs run --profile test

# --- Demo / Infra (headless CLI flow) ---

demo: contracts
	trap 'logos-scaffold localnet stop' EXIT INT TERM; lgs run --profile demo

infra: contracts localnet-start
	trap 'logos-scaffold localnet stop' EXIT INT TERM; cargo run --features demo -- infra

# --- Basecamp modules / UI / two-peer launch (now scaffold-native) ---
#
# The former `swap-module-build`, `swap-ui-build`, `swap-lgx-build`, and
# `swap-ui-run` targets (raw `nix build` / `nix run`) are gone: `lgs basecamp
# build` runs the aggregate module build (writing LGX under
# .scaffold/basecamp/{lgx,portable}/) and `lgs basecamp run swap_ui` runs the
# standalone UI smoke test. See README "Manual Basecamp Run" / "Build Verification".
#
# The former `basecamp-{init,run,clean,paths}-*` targets + scripts/basecamp-instance.sh
# are gone too. Their install path used the hand-rolled `extract_lgx_variant`
# workaround, now obsolete: `lgs basecamp setup` + `lgs basecamp install` install
# the `#lgx-portable` packages into each profile via `lgpm cli-portable` with zero
# variant errors (TR-03 resolved). Their launch path (unpinned Basecamp flake +
# `--user-dir`) never matched the pinned portable stack, so no minimal launch-only
# subset was worth keeping. Launch is `lgs basecamp launch <profile>` on every
# platform: since scaffold PR #238 (adopted at the pinned scaffold commit),
# `launch` sets an absolute LOGOS_DATA_DIR for the macOS portable stack itself,
# so the former scripts/basecamp-launch.sh macOS bridge is gone as well.
