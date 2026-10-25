use core::sync::atomic::{AtomicBool, AtomicU8, AtomicUsize, Ordering};
use x86_64::instructions::port::Port;
use x86_64::PhysAddr;

// pci config probe + the device-assignment smoke: an e1000 tx ring lives
// in an iommu domain's address space, so the nic's dma (descriptor fetch
// and payload read) goes through the second-level tables — mapped frames
// complete, unmapped ones land in the fault log. tx completion is observed
// through the nic's intx line (the same vector devd later routes into the
// guest), not by polling

const CFG_ADDR: u16 = 0xCF8;
const CFG_DATA: u16 = 0xCFC;

fn cfg_read(sid: u16, off: u8) -> u32 {
    let mut addr: Port<u32> = Port::new(CFG_ADDR);
    let mut data: Port<u32> = Port::new(CFG_DATA);
    unsafe {
        addr.write(0x8000_0000 | ((sid as u32) << 8) | ((off & 0xFC) as u32));
        data.read()
    }
}

fn cfg_write(sid: u16, off: u8, v: u32) {
    let mut addr: Port<u32> = Port::new(CFG_ADDR);
    let mut data: Port<u32> = Port::new(CFG_DATA);
    unsafe {
        addr.write(0x8000_0000 | ((sid as u32) << 8) | ((off & 0xFC) as u32));
        data.write(v)
    }
}

// memory + bus master: without the master bit the device's dma reads
// never leave the pci stub and never reach the iommu
fn enable_device(sid: u16) {
    let cmd = cfg_read(sid, 0x04);
    cfg_write(sid, 0x04, cmd | 0x6);
}

// vendor+device of one function; 0xffff = nothing there
fn ids(sid: u16) -> Option<(u16, u16)> {
    let v = cfg_read(sid, 0);
    if v & 0xFFFF == 0xFFFF {
        return None;
    }
    Some((v as u16, (v >> 16) as u16))
}

// first device with vendor/device id, any bus/function 0..8
pub fn find_device(vendor: u16, device: u16) -> Option<u16> {
    for bus in 0u16..256 {
        for dev in 0u16..32 {
            for fun in 0u16..8 {
                let sid = (bus << 8) | (dev << 3) | fun;
                if ids(sid) == Some((vendor, device)) {
                    return Some(sid);
                }
            }
        }
    }
    None
}

// memory bar `index` of a pci function: (base, size), size probed the
// standard way (write ~0, read the mask back, restore). io-space and
// unimplemented bars come back as None
pub fn pci_mem_bar(sid: u16, index: usize) -> Option<(u64, u64)> {
    if index > 5 {
        return None;
    }
    let off = 0x10 + index as u8 * 4;
    let low = cfg_read(sid, off);
    if low == 0 || low == 0xFFFF_FFFF || low & 1 != 0 || low & 0x6 == 0x2 {
        return None;
    }
    let high = if low & 0x6 == 0x4 {
        cfg_read(sid, off + 4)
    } else {
        0
    };
    cfg_write(sid, off, 0xFFFF_FFFF);
    let mask = cfg_read(sid, off);
    cfg_write(sid, off, low);
    if mask == 0 || mask == 0xFFFF_FFFF {
        return None;
    }
    let size = (!(mask & !0xF)).wrapping_add(1);
    if size == 0 {
        return None;
    }
    Some((
        ((low & 0xFFFF_FFF0) as u64) | ((high as u64) << 32),
        size as u64,
    ))
}

// first mmio bar (32-bit) of a function
fn mmio_bar(sid: u16) -> Option<u64> {
    (0..6).find_map(|i| pci_mem_bar(sid, i).map(|(base, _)| base))
}

// e1000 (82540em) registers used here
const CTRL: usize = 0x0000;
const ICR: usize = 0x00C0; // interrupt cause read (write 1 = clear)
const IMS: usize = 0x00D0; // interrupt mask set
const IMC: usize = 0x00D8; // interrupt mask clear
const TDBAL: usize = 0x3800;
const TDBAH: usize = 0x3804;
const TDLEN: usize = 0x3808;
const TDH: usize = 0x3810;
const TDT: usize = 0x3818;
const TCTL: usize = 0x0400;

struct E1000 {
    regs: usize,
}

impl E1000 {
    // r is a byte offset from the spec, registers are 32-bit there
    fn wr(&self, r: usize, v: u32) {
        unsafe { core::ptr::write_volatile((self.regs + r) as *mut u32, v) }
    }
}

// the nic's intx line: discovered by the smoke (interrupt line register
// through the ich9 pirq), then the same host vector is what devd routes
// into the guest with guest_pi — one discovery, two consumers
static NIC_LINE: AtomicU8 = AtomicU8::new(0);
static NIC_REGS: AtomicUsize = AtomicUsize::new(0);
static NIC_WAITING: AtomicBool = AtomicBool::new(false);
static NIC_IRQ: AtomicBool = AtomicBool::new(false);

pub fn nic_vec() -> u64 {
    match NIC_LINE.load(Ordering::Relaxed) {
        0 => 0,
        l => 32 + l as u64,
    }
}

// read icr: on the e1000 a read clears every cause and deasserts inta —
// the mandatory first half of a level-triggered intx service
pub fn nic_quiet() {
    let regs = NIC_REGS.load(Ordering::Relaxed);
    if regs != 0 {
        unsafe {
            core::ptr::read_volatile((regs + ICR) as *mut u32);
        }
    }
}

// idt path for pic vectors 34..47: quiet the nic (deassert inta), then
// the caller eoIs; a waiter (the smoke) latches the vector as completion
pub fn nic_intx(vector: u8) {
    if vector != 32 + NIC_LINE.load(Ordering::Relaxed) {
        return;
    }
    nic_quiet();
    if NIC_WAITING.load(Ordering::Relaxed) {
        NIC_IRQ.store(true, Ordering::Relaxed);
    }
}

const NDESC: usize = 8;
const RING_GPA: u64 = 0x4000;
const BUF_GPA: u64 = 0x8000;

struct Desc {
    addr: u64,
    len: u16,
    cmd: u8,
}

fn put_desc(frame: PhysAddr, i: usize, d: &Desc) {
    unsafe {
        let p = crate::memory::phys_to_virt(frame).as_mut_ptr::<u8>();
        core::ptr::write_volatile(p.add(i * 16) as *mut u64, d.addr);
        core::ptr::write_volatile(p.add(i * 16 + 8) as *mut u16, d.len);
        core::ptr::write_volatile(p.add(i * 16 + 11), d.cmd);
        core::ptr::write_volatile(p.add(i * 16 + 12), 0u8);
    }
}

fn desc_done(frame: PhysAddr, i: usize) -> bool {
    unsafe {
        let p = crate::memory::phys_to_virt(frame).as_mut_ptr::<u8>();
        core::ptr::read_volatile(p.add(i * 16 + 12)) & 1 != 0
    }
}

fn spin(budget: usize, f: impl Fn() -> bool) -> bool {
    for _ in 0..budget {
        if f() {
            return true;
        }
        core::hint::spin_loop();
    }
    false
}

// the whole round: nic assigned to a kernel domain, dma through it works
// (dd), then the buffer unmaps and the unit records the fault
pub fn smoke() {
    let Some(sid) = find_device(0x8086, 0x100E) else {
        crate::serial_println!("[iommu] no e1000 on pci — dma smoke skipped");
        return;
    };
    if !crate::iommu::present() {
        crate::serial_println!("[iommu] smoke skipped: no unit armed");
        return;
    }
    enable_device(sid);
    let Some(bar) = mmio_bar(sid) else {
        crate::serial_println!("[iommu] e1000 has no mmio bar");
        return;
    };
    let regs = crate::memory::map_mmio(bar, 32).as_u64() as usize;
    NIC_REGS.store(regs, Ordering::Relaxed);
    let nic = E1000 { regs };
    // intx: the line register (0x3c) carries what firmware routed for
    // inta# (pin 0x3d); without a route we program the ich9 pirq and the
    // device ourselves — qemu's e1000 has no msi cap, intx is the only line
    let cfg34 = cfg_read(sid, 0x3C);
    let pin = ((cfg34 >> 8) & 0x07) as u8;
    let mut line = (cfg34 & 0xFF) as u8;
    if line > 15 {
        line = 10;
        let pirq = (pin.saturating_sub(1).wrapping_add((sid >> 3) as u8 & 0x1F)) & 3;
        cfg_write(0x00F8, 0x60 + pirq, line as u32);
        cfg_write(sid, 0x3C, line as u32);
    }
    NIC_LINE.store(line, Ordering::Relaxed);
    crate::interrupts::unmask_pic_line(line);
    crate::serial_println!(
        "[nic] intx: pin {} line {} -> vector {:#x} (unmasked)",
        pin,
        line,
        32 + line
    );
    let Some(ring) = crate::memory::alloc_frame() else {
        return;
    };
    let Some(buf) = crate::memory::alloc_frame() else {
        crate::memory::dealloc_frame(ring);
        return;
    };
    unsafe {
        zero(ring);
        zero(buf);
        let p = crate::memory::phys_to_virt(buf.start_address()).as_mut_ptr::<u8>();
        for (i, b) in b"paparaci dma through vt-d".iter().enumerate() {
            core::ptr::write_volatile(p.add(i), *b);
        }
    }
    let mut dom = match crate::iommu::Domain::new() {
        Some(d) => d,
        None => {
            crate::memory::dealloc_frame(ring);
            crate::memory::dealloc_frame(buf);
            crate::serial_println!("[iommu] smoke skipped: no domain");
            return;
        }
    };
    let (ok_ring, ok_buf) = (
        dom.map(RING_GPA, ring.start_address()),
        dom.map(BUF_GPA, buf.start_address()),
    );
    if !ok_ring || !ok_buf || !dom.attach(sid) {
        crate::serial_println!("[iommu] smoke: setup failed");
        drop(dom);
        crate::memory::dealloc_frame(ring);
        crate::memory::dealloc_frame(buf);
        return;
    }
    // tx ring lives at ring gpa, payload at buf gpa: both reachable to the
    // nic only through the domain
    nic.wr(CTRL, 0x40); // slu
    nic.wr(TDBAL, RING_GPA as u32);
    nic.wr(TDBAH, (RING_GPA >> 32) as u32);
    nic.wr(TDLEN, (NDESC * 16) as u32);
    nic.wr(TDH, 0);
    nic.wr(TDT, 0);
    nic.wr(TCTL, 0x0004_0F0A); // en | psp | ct | cold
    put_desc(
        ring.start_address(),
        0,
        &Desc {
            addr: BUF_GPA,
            len: 26,
            cmd: 0x0B,
        },
    );
    // completion via interrupt: txdw (icr bit 0) rides the intx line to
    // the pic, the idt stub latches it here — dd is only the evidence
    nic.wr(IMC, 0xFFFF_FFFF);
    nic.wr(ICR, 0xFFFF_FFFF);
    nic.wr(IMS, 1); // txdw
    NIC_WAITING.store(true, Ordering::Relaxed);
    nic.wr(TDT, 1);
    let irq = spin(3_000_000, || NIC_IRQ.load(Ordering::Relaxed));
    let mut done = irq && desc_done(ring.start_address(), 0);
    if !done {
        done = spin(2_000_000, || desc_done(ring.start_address(), 0));
    }
    NIC_WAITING.store(false, Ordering::Relaxed);
    nic.wr(IMC, 0xFFFF_FFFF);
    if done {
        if irq {
            crate::serial_println!(
                "[iommu] dma ok: nic read gpa {:#x} through domain {} (dd set, via irq vec {:#x})",
                BUF_GPA,
                dom.id,
                32 + line
            );
        } else {
            crate::serial_println!("[nic] intx missing — completion polled");
            crate::serial_println!(
                "[iommu] dma ok: nic read gpa {:#x} through domain {} (dd set, via poll)",
                BUF_GPA,
                dom.id
            );
        }
    } else {
        crate::serial_println!("[iommu] dma incomplete: no dd from the nic");
    }
    // buffer out of the domain: the same read must now fault
    dom.unmap(BUF_GPA);
    put_desc(
        ring.start_address(),
        1,
        &Desc {
            addr: BUF_GPA,
            len: 26,
            cmd: 0x0B,
        },
    );
    nic.wr(TDT, 2);
    let _ = spin(400_000, || desc_done(ring.start_address(), 1));
    let seen = crate::iommu::drain_faults();
    if !seen {
        crate::serial_println!("[iommu] no fault after unmap (device idle or unit silent)");
    }
    // leave the nic idle and the device back in the host's domain
    nic.wr(TCTL, 0);
    let _ = dom.detach(sid);
    drop(dom);
    crate::memory::dealloc_frame(ring);
    crate::memory::dealloc_frame(buf);
}

unsafe fn zero(frame: x86_64::structures::paging::PhysFrame) {
    core::ptr::write_bytes(
        crate::memory::phys_to_virt(frame.start_address()).as_mut_ptr::<u8>(),
        0,
        4096,
    );
}
