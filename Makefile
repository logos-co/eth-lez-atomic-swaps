.PHONY: contracts demo infra \
       setup localnet-start localnet-stop test circuits \
       swap-vendor-ffi swap-module-build swap-ui-build swap-lgx-build swap-ui-run \
       basecamp-init-maker basecamp-init-taker \
       basecamp-run-maker basecamp-run-taker \
       basecamp-clean basecamp-paths-maker basecamp-paths-taker

UNAME := $(shell uname -s)
UNAME_M := $(shell uname -m)

# --- logos-blockchain-circuits (project-local, isolated from ~/.logos-blockchain-circuits/) ---
# Bump this when the lssa pin (scaffold.toml) requires a newer circuits release.
CIRCUITS_VERSION := v0.4.2
CIRCUITS_DIR     := $(CURDIR)/.scaffold/circuits

# Map host to upstream release platform string.
ifeq ($(UNAME),Darwin)
  ifeq ($(UNAME_M),arm64)
    CIRCUITS_PLATFORM := macos-aarch64
  endif
endif
ifeq ($(UNAME),Linux)
  ifeq ($(UNAME_M),x86_64)
    CIRCUITS_PLATFORM := linux-x86_64
  endif
  ifeq ($(UNAME_M),aarch64)
    CIRCUITS_PLATFORM := linux-aarch64
  endif
endif

CIRCUITS_URL := https://github.com/logos-blockchain/logos-blockchain-circuits/releases/download/$(CIRCUITS_VERSION)/logos-blockchain-circuits-$(CIRCUITS_VERSION)-$(CIRCUITS_PLATFORM).tar.gz

# Exported so every recipe (cargo, logos-scaffold, and their children) uses the
# project-local circuits dir instead of ~/.logos-blockchain-circuits/.
export LOGOS_BLOCKCHAIN_CIRCUITS := $(CIRCUITS_DIR)

contracts:
	cd contracts && forge build

# --- Scaffold (LEZ infrastructure) ---

# Fetch logos-blockchain-circuits release into $(CIRCUITS_DIR). Idempotent:
# a VERSION file matching $(CIRCUITS_VERSION) short-circuits the download.
# The exported LOGOS_BLOCKCHAIN_CIRCUITS env var points lssa (and any cargo
# build scripts in this repo) at this dir, avoiding collisions with a
# developer's pre-existing ~/.logos-blockchain-circuits/.
circuits:
	@if [ -f "$(CIRCUITS_DIR)/VERSION" ] && grep -qx "$(CIRCUITS_VERSION)" "$(CIRCUITS_DIR)/VERSION"; then \
		echo "circuits: $(CIRCUITS_VERSION) already present at $(CIRCUITS_DIR)"; \
		exit 0; \
	fi; \
	if [ -z "$(CIRCUITS_PLATFORM)" ]; then \
		echo "circuits: unsupported host $(UNAME)/$(UNAME_M). No logos-blockchain-circuits release for this platform (macOS Intel is not published upstream)."; \
		exit 1; \
	fi; \
	echo "circuits: fetching $(CIRCUITS_VERSION) for $(CIRCUITS_PLATFORM)"; \
	rm -rf "$(CIRCUITS_DIR)"; \
	mkdir -p "$(CIRCUITS_DIR)"; \
	tmp=$$(mktemp -t circuits.XXXXXX.tar.gz) && \
	curl -fL --proto '=https' --tlsv1.2 -o "$$tmp" "$(CIRCUITS_URL)" && \
	tar -xzf "$$tmp" -C "$(CIRCUITS_DIR)" --strip-components=1 && \
	rm -f "$$tmp"; \
	got=$$(cat "$(CIRCUITS_DIR)/VERSION" 2>/dev/null || echo "<missing>"); \
	if [ "$$got" != "$(CIRCUITS_VERSION)" ]; then \
		echo "circuits: VERSION mismatch. expected $(CIRCUITS_VERSION), got $$got"; \
		exit 1; \
	fi; \
	echo "circuits: installed $(CIRCUITS_VERSION) at $(CIRCUITS_DIR)"

setup: circuits
	logos-scaffold setup

localnet-start:
	logos-scaffold localnet start

localnet-stop:
	logos-scaffold localnet stop

test: circuits contracts localnet-start
	NSSA_WALLET_HOME_DIR=.scaffold/wallet cargo test; logos-scaffold localnet stop

# --- Demo / Infra (headless CLI flow) ---

demo: circuits contracts
	NSSA_WALLET_HOME_DIR=.scaffold/wallet cargo run --features demo -- demo

infra: circuits contracts localnet-start
	trap 'logos-scaffold localnet stop' EXIT INT TERM; cargo run --features demo -- infra

# --- Logos modules (built via logos-module-builder / Nix) ---
#
# swap-module: type=core, universal C++ wrapping swap-ffi
# swap-ui    : type=ui_qml, Basecamp UI calling the swap module via Qt Remote Objects
#
# Both flakes are standalone. Each builds inside its own subdirectory.

# Build the swap-ffi cdylib into swap-module/lib/ for ad hoc non-Nix testing.
# Nix builds compile swap-ffi from tracked Rust source via swap-module/flake.nix,
# so platform libraries in swap-module/lib/ stay ignored and untracked.
swap-vendor-ffi: circuits
	cargo build --release -p swap-ffi
ifeq ($(UNAME),Darwin)
	@cp target/release/libswap_ffi.dylib swap-module/lib/libswap_ffi.dylib
	@echo "swap-module/lib/libswap_ffi.dylib refreshed"
	@echo "Reminder: keep platform FFI binaries out of git; Nix builds compile swap-ffi from source."
else
	@cp target/release/libswap_ffi.so swap-module/lib/libswap_ffi.so
	@echo "swap-module/lib/libswap_ffi.so refreshed"
	@echo "Reminder: keep platform FFI binaries out of git; Nix builds compile swap-ffi from source."
endif

# Build the swap core module via Nix. The flake builds and stages swap-ffi from
# tracked Rust source; no checked-in libswap_ffi.dylib/.so is required.
swap-module-build:
	cd swap-module && nix build -L

# Build the swap UI via Nix. Pulls swap-module via path:../swap-module input.
swap-ui-build:
	cd swap-ui && nix build -L

# Build LGX packages for installing the core module and UI app into Basecamp.
# Builds both the `#lgx` (`darwin-arm64-dev` variant, used by the
# logos-standalone-app dev shell) and `#lgx-portable` (`darwin-arm64` variant,
# required by the bundled `bin-macos-app` Basecamp because it links a
# `LGPM_PORTABLE_BUILD`-defined PackageManagerLib that only resolves the bare
# host variant). The basecamp-init-* targets install `#lgx-portable`.
swap-lgx-build:
	cd swap-module && nix build .#lgx -L
	cd swap-module && nix build .#lgx-portable -L
	cd swap-ui && nix build .#lgx -L
	cd swap-ui && nix build .#lgx-portable -L

# Launch the UI in logos-standalone-app for smoke testing only.
# Manual testing should use Basecamp with the LGX packages from swap-lgx-build.
swap-ui-run:
	cd swap-ui && nix run .

# --- Two-Basecamp dogfooding (cross-node Delivery testing) ---
#
# Spin up two fully-isolated LogosBasecamp instances under .basecamp/ for
# manual cross-node testing of M1 (offer discovery) and M2 (per-swap
# coordination) on the Delivery channel. Each instance has its own
# LOGOS_DATA_DIR / HOME / XDG / runtime / wallet, so they cannot share
# state. swap-ui already picks a random per-process Delivery portsShift,
# so libp2p / discovery ports do not collide.
#
# Workflow:
#   1. make swap-lgx-build            # ensure swap+UI LGX are current
#   2. make basecamp-init-maker       # creates .basecamp/maker/, installs LGX
#   3. make basecamp-init-taker       # creates .basecamp/taker/, installs LGX
#   4. make infra                     # in a separate terminal (Anvil + LEZ + .env)
#   5. make basecamp-run-maker        # in a separate terminal; auto-loads .env
#   6. make basecamp-run-taker        # in a separate terminal; auto-loads .env.taker
#   7. drive the swap from the GUIs; logs land in .basecamp/<name>/basecamp.log
#
# Re-run basecamp-init-* after rebuilding LGX to refresh the installed copies.

basecamp-paths-maker:
	@scripts/basecamp-instance.sh paths maker

basecamp-paths-taker:
	@scripts/basecamp-instance.sh paths taker

basecamp-init-maker:
	scripts/basecamp-instance.sh init maker

basecamp-init-taker:
	scripts/basecamp-instance.sh init taker

basecamp-run-maker:
	scripts/basecamp-instance.sh run maker

basecamp-run-taker:
	scripts/basecamp-instance.sh run taker

basecamp-clean:
	scripts/basecamp-instance.sh clean maker
	scripts/basecamp-instance.sh clean taker
