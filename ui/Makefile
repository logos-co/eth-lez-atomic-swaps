.PHONY: build run clean configure

build:
	cmake --build build

run: build
	open build/atomic-swaps-ui.app

clean:
	cmake --build build --target clean

configure:
	cmake -B build -DCMAKE_BUILD_TYPE=Debug
