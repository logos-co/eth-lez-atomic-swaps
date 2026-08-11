.PHONY: contracts demo infra \
       setup localnet-start localnet-stop test basecamp-ui-smoke swap-ui-unit \
       basecamp-dev

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
# .scaffold/basecamp/{lgx,portable}/). UI testing always installs those packages
# into the isolated maker/taker profiles and launches Basecamp. See README
# "Manual Basecamp Run" / "Build Verification".
#
# The former `basecamp-{init,run,clean,paths}-*` targets + scripts/basecamp-instance.sh
# are gone too. Their install path used the hand-rolled `extract_lgx_variant`
# workaround, now obsolete: `lgs basecamp setup` + `lgs basecamp install` install
# the `#lgx-portable` packages into each profile via `lgpm cli-portable` with zero
# variant errors (TR-03 resolved). Their launch path (unpinned Basecamp flake +
# `--user-dir`) never matched the pinned portable stack, so no minimal launch-only
# subset was worth keeping.
#
# Launch goes through `make basecamp-launch-<profile>` rather than a bare
# `lgs basecamp launch <profile>`: Basecamp 0.2.x renamed the data-dir override
# and scaffold has not caught up yet. Basecamp resolves its data tree from
# LOGOS_USER_DIR (app/utils/LogosBasecampPaths.h `baseDirectory()`), while
# scaffold's launch still sets only the 0.1.1-era LOGOS_DATA_DIR (scaffold
# PR #238). On macOS Qt's AppDataLocation ignores XDG_DATA_HOME, so unbridged
# every profile collapses onto the single shared
# ~/Library/Application Support/Logos/LogosBasecamp: Basecamp then loads just
# its 3 embedded modules and `swap` / `swap_ui` / `delivery_module` never
# appear. Verified against basecamp 0.2.1 — see docs/scaffold-upstream-tracker.md
# (TR-21). Drop this target once scaffold sets LOGOS_USER_DIR itself.
#
# The value MUST be absolute: Basecamp absolutizes the `--user-dir` *flag* but
# consumes the LOGOS_USER_DIR *env var* verbatim, so a relative value would
# scatter state under Basecamp's cwd instead of failing loudly. `lgs basecamp
# launch` inherits ambient env, which is what lets this bridge work at all.
BASECAMP_USER_DIR = $(CURDIR)/.scaffold/basecamp/profiles/$*/xdg-data/Logos/LogosBasecamp

# Hermetic UI/runtime smoke test. This builds the portable LGX packages,
# installs them into a throwaway user-dir, and drives the real pinned Basecamp
# bundle with its test-only QML inspector enabled. It does not start localnet or
# Anvil and never uses a module-owned app host.
basecamp-ui-smoke:
	bash .github/scripts/run-basecamp-ui-smoke.sh

# LOCAL working-tree module harness for macOS (see docs/local-dev-harness.md).
# Builds swap + swap_ui from the CURRENT working tree, installs them plus the
# pinned delivery_module into an isolated dev profile via scaffold's pinned
# lgpm, stamps a distinct -dev.<sha> version into each manifest (so the corner
# badge proves which build is live), and launches Basecamp against it. This is
# the ~3-6 min inner loop that kills the "Basecamp kept loading a stale module"
# failure class. Unlike `basecamp-ui-smoke` it drives the developer's real
# Basecamp interactively (no headless inspector); unlike `lgs basecamp launch`
# it does NOT scrub+replay, so the dev-version stamp survives. Pass extra flags
# through ARGS, e.g. `make basecamp-dev ARGS=--skip-build`.
basecamp-dev:
	bash scripts/basecamp-dev.sh $(ARGS)

# Pure, headless swap-ui logic tests. These compile only dependency-free
# helpers; they do not launch Basecamp or any module-owned application host.
swap-ui-unit:
	@balance_bin=$$(mktemp /tmp/atomic-swaps-balance-refresh.XXXXXX); \
		$(CXX) -std=c++17 -Iswap-ui/src swap-ui/tests/balance_refresh_coordinator_test.cpp -o "$$balance_bin"; \
		"$$balance_bin"
	@timelock_bin=$$(mktemp /tmp/atomic-swaps-timelock.XXXXXX); \
		$(CXX) -std=c++17 -Iswap-ui/src swap-ui/tests/timelock_math_test.cpp -o "$$timelock_bin"; \
		"$$timelock_bin"
	@offer_venue_bin=$$(mktemp /tmp/atomic-swaps-offer-venue.XXXXXX); \
		$(CXX) -std=c++17 -Iswap-ui/src swap-ui/tests/offer_venue_test.cpp -o "$$offer_venue_bin"; \
		"$$offer_venue_bin"

# The grep guard turns a scaffold-side layout change into a hard error instead
# of a silently-wrong-directory launch (which looks like "the app opened but my
# module vanished"). It reuses scaffold's own resolved manifest, so the two
# cannot drift apart unnoticed.
basecamp-launch-%:
	@lgs basecamp paths $* --json | grep -q '"modules_dir": "$(BASECAMP_USER_DIR)/modules"' \
	  || { echo "basecamp layout drift: scaffold's modules_dir for profile '$*' is not $(BASECAMP_USER_DIR)/modules"; \
	       echo "compare 'lgs basecamp paths $* --json' with BASECAMP_USER_DIR in the Makefile"; exit 1; }
	LOGOS_USER_DIR=$(BASECAMP_USER_DIR) lgs basecamp launch $*
