use alloc::vec::Vec;
// acpi walk: rsdp scan (ebda + bios rom), rsdt/xsdt, dmar -> drhd unit bases.
// everything below 4g so the mmio window reaches it

const RSDP_SIG: &[u8; 8] = b"RSD PTR ";

fn sum(bytes: &[u8]) -> u8 {
    bytes.iter().fold(0u8, |a, b| a.wrapping_add(*b))
}

// a 16-byte-aligned find in a mapped span
fn scan(base_virt: usize, len: usize, hit: impl Fn(&[u8]) -> bool) -> Option<usize> {
    let buf = unsafe { core::slice::from_raw_parts(base_virt as *const u8, len) };
    ((0..len).step_by(16))
        .find(|&o| len - o >= 24 && hit(&buf[o..]))
        .map(|o| base_virt + o)
}

fn mapped(addr: u64, pages: u64) -> usize {
    crate::memory::map_mmio(addr, pages).as_u64() as usize
}

// returns the rsdp physical address
pub fn find_rsdp() -> Option<u64> {
    // ebda segment from the bda at 0x40e (up to 1 kib)
    let bda = mapped(0x400, 1);
    let ebda_seg = unsafe { ((bda + 0xE) as *const u16).read_unaligned() } as u64;
    if ebda_seg != 0 {
        let base = ebda_seg << 4;
        if (0x8000..0x9_F000).contains(&base) {
            let virt = mapped(base, 1);
            if let Some(a) = scan(virt, 1024, |b| b[..8] == *RSDP_SIG && sum(&b[..20]) == 0) {
                return Some(((a - virt) as u64) + base);
            }
        }
    }
    // bios rom area
    let virt = mapped(0xE_0000, 32);
    scan(virt, 0x2_0000, |b| {
        b[..8] == *RSDP_SIG && sum(&b[..20]) == 0
    })
    .map(|a| ((a - virt) as u64) + 0xE_0000)
}

// a table's header: (virtual base, length); the checksum covers the
// whole table, not just the header
fn table(base_virt: usize) -> Option<(usize, usize)> {
    let hdr = unsafe { core::slice::from_raw_parts(base_virt as *const u8, 36) };
    let len = u32::from_le_bytes([hdr[4], hdr[5], hdr[6], hdr[7]]) as usize;
    if !(36..=(1 << 20)).contains(&len) {
        return None;
    }
    let full = unsafe { core::slice::from_raw_parts(base_virt as *const u8, len) };
    if sum(full) != 0 {
        return None;
    }
    Some((base_virt, len))
}

fn sig_at(base_virt: usize) -> [u8; 4] {
    unsafe { *(base_virt as *const [u8; 4]) }
}

// find an acpi table by signature through rsdt (u32 entries) or xsdt (u64)
fn find_table(root_virt: usize, root_len: usize, wide: bool, sig: &[u8; 4]) -> Option<u64> {
    let entry_size = if wide { 8 } else { 4 };
    let n = (root_len - 36) / entry_size;
    for i in 0..n {
        let phys = if wide {
            unsafe { ((root_virt + 36 + i * 8) as *const u64).read_unaligned() }
        } else {
            (unsafe { ((root_virt + 36 + i * 4) as *const u32).read_unaligned() }) as u64
        };
        if phys == 0 || phys >= 0x1_0000_0000 {
            continue;
        }
        let virt = mapped(phys, 1);
        if let Some((v, len)) = table(virt) {
            if sig_at(v) == *sig {
                let _ = len;
                return Some(phys);
            }
        }
    }
    None
}

// a remapping hardware unit from the dmar table
pub struct Drhd {
    pub base: u64,
}

// dmar: body[0] = host address width, [1] = flags, then structures
pub fn dmar_units() -> Vec<Drhd> {
    let mut out = Vec::new();
    let Some(rsdp_phys) = find_rsdp() else {
        return out;
    };
    let rp = mapped(rsdp_phys, 1);
    let raw = unsafe { core::slice::from_raw_parts(rp as *const u8, 36) };
    let revision = raw[15];
    let (root_phys, _wide) = if revision >= 2 && sum(raw) == 0 {
        let xsdt = u64::from_le_bytes([
            raw[24], raw[25], raw[26], raw[27], raw[28], raw[29], raw[30], raw[31],
        ]);
        if xsdt != 0 && xsdt < 0x1_0000_0000 {
            (xsdt, true)
        } else {
            let rsdt = u32::from_le_bytes([raw[16], raw[17], raw[18], raw[19]]) as u64;
            (rsdt, false)
        }
    } else {
        (
            u32::from_le_bytes([raw[16], raw[17], raw[18], raw[19]]) as u64,
            false,
        )
    };
    if root_phys == 0 || root_phys >= 0x1_0000_0000 {
        return out;
    }
    let rv = mapped(root_phys, 1);
    let Some((_, root_len)) = table(rv) else {
        return out;
    };
    let root_sig = sig_at(rv);
    let dmar_phys = match root_sig.as_slice() {
        b"XSDT" => find_table(rv, root_len, true, b"DMAR"),
        b"RSDT" => find_table(rv, root_len, false, b"DMAR"),
        _ => None,
    };
    let Some(dmar_phys) = dmar_phys else {
        return out;
    };
    let dv = mapped(dmar_phys, 1);
    let Some((_, dmar_len)) = table(dv) else {
        return out;
    };
    // structures walk: type u16 + length u16; drhd = flags(1) pad(1)
    // seg(2) base(8). the body header is 12 bytes (haw, flags, 10 reserved
    // — linux's acpi_table_dmar and qemu both put structures at 48)
    let mut off = 36 + 12;
    while off + 4 <= dmar_len {
        let stype = unsafe { ((dv + off) as *const u16).read_unaligned() };
        let slen = unsafe { ((dv + off + 2) as *const u16).read_unaligned() } as usize;
        if slen < 4 || off + slen > dmar_len {
            break;
        }
        if stype == 0 && slen >= 16 {
            let body = dv + off;
            let base = unsafe { ((body + 8) as *const u64).read_unaligned() };
            if base != 0 && base < 0x1_0000_0000 {
                out.push(Drhd { base });
            }
        }
        off += slen;
    }
    out
}
