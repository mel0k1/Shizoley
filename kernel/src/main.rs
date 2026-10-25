#![no_std]
#![no_main]
#![feature(abi_x86_interrupt)]

extern crate alloc;

use alloc::vec::Vec;

mod acpi;
mod allocator;
mod caps;
mod dma;
mod elf;
mod eq;
mod futex;
mod gdt;
mod initrd;
mod interrupts;
mod iommu;
mod ipc;
mod memobj;
mod memory;
mod sched;
mod serial;
mod syscall;
mod task;
mod uar;
mod user;
mod vga;
mod vmx;

use bootloader::{entry_point, BootInfo};

entry_point!(kernel_main);

fn kernel_main(boot_info: &'static BootInfo) -> ! {
    serial_println!("[paparaci] boot");
    gdt::init();
    interrupts::init();

    let mut mapper = memory::init(boot_info);
    allocator::init(&mut mapper).expect("heap init failed");
    uar::init();
    memory::init_pat();
    serial_println!(
        "[paparaci] heap up, {} frames in use",
        memory::frames_used()
    );

    // usable ram ranges for the iommu's identity fallback
    let mut ram: Vec<(u64, u64)> = Vec::new();
    for r in boot_info.memory_map.iter() {
        if r.region_type == bootloader::bootinfo::MemoryRegionType::Usable {
            ram.push((
                r.range.start_frame_number * 0x1000,
                r.range.end_frame_number * 0x1000,
            ));
        }
    }
    iommu::init(&ram);

    println!(
        "shizoley v{} (kernel paparaci: caps, eq, vmx)",
        env!("CARGO_PKG_VERSION")
    );
    serial_println!("shizoley v{}", env!("CARGO_PKG_VERSION"));

    vmx::init();
    syscall::init();

    sched::init_current(task::Task::boot_current("main", sched::kernel_process()));

    // all spawns atomic vs the timer so the schedule is deterministic
    x86_64::instructions::interrupts::without_interrupts(|| {
        // kernel thread: proves preemption keeps running while init blocks
        sched::spawn_kernel("heartbeat", || {
            for i in 0..3 {
                for _ in 0..80 {
                    sched::yield_now();
                }
                serial_println!("[heartbeat] {}", i);
            }
        });
        // kernel thread: the device-assignment round — nic dma through a
        // domain: mapped frames complete, unmapped ones fault
        sched::spawn_kernel("dma", dma::smoke);
        match user::spawn_process("init") {
            Some(pid) => {
                serial_println!("[paparaci] init queued (pid {}), scheduler loop", pid);
            }
            None => {
                serial_println!("[paparaci] initrd: 'init' not found or broken");
                println!("[paparaci] initrd: 'init' not found or broken");
            }
        }
    });

    loop {
        sched::reap();
        x86_64::instructions::interrupts::enable_and_hlt();
    }
}

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    serial_println!("[paparaci] panic: {}", info);
    println!("[paparaci] panic: {}", info);
    loop {
        x86_64::instructions::hlt();
    }
}
