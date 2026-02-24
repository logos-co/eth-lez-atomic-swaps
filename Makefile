.PHONY: build run run-maker run-taker clean configure contracts demo infra nwaku nwaku-stop

UI_BIN = ui/build/atomic-swaps-ui.app/Contents/MacOS/atomic-swaps-ui

build:
	cmake --build ui/build

run: run-maker

run-maker: build
	env $$(cat .env | grep -v '^\#' | xargs) SWAP_ROLE=maker $(UI_BIN) &

run-taker: build
	env $$(cat .env.taker | grep -v '^\#' | xargs) SWAP_ROLE=taker $(UI_BIN) &

clean:
	cmake --build ui/build --target clean

configure:
	cmake -B ui/build -S ui -DCMAKE_BUILD_TYPE=Debug

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
