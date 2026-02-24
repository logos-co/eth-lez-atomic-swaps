.PHONY: build run clean configure

build:
	cmake --build ui/build

run: build
	open ui/build/atomic-swaps-ui.app

clean:
	cmake --build ui/build --target clean

configure:
	cmake -B ui/build -S ui -DCMAKE_BUILD_TYPE=Debug
