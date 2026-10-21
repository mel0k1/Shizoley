use core::arch::asm;
use x86_64::structures::paging::{FrameAllocator, Mapper, Page, PageTableFlags, Size4KiB};
use x86_64::VirtAddr;

use crate::{elf, gdt, memory};

pub const USER_STACK_BASE: u64 = 0x0050_0000;
pub const USER_STACK_TOP: u64 = 0x0060_0000;

// static init ELF, parsed and mapped by the elf loader
static USER_INIT: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/init.elf"));

pub fn load(
    mapper: &mut impl Mapper<Size4KiB>,
    frames: &mut impl FrameAllocator<Size4KiB>,
    phys_offset: VirtAddr,
) -> (VirtAddr, VirtAddr) {
    let entry = elf::load(USER_INIT, mapper, frames, phys_offset).expect("elf: init load failed");

    let flags =
        PageTableFlags::PRESENT | PageTableFlags::WRITABLE | PageTableFlags::USER_ACCESSIBLE;
    let sfirst = Page::containing_address(VirtAddr::new(USER_STACK_BASE));
    let slast = Page::containing_address(VirtAddr::new(USER_STACK_TOP - 1));
    for page in Page::range_inclusive(sfirst, slast) {
        let frame = frames.allocate_frame().expect("user stack: no frames");
        unsafe {
            mapper.map_to(page, frame, flags, frames).unwrap().flush();
            let dst = (phys_offset + frame.start_address().as_u64()).as_mut_ptr::<u8>();
            core::ptr::write_bytes(dst, 0, 4096);
        }
    }
    unsafe { memory::mark_user_range(phys_offset, USER_STACK_BASE, USER_STACK_TOP) };

    (
        entry,
        // 8 mod 16: _start behaves like a normal call frame
        VirtAddr::new(USER_STACK_TOP - 8),
    )
}

pub unsafe fn enter(entry: VirtAddr, stack_top: VirtAddr) -> ! {
    let code = (gdt::USER_CODE | 3) as u64;
    let data = (gdt::USER_DATA | 3) as u64;
    asm!(
        "mov ds, {d:x}",
        "mov es, {d:x}",
        "mov fs, {d:x}",
        "mov gs, {d:x}",
        "push {d}",
        "push {sp}",
        "push {fl}",
        "push {c}",
        "push {ip}",
        "iretq",
        d = in(reg) data,
        c = in(reg) code,
        sp = in(reg) stack_top.as_u64(),
        fl = in(reg) 0x202u64,
        ip = in(reg) entry.as_u64(),
        options(noreturn)
    );
}
