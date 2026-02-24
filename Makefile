.PHONY: build run run-maker run-taker clean configure swap-ffi contracts demo infra nwaku nwaku-stop \
       logos-module-configure logos-module-build logos-module-plugin logos-module-run

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

run-maker: build
	env $$(cat .env | grep -v '^\#' | xargs) SWAP_ROLE=maker $(UI_BIN) &

run-taker: build
	env $$(cat .env.taker | grep -v '^\#' | xargs) SWAP_ROLE=taker $(UI_BIN) &

clean:
	cmake --build ui/build --target clean

contracts:
	cd contracts && forge build

demo: contracts
	cargo run --features demo -- demo

infra: contracts nwaku
	cargo run --features demo -- infra

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
