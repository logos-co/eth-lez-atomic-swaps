.PHONY: build run run-maker run-taker clean configure swap-ffi contracts demo infra nwaku nwaku-stop \
       setup localnet-start localnet-stop test \
       logos-module-configure logos-module-build logos-module-plugin logos-module-run \
       plugin-configure plugin-build plugin-install plugin-run plugin-run-maker plugin-run-taker

UNAME := $(shell uname -s)
ifeq ($(UNAME),Darwin)
  UI_BIN = ui/build/atomic-swaps-ui.app/Contents/MacOS/atomic-swaps-ui
  LOGOS_BIN = logos-module/build/lez_atomic_swap_module.app/Contents/MacOS/lez_atomic_swap_module
else
  UI_BIN = ui/build/atomic-swaps-ui
  LOGOS_BIN = logos-module/build/lez_atomic_swap_module
endif

swap-ffi:
	cd swap-ffi && cargo build

configure: swap-ffi
	cmake -B ui/build -S ui -DCMAKE_BUILD_TYPE=Debug

build:
	cmake --build ui/build

run: run-maker

run-maker: configure build
	env $$(cat .env | grep -v '^\#' | xargs) SWAP_ROLE=maker $(UI_BIN) &

run-taker: configure build
	env $$(cat .env.taker | grep -v '^\#' | xargs) SWAP_ROLE=taker $(UI_BIN) &

clean:
	cmake --build ui/build --target clean

contracts:
	cd contracts && forge build

# --- Scaffold (LEZ infrastructure) ---

setup:
	logos-scaffold setup

localnet-start:
	logos-scaffold localnet start

localnet-stop:
	logos-scaffold localnet stop

test: contracts localnet-start
	NSSA_WALLET_HOME_DIR=.scaffold/wallet cargo test; logos-scaffold localnet stop

# --- Demo / Infra ---

demo: contracts nwaku
	NSSA_WALLET_HOME_DIR=.scaffold/wallet cargo run --features demo -- demo

infra: contracts nwaku localnet-start
	trap 'logos-scaffold localnet stop; docker compose down' EXIT INT TERM; cargo run --features demo -- infra

nwaku:
	docker compose up -d

nwaku-stop:
	docker compose down

# --- Logos Core module ---

logos-module-configure:
	cmake -B logos-module/build -S logos-module -DCMAKE_BUILD_TYPE=Debug

logos-module-build: logos-module-configure
	cmake --build logos-module/build

logos-module-plugin:
	cmake -B logos-module/build -S logos-module -DBUILD_PLUGIN=ON -DCMAKE_BUILD_TYPE=Debug
	cmake --build logos-module/build

logos-module-run: logos-module-build
	env $$(cat .env | grep -v '^\#' | xargs) SWAP_ROLE=maker $(LOGOS_BIN) &

# --- logos-app IComponent plugin ---

LOGOS_APP_INTERFACES := $(HOME)/Developer/status/logos-app/app/interfaces
LOGOS_APP_BIN        := $(HOME)/Developer/status/logos-app/result/bin/logos-app
PLUGIN_BUILD         := logos-module/build-plugin
PLUGIN_DIR           := $(HOME)/Library/Application Support/Logos/LogosAppNix/plugins/lez_atomic_swap

# Use the same Nix Qt 6.9.2 that logos-app ships (not Homebrew Qt)
NIX_QTBASE        := /nix/store/a9aq909fc6ymnawnk877qcs4gklzm1c1-qtbase-6.9.2
NIX_QTDECLARATIVE := /nix/store/fn7iqppsl6z7ikbspxnjirwdz345w8mj-qtdeclarative-6.9.2
NIX_QTSHADERTOOLS := /nix/store/awcf75ll0ynkkknwzam9qi6w663y0q9q-qtshadertools-6.9.2
NIX_QTSVG         := /nix/store/6mjqccb1hfr5mffqz80icfvh8w0lvqmf-qtsvg-6.9.2

plugin-configure: swap-ffi
	cmake -B $(PLUGIN_BUILD) -S logos-module \
		-DBUILD_APP_PLUGIN=ON \
		-DLOGOS_APP_INTERFACES_DIR=$(LOGOS_APP_INTERFACES) \
		-DCMAKE_PREFIX_PATH="$(NIX_QTBASE)" \
		-DQT_ADDITIONAL_PACKAGES_PREFIX_PATH="$(NIX_QTDECLARATIVE);$(NIX_QTSHADERTOOLS);$(NIX_QTSVG)" \
		-DQt6QmlTools_DIR=$(NIX_QTDECLARATIVE)/lib/cmake/Qt6QmlTools \
		-DQt6QuickTools_DIR=$(NIX_QTDECLARATIVE)/lib/cmake/Qt6QuickTools \
		-DCMAKE_BUILD_TYPE=Debug

plugin-build: plugin-configure
	cmake --build $(PLUGIN_BUILD)

plugin-install: plugin-build
	@mkdir -p "$(PLUGIN_DIR)"
	cp $(PLUGIN_BUILD)/lez_atomic_swap.dylib "$(PLUGIN_DIR)/"
	@# Always copy latest FFI dylib (prefer release, fall back to debug)
	@if [ -f swap-ffi/target/release/libswap_ffi.dylib ]; then \
		cp swap-ffi/target/release/libswap_ffi.dylib "$(PLUGIN_DIR)/"; \
	elif [ -f swap-ffi/target/debug/libswap_ffi.dylib ]; then \
		cp swap-ffi/target/debug/libswap_ffi.dylib "$(PLUGIN_DIR)/"; \
	fi
ifeq ($(UNAME),Darwin)
	@codesign -fs - "$(PLUGIN_DIR)/lez_atomic_swap.dylib" 2>/dev/null || true
	@codesign -fs - "$(PLUGIN_DIR)/libswap_ffi.dylib" 2>/dev/null || true
endif

plugin-run: plugin-run-maker

plugin-run-maker: plugin-install
	env $$(cat .env | grep -v '^\#' | xargs) SWAP_ROLE=maker $(LOGOS_APP_BIN) &

plugin-run-taker: plugin-install
	env $$(cat .env.taker | grep -v '^\#' | xargs) SWAP_ROLE=taker $(LOGOS_APP_BIN) &
