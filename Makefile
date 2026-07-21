.PHONY: contracts demo demo-makefile infra \
       setup localnet-start localnet-stop test test-makefile \
       check-circuits

# check-circuits sits above `contracts` in this file; keep bare `make`
# building the contracts as before.
.DEFAULT_GOAL := contracts

# --- logos-blockchain-circuits (project-local, isolated from ~/.logos-blockchain-circuits/) ---
# Circuits are fetched by `lgs setup` into $(CIRCUITS_DIR) (driven by the
# [circuits] block in scaffold.toml). This Makefile no longer downloads them;
# it only points every recipe at the project-local dir via the exported env var
# below so cargo / logos-scaffold and their children never collide with a
# developer's pre-existing ~/.logos-blockchain-circuits/.
CIRCUITS_DIR := $(CURDIR)/.scaffold/circuits

# Exported so every recipe (cargo, logos-scaffold, and their children) uses the
# project-local circuits dir instead of ~/.logos-blockchain-circuits/.
export LOGOS_BLOCKCHAIN_CIRCUITS := $(CIRCUITS_DIR)

# Cheap guard: fail early with actionable guidance if `lgs setup` has not fetched
# the circuits yet, instead of letting the LEZ build fail cryptically later.
check-circuits:
	@if [ ! -d "$(CIRCUITS_DIR)" ]; then \
		echo "error: circuits not found at $(CIRCUITS_DIR)."; \
		echo "       Run 'lgs setup' first (it fetches logos-blockchain-circuits)."; \
		exit 1; \
	fi

contracts:
	cd contracts && forge build

# --- Scaffold (LEZ infrastructure) ---

# `lgs setup` fetches the circuits into $(CIRCUITS_DIR) itself, so this target
# must not depend on check-circuits.
setup:
	logos-scaffold setup

localnet-start:
	logos-scaffold localnet start

localnet-stop:
	logos-scaffold localnet stop

# NOTE: `test` / `demo` call `lgs run --profile ...`, which is currently blocked
# in this repo: scaffold's deploy step hardcodes the deployable-program dir as
# <root>/methods/guest/src/bin, while this repo keeps its guest program at
# programs/lez-htlc/methods/guest/, so `lgs run` fails with a missing-program
# error (a configurable program dir is a pending upstream Scaffold ask). Use the
# `-makefile` equivalents below — they are the working headless paths.
test: check-circuits contracts
	lgs run --profile test

test-makefile: check-circuits contracts localnet-start
	NSSA_WALLET_HOME_DIR=.scaffold/wallet cargo test; logos-scaffold localnet stop

# --- Demo / Infra (headless CLI flow) ---

demo: check-circuits contracts
	lgs run --profile demo

demo-makefile: check-circuits contracts
	NSSA_WALLET_HOME_DIR=.scaffold/wallet cargo run --features demo -- demo

infra: check-circuits contracts localnet-start
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
# subset was worth keeping. Launch is `lgs basecamp launch <profile>`; on macOS
# that still needs an absolute LOGOS_DATA_DIR which scaffold cannot yet express in
# a committed scaffold.toml, so the working macOS launch fallback is the committed
# scripts/basecamp-launch.sh bridge (see README "macOS `lgs basecamp launch` note")
# until that gap is fixed upstream. On Linux `lgs basecamp launch <profile>` works
# directly.
