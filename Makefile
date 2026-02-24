.PHONY: build run clean configure contracts demo

build:
	cmake --build ui/build

run: build
	open ui/build/atomic-swaps-ui.app

clean:
	cmake --build ui/build --target clean

configure:
	cmake -B ui/build -S ui -DCMAKE_BUILD_TYPE=Debug

contracts:
	cd contracts && forge build

demo: contracts
	cargo run --features demo -- demo
