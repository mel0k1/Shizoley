use alloc::vec::Vec;

// ustar archive embedded at link time by build.rs, one file per program;
// spawn(name) maps the matching elf into a fresh process
static INITRD: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/initrd.tar"));

fn octal(bytes: &[u8]) -> Option<u64> {
    let mut v: u64 = 0;
    for &b in bytes {
        match b {
            b'0'..=b'7' => v = v.checked_mul(8)?.checked_add((b - b'0') as u64)?,
            b' ' | 0 => continue,
            _ => return None,
        }
    }
    Some(v)
}

pub fn entries() -> Vec<(&'static str, &'static [u8])> {
    let mut out = Vec::new();
    let mut off = 0usize;
    while off + 512 <= INITRD.len() {
        let hdr = &INITRD[off..off + 512];
        if hdr[0] == 0 {
            break;
        }
        let name_end = hdr[..100].iter().position(|&b| b == 0).unwrap_or(100);
        let Ok(name) = core::str::from_utf8(&hdr[..name_end]) else {
            break;
        };
        let Some(size) = octal(&hdr[124..136]) else {
            break;
        };
        let size = size as usize;
        let data_start = off + 512;
        let data_end = data_start + size;
        if data_end > INITRD.len() {
            break;
        }
        out.push((name, &INITRD[data_start..data_end]));
        off = data_start + size.div_ceil(512) * 512;
    }
    out
}

pub fn lookup(name: &str) -> Option<&'static [u8]> {
    entries()
        .into_iter()
        .find(|(n, _)| *n == name)
        .map(|(_, d)| d)
}
