use core::arch::asm;
use x86_64::structures::paging::page_table::PageTableEntry;
use x86_64::structures::paging::{
    FrameAllocator, Mapper, Page, PageTable, PageTableFlags, Size4KiB,
};
use x86_64::VirtAddr;

use crate::gdt;
use crate::memory;

pub const USER_CODE_BASE: u64 = 0x0040_0000;
pub const USER_STACK_BASE: u64 = 0x0050_0000;
pub const USER_STACK_TOP: u64 = 0x0060_0000;

// raw image linked at USER_CODE_BASE, _start is the first byte
static USER_INIT: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/user_init.bin"));

pub fn load(
    mapper: &mut impl Mapper<Size4KiB>,
    frames: &mut impl FrameAllocator<Size4KiB>,
    phys_offset: VirtAddr,
) -> (VirtAddr, VirtAddr) {
    let flags =
        PageTableFlags::PRESENT | PageTableFlags::WRITABLE | PageTableFlags::USER_ACCESSIBLE;

    let pages = (USER_INIT.len() as u64).div_ceil(4096);
    let first = Page::containing_address(VirtAddr::new(USER_CODE_BASE));
    let last = Page::containing_address(VirtAddr::new(USER_CODE_BASE + pages * 4096 - 1));
    for (i, page) in Page::range_inclusive(first, last).enumerate() {
        let frame = frames.allocate_frame().expect("user code: no frames");
        unsafe {
            mapper.map_to(page, frame, flags, frames).unwrap().flush();
            let dst = (phys_offset + frame.start_address().as_u64()).as_mut_ptr::<u8>();
            core::ptr::write_bytes(dst, 0, 4096);
            let lo = i * 4096;
            let hi = core::cmp::min(lo + 4096, USER_INIT.len());
            core::ptr::copy_nonoverlapping(USER_INIT.as_ptr().add(lo), dst, hi - lo);
        }
    }

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

    // bootloader-owned ancestors of the user region need the user bit
    unsafe { user_ancestors(phys_offset) };

    (
        VirtAddr::new(USER_CODE_BASE),
        // 8 mod 16: _start behaves like a normal call frame
        VirtAddr::new(USER_STACK_TOP - 8),
    )
}

unsafe fn user_ancestors(phys_offset: VirtAddr) {
    fn allow_user(e: &mut PageTableEntry) {
        e.set_flags(e.flags() | PageTableFlags::USER_ACCESSIBLE);
    }
    let pml4 = memory::active_pml4(phys_offset);
    allow_user(&mut pml4[0]);
    let pdpt = &mut *((phys_offset + pml4[0].addr().as_u64()).as_mut_ptr::<PageTable>());
    allow_user(&mut pdpt[0]);
    let pd = &mut *((phys_offset + pdpt[0].addr().as_u64()).as_mut_ptr::<PageTable>());
    // 0x400000..0x5FFFFF covers user code and stack
    allow_user(&mut pd[2]);
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
