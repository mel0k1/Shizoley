use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

// builds every userspace program (programs/<name>/main.cpp) with the host
// g++ against the shz runtime, packs the static elfs into a ustar archive
// the kernel embeds and walks as its initrd
fn main() {
    let out = PathBuf::from(env::var("OUT_DIR").unwrap());
    let manifest = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let usr = manifest.parent().unwrap().join("userspace");
    let inc = usr.join("include");
    let src = usr.join("src");
    let programs = usr.join("programs");

    println!("cargo:rerun-if-changed={}", src.display());
    println!("cargo:rerun-if-changed={}", programs.display());
    println!("cargo:rerun-if-changed={}", usr.join("link.ld").display());

    let cxx = env::var("CXX").unwrap_or_else(|_| "g++".into());

    // shared shz runtime objects
    let mut runtime = Vec::new();
    for (i, unit) in ["syscall.cpp", "io.cpp", "start.cpp"].iter().enumerate() {
        runtime.push(compile(
            &cxx,
            &inc,
            &src.join(unit),
            &out.join(format!("r{}.o", i)),
        ));
    }

    // one static elf per programs/<name>/main.cpp
    let mut progs: Vec<PathBuf> = fs::read_dir(&programs)
        .unwrap_or_else(|e| panic!("kernel build: cannot read programs dir: {e}"))
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.join("main.cpp").exists())
        .collect();
    progs.sort();

    let mut images: Vec<(String, Vec<u8>)> = Vec::new();
    for prog in progs {
        let name = prog.file_name().unwrap().to_string_lossy().to_string();
        let main = prog.join("main.cpp");
        println!("cargo:rerun-if-changed={}", main.display());
        let obj = compile(&cxx, &inc, &main, &out.join(format!("p_{}.o", name)));
        let elf = out.join(format!("{}.elf", name));
        let status = Command::new(&cxx)
            .args(["-nostdlib", "-static", "-no-pie", "-o"])
            .arg(&elf)
            .arg(format!("-Wl,-T,{}", usr.join("link.ld").display()))
            .args(["-Wl,--build-id=none", "-Wl,-z,noexecstack"])
            .args(&runtime)
            .arg(&obj)
            .status()
            .unwrap_or_else(|e| panic!("kernel build: cannot link {name} ({e})"));
        if !status.success() {
            panic!("kernel build: link of '{name}' failed");
        }
        images.push((name, fs::read(&elf).expect("kernel build: elf read failed")));
    }
    if images.is_empty() {
        panic!("kernel build: no userspace programs found");
    }

    // ustar archive: 512-byte header + data + padding per file, zero tail
    let mut tar: Vec<u8> = Vec::new();
    for (name, data) in &images {
        tar.extend_from_slice(&tar_header(name, data.len()));
        tar.extend_from_slice(data);
        let pad = (512 - data.len() % 512) % 512;
        tar.extend(std::iter::repeat_n(0u8, pad));
    }
    tar.extend_from_slice(&[0u8; 1024]);
    fs::write(out.join("initrd.tar"), &tar).expect("kernel build: initrd write failed");
}

fn compile(cxx: &str, inc: &Path, unit: &Path, obj: &Path) -> PathBuf {
    let status = Command::new(cxx)
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
        .arg(inc)
        .arg(unit)
        .arg("-o")
        .arg(obj)
        .status()
        .unwrap_or_else(|e| panic!("kernel build: cannot run {cxx} ({e})"));
    if !status.success() {
        panic!("kernel build: {} failed", unit.display());
    }
    obj.to_path_buf()
}

// posix ustar header with a correct checksum so standard tools can read it too
fn tar_header(name: &str, size: usize) -> [u8; 512] {
    let mut h = [0u8; 512];
    h[..name.len()].copy_from_slice(name.as_bytes());
    h[100..107].copy_from_slice(b"0000644");
    h[108..115].copy_from_slice(b"0000000");
    h[116..123].copy_from_slice(b"0000000");
    h[124..135].copy_from_slice(format!("{:011o}", size).as_bytes());
    h[135] = 0;
    h[136..147].copy_from_slice(b"00000000000");
    h[147] = 0;
    h[148..156].copy_from_slice(b"        ");
    h[156] = b'0';
    h[257..262].copy_from_slice(b"ustar");
    h[263..265].copy_from_slice(b"00");
    let sum: u32 = h.iter().map(|&b| b as u32).sum();
    h[148..154].copy_from_slice(format!("{:06o}", sum).as_bytes());
    h[154] = 0;
    h[155] = b' ';
    h
}
