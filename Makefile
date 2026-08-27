.PHONY: contracts demo infra \
       setup localnet-start localnet-stop test basecamp-ui-smoke swap-ui-unit \
       basecamp-dev \
       preflight preflight-full preflight-qmllint preflight-node-tests preflight-expectations-coverage \
       preflight-rust-check preflight-rust-anvil install-hooks

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
	@eth_funds_bin=$$(mktemp /tmp/atomic-swaps-eth-funds-guard.XXXXXX); \
		$(CXX) -std=c++17 -Iswap-ui/src swap-ui/tests/eth_funds_guard_test.cpp -o "$$eth_funds_bin"; \
		"$$eth_funds_bin"
	@balance_gate_bin=$$(mktemp /tmp/atomic-swaps-balance-read-gate.XXXXXX); \
		$(CXX) -std=c++17 -Iswap-ui/src swap-ui/tests/balance_read_gate_test.cpp -o "$$balance_gate_bin"; \
		"$$balance_gate_bin"

# The grep guard turns a scaffold-side layout change into a hard error instead
# of a silently-wrong-directory launch (which looks like "the app opened but my
# module vanished"). It reuses scaffold's own resolved manifest, so the two
# cannot drift apart unnoticed.
basecamp-launch-%:
	@lgs basecamp paths $* --json | grep -q '"modules_dir": "$(BASECAMP_USER_DIR)/modules"' \
	  || { echo "basecamp layout drift: scaffold's modules_dir for profile '$*' is not $(BASECAMP_USER_DIR)/modules"; \
	       echo "compare 'lgs basecamp paths $* --json' with BASECAMP_USER_DIR in the Makefile"; exit 1; }
	LOGOS_USER_DIR=$(BASECAMP_USER_DIR) lgs basecamp launch $*

# --- Preflight (fast local checks — see docs/DEVELOPMENT.md "Before you push") ---
#
# `preflight` mirrors as much of ci.yml's `rust-checks` job as runs in seconds,
# plus the dependency-free swap-ui/JS checks, with NO nix build and NO real
# Basecamp launch. Cold (first checkout, empty target/) it pays for compiling
# the Rust dependency tree once, a few minutes; warm (the target/ dir any
# normal `cargo check`/`cargo test` use leaves behind) it's seconds. Each
# stage below echoes its own name before running, so a failure names the
# exact check that broke instead of a bare "make: *** Error 1".
#
# NOT covered, and why:
#   - the swap-module / swap-ui nix builds (build-modules.yml's `build` job,
#     matrixed over darwin-arm64/linux-amd64/linux-arm64) — there is no fast
#     local form; a real `nix build` on each platform IS the check.
#   - basecamp-ui-runtime's real-Basecamp smoke (tests/basecamp-ui-smoke.mjs)
#     — see `preflight-full` below, which gets partway there.
#
# RISC0_SKIP_BUILD is exported only for the two rust preflight recipes (GNU
# Make target-specific `export`, scoped via the recipe's prerequisites) so it
# never leaks into `test`/`demo`/`infra`, which need the real risc0 guest
# build to run an actual swap — see ci.yml's own comment on the same var.

# Mirrors ci.yml's "verify protocol-v2 EIP-712 fixture" + "cargo check
# swap-ffi" + "cargo test (lib targets)" steps.
preflight-rust-check: export RISC0_SKIP_BUILD := 1
preflight-rust-check:
	@echo "==> scripts/verify-protocol-v2-fixture.sh"
	@scripts/verify-protocol-v2-fixture.sh || { echo "preflight FAILED: scripts/verify-protocol-v2-fixture.sh"; exit 1; }
	@echo "==> cargo check -p swap-ffi --locked"
	@cargo check -p swap-ffi --locked || { echo "preflight FAILED: cargo check -p swap-ffi"; exit 1; }
	@echo "==> cargo test --lib --locked"
	@cargo test --lib --locked || { echo "preflight FAILED: cargo test --lib"; exit 1; }

# Mirrors ci.yml's "forge test (contract invariants)" + "cargo test (anvil
# integration)" steps — the anvil-backed suites that catch a silent ABI/
# binding regression, plus chain_report — the public-trial swap count decoded
# from real logs (see ci.yml's comment on the three suites).
# `contracts` (forge build) is a prerequisite, not duplicated here.
preflight-rust-anvil: export RISC0_SKIP_BUILD := 1
preflight-rust-anvil: contracts
	@echo "==> forge test (contracts)"
	@cd contracts && forge test || { echo "preflight FAILED: forge test"; exit 1; }
	@echo "==> cargo test --locked --test taker_binding --test eth_integration --test chain_report"
	@cargo test --locked --test taker_binding --test eth_integration --test chain_report \
	  || { echo "preflight FAILED: cargo test (anvil integration)"; exit 1; }

# Every dependency-free node contract/unit test in the repo (no Waku node, no
# Qt, no Basecamp) — see each file's own header comment for what it guards.
NODE_PREFLIGHT_TESTS := \
	tests/check-feedback-evidence.mjs \
	tests/check-qml-backend-contract.mjs \
	tests/check-qml-scrollview-content.mjs \
	tests/check-persistence-paths.mjs \
	tests/amount-format.test.mjs \
	tests/insufficient-eth-guard.test.mjs \
	tests/offer-filter.test.mjs \
	tests/maker-balance-refresh-contract.test.mjs \
	tests/basecamp-ui-process-match.test.mjs \
	offer-publisher/rfq.test.mjs \
	offer-publisher/fleet.test.mjs

# Issue #165: the release-content map must have an entry for the version each
# metadata.json declares (see canary/check-expectations-coverage.py).
preflight-expectations-coverage:
	@python3 canary/check-expectations-coverage.py

preflight-node-tests:
	@for f in $(NODE_PREFLIGHT_TESTS); do \
		echo "==> node $$f"; \
		node "$$f" || { echo "preflight FAILED: node $$f"; exit 1; }; \
	done

# qmllint is not wired into CI at all today (nothing on the fast path builds
# Qt), but where it's on PATH it catches real QML syntax/type errors in
# seconds — e.g. from `brew install qt` on macOS, or any nix profile that has
# already built swap-ui once (its qtdeclarative closure carries qmllint).
# Existing QML already trips plenty of qmllint's stylistic Info/Warning
# diagnostics; only genuine errors (bad syntax, unresolved types) make it
# exit non-zero, which is what this gates on. Skips cleanly, rather than
# forcing a Qt install on every contributor, when it isn't available.
preflight-qmllint:
	@if command -v qmllint >/dev/null 2>&1; then \
		echo "==> qmllint swap-ui/src/qml"; \
		qmllint swap-ui/src/qml/*.qml || { echo "preflight FAILED: qmllint"; exit 1; }; \
	else \
		echo "==> qmllint: skipped (not on PATH — brew install qt on macOS, or use a nix profile/shell that provides qt6.qtdeclarative, to enable this check)"; \
	fi

preflight: preflight-qmllint swap-ui-unit preflight-node-tests preflight-expectations-coverage preflight-rust-check preflight-rust-anvil
	@echo "==> preflight: all fast checks passed"

# Adds the one CI job with no fast local equivalent, as far as it goes:
# builds swap + swap_ui from the WORKING TREE and stages them (plus the
# pinned delivery_module) into the isolated dev profile, via
# `scripts/basecamp-dev.sh --no-launch` — the same packaging path
# basecamp-ui-runtime exercises. This is a real nix build: minutes, not
# seconds (~3-6 min warm per docs/local-dev-harness.md, longer cold) — that's
# why it's opt-in via preflight-full and not part of plain `preflight`.
#
# TODO(local UI-text smoke): this only proves the packages BUILD and INSTALL
# cleanly. It does NOT run tests/basecamp-ui-smoke.mjs's actual UI assertions
# — the ones that catch a stale-text regression like today's — because that
# script additionally needs a *launched* Basecamp binary plus the
# logos-qt-mcp inspector test framework (BASECAMP_BIN/BASECAMP_RUNTIME_DIR/
# LOGOS_QT_MCP; see .github/scripts/run-basecamp-ui-smoke.sh), none of which
# `basecamp-dev.sh --no-launch` stages. Wiring that up is a real follow-up
# (candidate: teach basecamp-dev.sh to optionally build logos-qt-mcp and hand
# off to tests/basecamp-ui-smoke.mjs against its own staged profile), not a
# few lines here. Until then, verify UI text changes by hand:
#   make basecamp-dev              # (no ARGS=--no-launch) launches real Basecamp
# then walk the screens your change touched.
preflight-full: preflight
	@echo "==> make basecamp-dev ARGS=--no-launch (working-tree build + stage, no launch)"
	@$(MAKE) basecamp-dev ARGS=--no-launch || { echo "preflight-full FAILED: basecamp-dev --no-launch"; exit 1; }
	@echo "==> preflight-full: build+stage passed — see the TODO above to verify UI text by hand"

# One-time opt-in: symlinks scripts/pre-push into the shared hooks dir (works
# from any worktree of this repo — `git rev-parse --git-path hooks` resolves
# to the same shared .git/hooks either way) so `git push` runs `make
# preflight` first. Never installed automatically; nothing here runs the
# hook until a contributor asks for it.
install-hooks:
	@hooks_dir=$$(git rev-parse --git-path hooks) && \
	mkdir -p "$$hooks_dir" && \
	chmod +x scripts/pre-push && \
	ln -sf "$(CURDIR)/scripts/pre-push" "$$hooks_dir/pre-push" && \
	echo "installed: $$hooks_dir/pre-push -> scripts/pre-push (runs 'make preflight' before every push)"
