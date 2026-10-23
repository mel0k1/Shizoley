#![no_std]
#![no_main]
#![feature(abi_x86_interrupt)]

extern crate alloc;

mod allocator;
mod caps;
mod elf;
mod eq;
mod gdt;
mod initrd;
mod interrupts;
mod ipc;
mod memory;
mod sched;
mod serial;
mod syscall;
mod task;
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
    serial_println!(
        "[paparaci] heap up, {} frames in use",
        memory::frames_used()
    );

    println!(
        "shizoley v{} (kernel paparaci: caps, eq, vmx)",
        env!("CARGO_PKG_VERSION")
    );

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
