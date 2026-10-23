use alloc::sync::Arc;
use core::arch::asm;
use core::sync::atomic::{AtomicU64, Ordering};
use x86_64::structures::paging::{Page, PageTableFlags};
use x86_64::VirtAddr;

use crate::serial_println;
use crate::{elf, gdt, initrd, ipc, memory, sched, task};

pub const USER_STACK_BASE: u64 = 0x0050_0000;
pub const USER_STACK_TOP: u64 = 0x0060_0000;

// every process sees the initrd read-only here: services (fsd) walk it from
// ring 3; the frames are shared with the kernel image, no copies
const INITRD_PAGES: u64 = (initrd::INITRD_SIZE as u64).div_ceil(0x1000);

static NEXT_PID: AtomicU64 = AtomicU64::new(1);

fn map_zeroed_range(
    aspace: &mut task::AddressSpace,
    base: u64,
    top: u64,
    what: &str,
) -> Result<(), &'static str> {
    let flags =
        PageTableFlags::PRESENT | PageTableFlags::WRITABLE | PageTableFlags::USER_ACCESSIBLE;
    let first = Page::containing_address(VirtAddr::new(base));
    let last = Page::containing_address(VirtAddr::new(top - 1));
    for page in Page::range_inclusive(first, last) {
        let frame = memory::alloc_frame().ok_or("no frames")?;
        let fresh = match aspace.map_user_page(page, frame, flags) {
            Ok(f) => f,
            Err(_) => {
                memory::dealloc_frame(frame);
                return Err("map");
            }
        };
        if fresh {
            let dst = memory::phys_to_virt(frame.start_address()).as_mut_ptr::<u8>();
            unsafe { core::ptr::write_bytes(dst, 0, 4096) };
        } else {
            memory::dealloc_frame(frame);
        }
    }
    let _ = what;
    Ok(())
}

// share the kernel's initrd frames read-only into a fresh address space
fn map_initrd(aspace: &mut task::AddressSpace) -> Result<(), &'static str> {
    use x86_64::structures::paging::{Mapper, OffsetPageTable};

    // BIT_9 marks kernel-owned frames: teardown must not free them
    let flags = PageTableFlags::PRESENT
        | PageTableFlags::USER_ACCESSIBLE
        | PageTableFlags::NO_EXECUTE
        | PageTableFlags::BIT_9;
    // the blob lives in the kernel image: resolve its physical frames via
    // the boot page tables and map the same frames for the user
    let root = *memory::KERNEL_PML4.get().expect("kernel pml4");
    let kmapper = unsafe {
        OffsetPageTable::new(
            &mut *(memory::phys_to_virt(root.start_address()).as_mut_ptr()),
            memory::phys_offset(),
        )
    };
    let blob = initrd::initrd_vaddr();
    for i in 0..INITRD_PAGES {
        let kpage = Page::containing_address(VirtAddr::new(blob + i * 0x1000));
        let frame = kmapper
            .translate_page(kpage)
            .expect("initrd: kernel page unmapped");
        let upage = Page::containing_address(VirtAddr::new(initrd::INITRD_BASE + i * 0x1000));
        aspace
            .map_user_page(upage, frame, flags)
            .map_err(|_| "initrd map failed")?;
    }
    Ok(())
}

// loads `name` from the initrd into a fresh process and queues its main
// thread; each process gets its own CR3 and the standard user stack region.
// every failure path tears the half-built address space down so no frames
// leak into the void
pub fn spawn_process(name: &str) -> Option<u64> {
    let image = initrd::lookup(name)?;
    let mut aspace = task::AddressSpace::new_user().ok()?;
    let loaded = match elf::load(image, &mut aspace) {
        Ok(l) => l,
        Err(e) => {
            serial_println!("[spawn] elf '{}' rejected: {}", name, e);
            aspace.teardown();
            return None;
        }
    };
    // the image must not claim our own regions: stack window and initrd
    let stack_range = (USER_STACK_BASE, USER_STACK_TOP);
    let initrd_range = (
        initrd::INITRD_BASE,
        initrd::INITRD_BASE + initrd::INITRD_SIZE as u64,
    );
    for (s, e) in &loaded.ranges {
        for (rs, re) in [stack_range, initrd_range] {
            if *s < re && rs < *e {
                serial_println!("[spawn] elf '{}' overlaps reserved region", name);
                aspace.teardown();
                return None;
            }
        }
    }
    if map_zeroed_range(&mut aspace, USER_STACK_BASE, USER_STACK_TOP, "user stack").is_err() {
        aspace.teardown();
        return None;
    }
    if let Err(e) = map_initrd(&mut aspace) {
        serial_println!("[spawn] initrd '{}' rejected: {}", name, e);
        aspace.teardown();
        return None;
    }

    let pid = NEXT_PID.fetch_add(1, Ordering::AcqRel);
    let proc = Arc::new(task::Process::new(pid, name, aspace));
    ipc::register(&proc);
    let tid = sched::spawn_user(
        "main",
        proc,
        loaded.entry,
        // 8 mod 16: _start behaves like a normal call frame
        VirtAddr::new(USER_STACK_TOP - 8),
        0,
    );
    serial_println!(
        "[paparaci] spawned '{}' from initrd: {} bytes, entry {:#x}, pid {} tid {}",
        name,
        image.len(),
        loaded.entry,
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
        fl = in(reg) 0x3202u64,
        ip = in(reg) entry.as_u64(),
        in("rdi") arg,
        options(noreturn)
    );
}
