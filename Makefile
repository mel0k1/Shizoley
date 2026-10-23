.PHONY: build image run run-kvm lint fmt clean

build:
	cargo build --manifest-path kernel/Cargo.toml
	cmake -S userspace -B userspace/build
	cmake --build userspace/build

image:
	cd kernel && cargo bootimage

run:
	cd kernel && cargo run

# full vmx round trip needs real VT-x: -enable-kvm -cpu host passes vmx/ept
# through to paparaci (plain tcg reports "vmx unavailable" by design).
# q35 + intel-iommu arms the vt-d unit; e1000 is the dma smoke device
run-kvm:
	qemu-system-x86_64 -enable-kvm -cpu host -machine q35 -device intel-iommu,intremap=off -device e1000,netdev=n0 -netdev user,id=n0 -drive format=raw,file=kernel/target/x86_64-paparaci/debug/bootimage-paparaci.bin -serial stdio

lint:
	cargo clippy --manifest-path kernel/Cargo.toml -- -D warnings
	cargo fmt --manifest-path kernel/Cargo.toml --check
	cppcheck --enable=warning --error-exitcode=1 --std=c++20 userspace/src userspace/programs

fmt:
	cargo fmt --manifest-path kernel/Cargo.toml

clean:
	cargo clean --manifest-path kernel/Cargo.toml
	cmake -E rm -rf userspace/build
