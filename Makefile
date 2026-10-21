.PHONY: build image run lint fmt clean

build:
	cargo build --manifest-path kernel/Cargo.toml
	cmake -S userspace -B userspace/build
	cmake --build userspace/build

image:
	cd kernel && cargo bootimage

run:
	cd kernel && cargo run

lint:
	cargo clippy --manifest-path kernel/Cargo.toml -- -D warnings
	cargo fmt --manifest-path kernel/Cargo.toml --check
	cppcheck --enable=warning --error-exitcode=1 --std=c++20 userspace/src userspace/programs

fmt:
	cargo fmt --manifest-path kernel/Cargo.toml

clean:
	cargo clean --manifest-path kernel/Cargo.toml
	cmake -E rm -rf userspace/build
