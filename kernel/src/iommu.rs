use alloc::{boxed::Box, collections::BTreeMap, vec, vec::Vec};
use core::sync::atomic::{AtomicBool, AtomicU16, Ordering};
use spin::{Mutex, Once};
use x86_64::{PhysAddr, VirtAddr};

// vt-d: one driver per remapping unit found in dmar; a domain holds the
// second-level tables that translate a device's dma (the guest's memory
// as its cpu sees it). host devices run pass-through (or an identity
// map, when a unit cannot pass through), so nothing changes for them
// until a device is assigned to a domain. encodings per the vt-d spec,
// legacy format; int remapping stays off (the pic needs none, devices
// the kernel drives poll instead).

const CAP: usize = 0x08;
const ECAP: usize = 0x10;
const GCMD: usize = 0x18;
const GSTS: usize = 0x1C;
const RTADDR: usize = 0x20;
const FSTS: usize = 0x34;
const FECTL: usize = 0x38;
const PMEN: usize = 0x64;
const IQH: usize = 0x80;
const IQT: usize = 0x88;
const IQA: usize = 0x90;

const G_TE: u32 = 1 << 31;
const G_SRTP: u32 = 1 << 30;
const G_QIE: u32 = 1 << 26;
const G_STATES: u32 = G_TE | G_QIE;

const F_PPF: u32 = 1 << 1;
const F_IQE: u32 = 1 << 4;
const F_ERRORS: u32 = 1 | F_IQE | (1 << 5) | (1 << 6);

// leaf bits: r|w, memory-type field left 0 (uc is always correct for
// dma, and the type bits stay clear of the reserved window)
const SL_READ: u64 = 1 << 0;
const SL_WRITE: u64 = 1 << 1;
const SL_ADDR: u64 = 0x000F_FFFF_FFFF_F000;
const SL_LEAF: u64 = SL_READ | SL_WRITE;

const PAGE: u64 = 4096;
const QUEUE_LEN: usize = 256;
const TIMEOUT: u64 = 1_000_000_000;
const HOST_DOMAIN: u16 = 1;

pub struct Fault {
    pub source: u16,
    pub reason: u8,
    pub read: bool,
    pub address: u64,
}

impl Fault {
    fn from_record(low: u64, high: u64) -> Option<Fault> {
        if high & (1 << 63) == 0 {
            return None;
        }
        let reason = (high >> 32) as u8;
        let interrupt = (0x20..0x30).contains(&reason);
        Some(Fault {
            source: high as u16,
            reason,
            read: !interrupt && high & (1 << 62) != 0,
            address: if interrupt { low >> 48 } else { low & !0xFFF },
        })
    }
}

fn reason_str(reason: u8) -> &'static str {
    match reason {
        0x2 => "no context entry",
        0x4 => "beyond the domain",
        0x5 => "not writable",
        0x6 => "not readable",
        0xC => "pte invalid",
        _ => "other",
    }
}

fn now_ns() -> u64 {
    let (lo, hi): (u32, u32);
    unsafe {
        core::arch::asm!("rdtsc", out("eax") lo, out("edx") hi, options(nostack, nomem));
    }
    ((hi as u64) << 32) | lo as u64
}

// a unit's root/context tables: 256 entries of two quadwords; a domain's
// page tables: 512 entries of one
fn ctx_table(phys: PhysAddr) -> &'static mut [[u64; 2]; 256] {
    unsafe { &mut *crate::memory::phys_to_virt(phys).as_mut_ptr::<[[u64; 2]; 256]>() }
}
fn page_table(phys: PhysAddr) -> &'static mut [u64; 512] {
    unsafe { &mut *crate::memory::phys_to_virt(phys).as_mut_ptr::<[u64; 512]>() }
}

struct Queue {
    ring: PhysAddr,
    status: PhysAddr,
    tail: usize,
    seq: u32,
}

struct Tables {
    root: PhysAddr,
    host: PhysAddr,
    own: BTreeMap<u8, PhysAddr>,
}

pub struct Unit {
    pub base: u64,
    regs: VirtAddr,
    cap: u64,
    ecap: u64,
    pub levels: u32,
    pub aw: u64,
    pub pt: bool,
    queue: Mutex<Queue>,
    tables: Mutex<Tables>,
}

impl Unit {
    fn read(&self, r: usize) -> u64 {
        unsafe { core::ptr::read_volatile((self.regs.as_u64() + r as u64) as *const u64) }
    }
    fn read32(&self, r: usize) -> u32 {
        unsafe { core::ptr::read_volatile((self.regs.as_u64() + r as u64) as *const u32) }
    }
    fn write32(&self, r: usize, v: u32) {
        unsafe { core::ptr::write_volatile((self.regs.as_u64() + r as u64) as *mut u32, v) }
    }
    fn write64(&self, r: usize, v: u64) {
        unsafe { core::ptr::write_volatile((self.regs.as_u64() + r as u64) as *mut u64, v) }
    }

    fn wait(&self, done: impl Fn() -> bool) -> bool {
        let start = now_ns();
        while !done() {
            if now_ns() - start > TIMEOUT {
                return done();
            }
            core::hint::spin_loop();
        }
        true
    }

    fn command(&self, bit: u32, on: bool) -> Result<(), &'static str> {
        let states = self.read32(GSTS) & G_STATES;
        self.write32(GCMD, if on { states | bit } else { states & !bit });
        if self.wait(|| (self.read32(GSTS) & bit != 0) == on) {
            Ok(())
        } else {
            Err("command did not take")
        }
    }

    fn new(base: u64) -> Result<Unit, &'static str> {
        let regs = crate::memory::map_mmio(base, 1);
        let cap = unsafe { core::ptr::read_volatile((regs.as_u64() + CAP as u64) as *const u64) };
        let ecap = unsafe { core::ptr::read_volatile((regs.as_u64() + ECAP as u64) as *const u64) };
        if ecap & (1 << 1) == 0 {
            return Err("no queued invalidation");
        }
        if cap & (1 << 4) != 0 {
            return Err("needs write-buffer flushes");
        }
        let (levels, aw) = match (cap >> 8) & 0x1F {
            s if s & 4 != 0 => (4u32, 2u64),
            s if s & 2 != 0 => (3u32, 1u64),
            _ => return Err("walks neither 3- nor 4-level tables"),
        };
        // fault records may sit past the first page: remap wide enough
        let fro = ((cap >> 24) & 0x3FF) as u64 * 16;
        let nfr = ((cap >> 40) & 0xFF) as u64 + 1;
        if fro + nfr * 16 > PAGE {
            crate::memory::map_mmio(base, (fro + nfr * 16).div_ceil(PAGE));
        }
        let mut got: [Option<x86_64::structures::paging::PhysFrame>; 4] = [None; 4];
        for slot in got.iter_mut() {
            match crate::memory::alloc_frame() {
                Some(f) => {
                    unsafe { zero_frame(f) };
                    *slot = Some(f);
                }
                None => {
                    for f in got.iter().flatten() {
                        crate::memory::dealloc_frame(*f);
                    }
                    return Err("no memory for unit tables");
                }
            }
        }
        let mut it = got.into_iter().flatten().map(|f| f.start_address());
        let (ring, status, root, host) = (
            it.next().unwrap(),
            it.next().unwrap(),
            it.next().unwrap(),
            it.next().unwrap(),
        );
        let unit = Unit {
            base,
            regs,
            cap,
            ecap,
            levels,
            aw,
            pt: ecap & (1 << 6) != 0,
            queue: Mutex::new(Queue {
                ring,
                status,
                tail: 0,
                seq: 0,
            }),
            tables: Mutex::new(Tables {
                root,
                host,
                own: BTreeMap::new(),
            }),
        };
        crate::serial_println!(
            "[iommu] unit {:#x}: levels {} aw {} pt {} caching {} coherent {}",
            base,
            levels,
            aw,
            unit.pt,
            cap & (1 << 7) != 0,
            ecap & 1 != 0
        );
        Ok(unit)
    }

    // every bus starts in the host's context table
    fn arm_root(&self, host_entry: [u64; 2]) {
        let t = self.tables.lock();
        for e in ctx_table(t.host).iter_mut() {
            *e = host_entry;
        }
        for e in ctx_table(t.root).iter_mut() {
            *e = [t.host.as_u64() | 1, 0];
        }
        publish();
    }

    fn enable(&self, host_entry: [u64; 2]) -> Result<(), &'static str> {
        let status = self.read32(GSTS);
        if status & G_TE != 0 {
            self.command(G_TE, false)?;
        }
        if status & G_QIE != 0 {
            self.wait(|| self.read(IQH) == self.read(IQT));
            self.command(G_QIE, false)?;
        }
        self.take_faults(|_| {});
        // fault event interrupt stays masked (IM): faults are polled
        self.write32(FECTL, 1 << 1);
        self.arm_root(host_entry);
        let q = self.queue.lock();
        self.write64(IQA, q.ring.as_u64());
        self.write64(IQT, 0);
        drop(q);
        self.command(G_QIE, true)?;
        let root = self.tables.lock().root.as_u64();
        self.write64(RTADDR, root);
        self.command(G_SRTP, true)?;
        self.invalidate(&[desc_context_global(), desc_iotlb_global()])?;
        // protected memory some firmware leaves on goes off
        if self.read32(PMEN) & (1 << 31) != 0 {
            self.write32(PMEN, 0);
        }
        self.command(G_TE, true)?;
        Ok(())
    }

    // descriptors + a wait, polled to completion (or iq error)
    fn invalidate(&self, descs: &[[u64; 2]]) -> Result<(), &'static str> {
        let mut q = self.queue.lock();
        q.seq = q.seq.wrapping_add(1).max(1);
        let (seq, tail) = (q.seq, q.tail);
        let (ring, status) = (q.ring, q.status);
        unsafe {
            core::ptr::write_volatile(crate::memory::phys_to_virt(status).as_mut_ptr::<u32>(), 0);
        }
        let mut n = 0usize;
        for d in descs.iter().copied().chain(core::iter::once([
            0x5 | (1 << 5) | (1 << 6) | ((seq as u64) << 32),
            status.as_u64(),
        ])) {
            unsafe {
                let slot = crate::memory::phys_to_virt(ring + ((tail + n) % QUEUE_LEN) as u64 * 16);
                core::ptr::write_volatile(slot.as_mut_ptr::<[u64; 2]>(), d);
            }
            n += 1;
        }
        q.tail = (tail + n) % QUEUE_LEN;
        self.write64(IQT, (q.tail as u64) << 4);
        let failed = || self.read32(FSTS) & F_IQE != 0;
        let done = self.wait(
            || unsafe { core::ptr::read_volatile(crate::memory::phys_to_virt(status).as_u64() as *const u32) } == seq,
        ) || failed();
        if failed() {
            q.tail = (self.read(IQH) >> 4) as usize % QUEUE_LEN;
            self.write64(IQT, (q.tail as u64) << 4);
            self.write32(FSTS, F_IQE);
            return Err("unit refused an invalidation");
        }
        if done {
            Ok(())
        } else {
            Err("unit did not finish an invalidation")
        }
    }

    fn context(&self, sid: u16) -> [u64; 2] {
        let t = self.tables.lock();
        let frame = t.own.get(&((sid >> 8) as u8)).copied().unwrap_or(t.host);
        ctx_table(frame)[(sid & 0xFF) as usize]
    }

    // (re)place a device's context entry; a present entry goes absent
    // first so the unit never reads half of the new one
    fn set_context(&self, sid: u16, entry: Option<[u64; 2]>) -> Result<(), &'static str> {
        let mut t = self.tables.lock();
        let bus = (sid >> 8) as u8;
        let frame = match t.own.get(&bus) {
            Some(&f) => f,
            None => {
                let Some(f) = crate::memory::alloc_frame() else {
                    return Err("no memory for a context table");
                };
                unsafe {
                    zero_frame(f);
                    *ctx_table(f.start_address()) = *ctx_table(t.host);
                }
                publish();
                let root = t.root;
                unsafe {
                    core::ptr::write_volatile(
                        &mut ctx_table(root)[bus as usize],
                        [f.start_address().as_u64() | 1, 0],
                    );
                }
                publish();
                t.own.insert(bus, f.start_address());
                f.start_address()
            }
        };
        let idx = (sid & 0xFF) as usize;
        let old = ctx_table(frame)[idx];
        let domain_of = |e: [u64; 2]| (e[1] >> 8) as u16;
        if old[0] & 1 != 0 {
            unsafe {
                core::ptr::write_volatile(&mut ctx_table(frame)[idx], [0, 0]);
            }
            publish();
            self.invalidate(&[
                desc_context_device(domain_of(old), sid),
                desc_iotlb_domain(domain_of(old)),
            ])?;
        }
        if let Some(e) = entry {
            unsafe {
                core::ptr::write_volatile(&mut ctx_table(frame)[idx], e);
            }
            publish();
            self.invalidate(&[desc_context_device(domain_of(e), sid)])?;
        }
        Ok(())
    }

    // hand every recorded fault to the reporter, clear the records
    fn take_faults(&self, mut report: impl FnMut(Fault)) {
        let status = self.read32(FSTS);
        if status & F_PPF != 0 {
            let base = ((self.cap >> 24) & 0x3FF) as usize * 16;
            let count = ((self.cap >> 40) & 0xFF) as usize + 1;
            let mut i = ((status >> 8) & 0xFF) as usize % count;
            for _ in 0..count {
                let (low, high) = unsafe {
                    (
                        core::ptr::read_volatile(
                            (self.regs.as_u64() + (base + i * 16) as u64) as *const u64,
                        ),
                        core::ptr::read_volatile(
                            (self.regs.as_u64() + (base + i * 16 + 8) as u64) as *const u64,
                        ),
                    )
                };
                let Some(fault) = Fault::from_record(low, high) else {
                    break;
                };
                report(fault);
                self.write32(base + i * 16 + 12, 1 << 31);
                i = (i + 1) % count;
            }
        }
        if status & F_ERRORS != 0 {
            self.write32(FSTS, status & F_ERRORS);
        }
    }
}

unsafe fn zero_frame(frame: x86_64::structures::paging::PhysFrame) {
    core::ptr::write_bytes(
        crate::memory::phys_to_virt(frame.start_address()).as_mut_ptr::<u8>(),
        0,
        4096,
    );
}

fn publish() {
    if !COHERENT.load(Ordering::Relaxed) {
        unsafe { core::arch::asm!("wbinvd", options(nostack, nomem)) };
    }
}

fn desc_context_global() -> [u64; 2] {
    [0x1 | (1 << 4), 0]
}
fn desc_context_device(domain: u16, sid: u16) -> [u64; 2] {
    [
        0x1 | (3 << 4) | ((domain as u64) << 16) | ((sid as u64) << 32),
        0,
    ]
}
fn desc_iotlb_global() -> [u64; 2] {
    [0x2 | (1 << 4) | (1 << 6) | (1 << 7), 0]
}
fn desc_iotlb_domain(domain: u16) -> [u64; 2] {
    [
        0x2 | (2 << 4) | (1 << 6) | (1 << 7) | ((domain as u64) << 16),
        0,
    ]
}

// a context entry: tables at `root`, domain id, address-width field
fn ctx_tables(root: PhysAddr, domain: u16, aw: u64) -> [u64; 2] {
    [(root.as_u64() & !0xFFF) | 1, aw | ((domain as u64) << 8)]
}
fn ctx_pass_through(aw: u64) -> [u64; 2] {
    [(2 << 2) | 1, aw]
}

static UNITS: Once<Vec<&'static Unit>> = Once::new();
static COHERENT: AtomicBool = AtomicBool::new(true);
static NEXT_DOMAIN: AtomicU16 = AtomicU16::new(2);

pub fn units() -> &'static [&'static Unit] {
    UNITS.get().map(|v| v.as_slice()).unwrap_or(&[])
}

pub fn present() -> bool {
    !units().is_empty()
}

// second-level tables of one domain: devices' dma walks these instead of
// host memory; devices join with attach and leave on drop
pub struct Domain {
    pub id: u16,
    root: PhysAddr,
    levels: u32,
    devices: Vec<(&'static Unit, u16)>,
}

fn index(gpa: u64, level: u32) -> usize {
    ((gpa >> (12 + 9 * level)) & 0x1FF) as usize
}

impl Domain {
    pub fn new() -> Option<Domain> {
        let u = units().first()?;
        let id = NEXT_DOMAIN.fetch_add(1, Ordering::AcqRel).max(1);
        if id == 0 {
            return None; // 16-bit field wrapped
        }
        let root = crate::memory::alloc_frame()?;
        unsafe { zero_frame(root) };
        publish();
        Some(Domain {
            id,
            root: root.start_address(),
            levels: u.levels,
            devices: Vec::new(),
        })
    }

    fn bare(id: u16, u: &Unit) -> Option<Domain> {
        let root = crate::memory::alloc_frame()?;
        unsafe { zero_frame(root) };
        publish();
        Some(Domain {
            id,
            root: root.start_address(),
            levels: u.levels,
            devices: Vec::new(),
        })
    }

    // map one 4k page read+write
    pub fn map(&mut self, gpa: u64, frame: PhysAddr) -> bool {
        let limit = 1u64 << (12 + 9 * self.levels);
        if gpa >= limit || !gpa.is_multiple_of(PAGE) || !frame.as_u64().is_multiple_of(PAGE) {
            return false;
        }
        let mut walk = self.root;
        for level in (1..self.levels).rev() {
            let e = &mut page_table(walk)[index(gpa, level)];
            if *e & (SL_READ | SL_WRITE) == 0 {
                let Some(t) = crate::memory::alloc_frame() else {
                    return false;
                };
                unsafe { zero_frame(t) };
                *e = t.start_address().as_u64() | SL_READ | SL_WRITE;
                publish();
            }
            walk = PhysAddr::new(*e & SL_ADDR);
        }
        unsafe {
            core::ptr::write_volatile(
                &mut page_table(walk)[index(gpa, 0)],
                frame.as_u64() | SL_LEAF,
            );
        }
        publish();
        true
    }

    // the page's translations leave the domain; the frame is reusable
    // only after the invalidation below
    pub fn unmap(&mut self, gpa: u64) {
        let mut walk = self.root;
        for level in (1..self.levels).rev() {
            let e = page_table(walk)[index(gpa, level)];
            if e & (SL_READ | SL_WRITE) == 0 {
                return;
            }
            walk = PhysAddr::new(e & SL_ADDR);
        }
        unsafe {
            core::ptr::write_volatile(&mut page_table(walk)[index(gpa, 0)], 0);
        }
        publish();
        self.commit();
    }

    // units forget cached translations of this domain: always after an
    // unmap (pages become reusable), on map only in caching mode
    pub fn commit(&self) {
        let mut us: Vec<&'static Unit> = self.devices.iter().map(|(u, _)| *u).collect();
        us.dedup_by(|a, b| core::ptr::eq(*a, *b));
        for u in us {
            if let Err(e) = u.invalidate(&[desc_iotlb_domain(self.id)]) {
                crate::serial_println!("[iommu] unit {:#x}: {}", u.base, e);
            }
        }
    }

    // put a pci device (segment 0) in the domain: its context entry moves
    // from the host's to these tables
    pub fn attach(&mut self, sid: u16) -> bool {
        let Some(u) = units().first() else {
            return false;
        };
        let cur = u.context(sid);
        // tt bits 3:2 of a present entry: 2 = pass-through (the host's);
        // tables of another domain refuse a second owner
        if cur[0] & 1 != 0 && cur[0] & 0b1100 != 0b1000 {
            crate::serial_println!("[iommu] device {:#06x} already translated elsewhere", sid);
            return false;
        }
        let entry = ctx_tables(self.root, self.id, u.aw);
        if let Err(e) = u.set_context(sid, Some(entry)) {
            crate::serial_println!("[iommu] unit {:#x}: attach: {}", u.base, e);
            return false;
        }
        crate::serial_println!(
            "[iommu] device {:02x}:{:02x}.{} -> domain {}",
            (sid >> 8) as u8,
            (sid >> 3) & 0x1F,
            sid & 7,
            self.id
        );
        self.devices.push((u, sid));
        true
    }

    // is the device in this domain right now
    pub fn holds(&self, sid: u16) -> bool {
        self.devices.iter().any(|(_, s)| *s == sid)
    }

    pub fn detach(&mut self, sid: u16) -> bool {
        let Some(pos) = self.devices.iter().position(|(_, s)| *s == sid) else {
            return false;
        };
        let (u, _) = self.devices.remove(pos);
        match u.set_context(sid, None) {
            Ok(()) => {
                crate::serial_println!(
                    "[iommu] device {:02x}:{:02x}.{} out of domain {} (reaches nothing)",
                    (sid >> 8) as u8,
                    (sid >> 3) & 0x1F,
                    sid & 7,
                    self.id
                );
                true
            }
            Err(e) => {
                crate::serial_println!("[iommu] unit {:#x}: detach: {}", u.base, e);
                false
            }
        }
    }
}

impl Drop for Domain {
    fn drop(&mut self) {
        let devices = core::mem::take(&mut self.devices);
        for (u, sid) in devices {
            let _ = u.set_context(sid, None);
        }
        let mut tables = vec![(self.root, self.levels - 1)];
        while let Some((t, level)) = tables.pop() {
            if level > 0 {
                for e in page_table(t).iter() {
                    if e & (SL_READ | SL_WRITE) != 0 {
                        tables.push((PhysAddr::new(*e & SL_ADDR), level - 1));
                    }
                }
            }
            let _ = crate::memory::dealloc_frame(
                x86_64::structures::paging::PhysFrame::containing_address(t),
            );
        }
    }
}

// bring the units up: host devices pass through (or walk an identity map
// when a unit cannot pass through), every unit translation-enabled
pub fn init(usable_ram: &[(u64, u64)]) {
    let drhds = crate::acpi::dmar_units();
    if drhds.is_empty() {
        crate::serial_println!("[iommu] no dmar — devices unmanaged");
        return;
    }
    let mut ok: Vec<&'static Unit> = Vec::new();
    let mut coherent = true;
    for d in &drhds {
        match Unit::new(d.base) {
            Ok(u) => {
                coherent &= u.ecap & 1 != 0;
                ok.push(Box::leak(Box::new(u)));
            }
            Err(e) => crate::serial_println!("[iommu] unit {:#x}: refused ({})", d.base, e),
        }
    }
    if ok.is_empty() {
        crate::serial_println!("[iommu] no usable unit — devices unmanaged");
        return;
    }
    COHERENT.store(coherent, Ordering::Relaxed);
    let _ = UNITS.call_once(|| ok);
    let mut armed = 0u64;
    for u in units() {
        let host = if u.pt {
            ctx_pass_through(u.aw)
        } else {
            match identity_host(u, usable_ram) {
                Some((root, id)) => ctx_tables(root, id, u.aw),
                None => {
                    crate::serial_println!("[iommu] unit {:#x}: no host domain, stays off", u.base);
                    continue;
                }
            }
        };
        match u.enable(host) {
            Ok(()) => {
                armed += 1;
                crate::serial_println!(
                    "[iommu] unit {:#x}: te on (host {})",
                    u.base,
                    if u.pt { "pass-through" } else { "identity" }
                );
            }
            Err(e) => crate::serial_println!("[iommu] unit {:#x}: enable failed: {}", u.base, e),
        }
    }
    if armed > 0 {
        crate::serial_println!("[iommu] on: {} unit(s) armed", armed);
    } else {
        crate::serial_println!("[iommu] all units failed to arm — devices unmanaged");
    }
}

// identity-map usable ram for units without pass-through; the tables
// live as long as the kernel
fn identity_host(u: &Unit, usable_ram: &[(u64, u64)]) -> Option<(PhysAddr, u16)> {
    let host = Box::leak(Box::new(Domain::bare(HOST_DOMAIN, u)?));
    for (base, end) in usable_ram {
        let mut a = base & !0xFFF;
        while a < *end {
            if !host.map(a, PhysAddr::new(a)) {
                return None;
            }
            a += PAGE;
        }
    }
    Some((host.root, HOST_DOMAIN))
}

// drain faults of every unit into the log; true when any was seen
pub fn drain_faults() -> bool {
    let mut any = false;
    for u in units() {
        u.take_faults(|f| {
            any = true;
            crate::serial_println!(
                "[iommu] fault: {:#06x} {:#x} {} ({})",
                f.source,
                f.address,
                if f.read { "read" } else { "write" },
                reason_str(f.reason)
            );
        });
    }
    any
}
