use alloc::sync::Arc;
use core::arch::asm;
use core::sync::atomic::{AtomicU64, Ordering};
use x86_64::structures::paging::{Page, PageTableFlags};
use x86_64::VirtAddr;

use crate::serial_println;
use crate::{elf, gdt, initrd, ipc, memory, sched, task};

pub const USER_STACK_BASE: u64 = 0x0050_0000;
pub const USER_STACK_TOP: u64 = 0x0060_0000;

static NEXT_PID: AtomicU64 = AtomicU64::new(1);

fn map_zeroed_range(aspace: &mut task::AddressSpace, base: u64, top: u64, what: &str) {
    let flags =
        PageTableFlags::PRESENT | PageTableFlags::WRITABLE | PageTableFlags::USER_ACCESSIBLE;
    let first = Page::containing_address(VirtAddr::new(base));
    let last = Page::containing_address(VirtAddr::new(top - 1));
    for page in Page::range_inclusive(first, last) {
        let frame = memory::alloc_frame().unwrap_or_else(|| panic!("{}: no frames", what));
        let fresh = aspace
            .map_user_page(page, frame, flags)
            .expect("map failed");
        if fresh {
            let dst = memory::phys_to_virt(frame.start_address()).as_mut_ptr::<u8>();
            unsafe { core::ptr::write_bytes(dst, 0, 4096) };
        }
    }
}

// loads `name` from the initrd into a fresh process and queues its main
// thread; each process gets its own CR3 and the standard user stack region
pub fn spawn_process(name: &str) -> Option<u64> {
    let image = initrd::lookup(name)?;
    let mut aspace = task::AddressSpace::new_user().ok()?;
    let entry = elf::load(image, &mut aspace).ok()?;
    map_zeroed_range(&mut aspace, USER_STACK_BASE, USER_STACK_TOP, "user stack");

    let pid = NEXT_PID.fetch_add(1, Ordering::AcqRel);
    let proc = Arc::new(task::Process::new(pid, name, aspace));
    ipc::register(&proc);
    let tid = sched::spawn_user(
        "main",
        proc,
        entry,
        // 8 mod 16: _start behaves like a normal call frame
        VirtAddr::new(USER_STACK_TOP - 8),
        0,
    );
    serial_println!(
        "[paparaci] spawned '{}' from initrd: {} bytes, entry {:#x}, pid {} tid {}",
        name,
        image.len(),
        entry,
        pid,
        tid
    );
    Some(pid)
}

// iretq into ring 3; runs on the task's own kernel stack, user CR3 active;
// the thread entry convention is: first argument (arg) arrives in rdi
pub unsafe fn enter_ring3(entry: VirtAddr, stack_top: VirtAddr, arg: u64) -> ! {
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
        in("rdi") arg,
        options(noreturn)
    );
}
