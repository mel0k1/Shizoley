use std::env;
use std::path::PathBuf;
use std::process::Command;

// builds userspace/init with host g++ and embeds it as raw binary
fn main() {
    let out = PathBuf::from(env::var("OUT_DIR").unwrap());
    let manifest = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let usr = manifest.parent().unwrap().join("userspace");
    let inc = usr.join("include");
    let src = usr.join("src");
    let prog = usr.join("programs/init");

    for u in [
        src.join("syscall.cpp"),
        src.join("io.cpp"),
        src.join("start.cpp"),
        prog.join("main.cpp"),
        usr.join("include/shizoley/syscall.hpp"),
        usr.join("include/shizoley/io.hpp"),
        usr.join("include/shizoley/types.hpp"),
        usr.join("link.ld"),
    ] {
        println!("cargo:rerun-if-changed={}", u.display());
    }

    let cxx = env::var("CXX").unwrap_or_else(|_| "g++".into());
    let units = [
        src.join("syscall.cpp"),
        src.join("io.cpp"),
        src.join("start.cpp"),
        prog.join("main.cpp"),
    ];

    let mut objs = Vec::new();
    for (i, unit) in units.iter().enumerate() {
        let obj = out.join(format!("u{}.o", i));
        let status = Command::new(&cxx)
            .args([
                "-std=c++20",
                "-ffreestanding",
                "-fno-exceptions",
                "-fno-rtti",
                "-fno-stack-protector",
                "-fno-threadsafe-statics",
                "-fno-asynchronous-unwind-tables",
                "-fno-pie",
                "-O2",
                "-Wall",
                "-Wextra",
                "-Werror",
                "-c",
            ])
            .arg("-I")
            .arg(&inc)
            .arg(unit)
            .arg("-o")
            .arg(&obj)
            .status()
            .unwrap_or_else(|e| panic!("kernel build: cannot run {cxx} ({e})"));
        if !status.success() {
            panic!("kernel build: {} failed", unit.display());
        }
        objs.push(obj);
    }

    let elf = out.join("init.elf");
    let status = Command::new(&cxx)
        .args(["-nostdlib", "-static", "-no-pie", "-o"])
        .arg(&elf)
        .arg(format!("-Wl,-T,{}", usr.join("link.ld").display()))
        .args(["-Wl,--build-id=none", "-Wl,-z,noexecstack"])
        .args(&objs)
        .status()
        .unwrap_or_else(|e| panic!("kernel build: userspace link failed ({e})"));
    if !status.success() {
        panic!("kernel build: userspace link failed");
    }

    let bin = out.join("user_init.bin");
    let status = Command::new("objcopy")
        .args(["-O", "binary"])
        .arg(&elf)
        .arg(&bin)
        .status()
        .unwrap_or_else(|e| panic!("kernel build: objcopy not found ({e})"));
    if !status.success() {
        panic!("kernel build: objcopy failed");
    }
}
