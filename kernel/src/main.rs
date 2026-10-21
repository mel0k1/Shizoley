#![no_std]
#![no_main]
#![feature(abi_x86_interrupt)]

extern crate alloc;

mod allocator;
mod elf;
mod gdt;
mod interrupts;
mod memory;
mod sched;
mod serial;
mod syscall;
mod task;
mod user;
mod vga;

use bootloader::{entry_point, BootInfo};

entry_point!(kernel_main);

fn kernel_main(boot_info: &'static BootInfo) -> ! {
    serial_println!("[paparaci] boot");
    gdt::init();
    interrupts::init();

    let mut mapper = memory::init(boot_info);
    allocator::init(&mut mapper).expect("heap init failed");
    serial_println!("[paparaci] heap up");

    println!("shizoley / paparaci v0.4.0 (tasks, address spaces, threads)");

    syscall::init();

    sched::init_current(task::Task::boot_current("main", sched::kernel_aspace()));

    // all spawns atomic vs the timer so the schedule is deterministic
    x86_64::instructions::interrupts::without_interrupts(|| {
        sched::spawn_kernel("alpha", || {
            for i in 0..5 {
                serial_println!("[alpha] tick {}", i);
                sched::yield_now();
            }
        });
        sched::spawn_kernel("beta", || {
            for i in 0..5 {
                serial_println!("[beta] tick {}", i);
                sched::yield_now();
            }
        });
        user::spawn_init();
        serial_println!("[paparaci] tasks ready, scheduler loop");
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
