.PHONY: contracts demo infra \
       setup localnet-start localnet-stop test circuits \
       swap-vendor-ffi swap-module-build swap-ui-build swap-ui-run

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

# Vendor the swap-ffi cdylib into swap-module/lib/ so Nix can find it during
# `nix build`. The header is already tracked in git (small, stable). The dylib
# is gitignored by default; force-add with `git add -f` to pin a build.
swap-vendor-ffi: circuits
	cargo build --release -p swap-ffi
ifeq ($(UNAME),Darwin)
	@cp target/release/libswap_ffi.dylib swap-module/lib/libswap_ffi.dylib
	@echo "swap-module/lib/libswap_ffi.dylib refreshed"
	@echo "Reminder: 'git add -f swap-module/lib/libswap_ffi.dylib' before 'nix build' for Nix to see it."
else
	@cp target/release/libswap_ffi.so swap-module/lib/libswap_ffi.so
	@echo "swap-module/lib/libswap_ffi.so refreshed"
	@echo "Reminder: 'git add -f swap-module/lib/libswap_ffi.so' before 'nix build' for Nix to see it."
endif

# Build the swap core module via Nix. Requires `swap-vendor-ffi` to have been
# run and the resulting binary to be git-tracked (Nix only sees tracked files).
swap-module-build: swap-vendor-ffi
	cd swap-module && nix build -L

# Build the swap UI via Nix. Pulls swap-module via path:../swap-module input.
swap-ui-build:
	cd swap-ui && nix build -L

# Launch the UI in logos-standalone-app (auto-loads the swap module dependency).
swap-ui-run:
	cd swap-ui && nix run .
