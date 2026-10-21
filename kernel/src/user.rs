use alloc::sync::Arc;
use core::arch::asm;
use x86_64::structures::paging::{Page, PageTableFlags};
use x86_64::VirtAddr;

use crate::serial_println;
use crate::{elf, gdt, memory, sched, task};

pub const USER_STACK_BASE: u64 = 0x0050_0000;
pub const USER_STACK_TOP: u64 = 0x0060_0000;

// static init ELF, parsed and mapped by the elf loader
static USER_INIT: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/init.elf"));

// builds a fresh address space, loads init.elf into it and queues the task
pub fn spawn_init() -> u64 {
    let mut aspace = task::AddressSpace::new_user().expect("user address space");
    let entry = elf::load(USER_INIT, &mut aspace).expect("elf: init load failed");

    let flags =
        PageTableFlags::PRESENT | PageTableFlags::WRITABLE | PageTableFlags::USER_ACCESSIBLE;
    let sfirst = Page::containing_address(VirtAddr::new(USER_STACK_BASE));
    let slast = Page::containing_address(VirtAddr::new(USER_STACK_TOP - 1));
    for page in Page::range_inclusive(sfirst, slast) {
        let frame = memory::alloc_frame().expect("user stack: no frames");
        let fresh = aspace
            .map_user_page(page, frame, flags)
            .expect("user stack: map failed");
        if fresh {
            let dst = memory::phys_to_virt(frame.start_address()).as_mut_ptr::<u8>();
            unsafe { core::ptr::write_bytes(dst, 0, 4096) };
        }
    }

    let pid = sched::spawn_user(
        "init",
        Arc::new(aspace),
        entry,
        // 8 mod 16: _start behaves like a normal call frame
        VirtAddr::new(USER_STACK_TOP - 8),
    );
    serial_println!(
        "[paparaci] init.elf: {} bytes, entry {:#x}, task {}",
        USER_INIT.len(),
        entry,
        pid
    );
    pid
}

// iretq into ring 3; runs on the task's own kernel stack, user CR3 active
pub unsafe fn enter_ring3(entry: VirtAddr, stack_top: VirtAddr) -> ! {
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
