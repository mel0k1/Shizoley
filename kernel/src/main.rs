#![no_std]
#![no_main]
#![feature(abi_x86_interrupt)]

extern crate alloc;

mod allocator;
mod elf;
mod gdt;
mod interrupts;
mod memory;
mod serial;
mod syscall;
mod user;
mod vga;

use alloc::vec::Vec;
use bootloader::{entry_point, BootInfo};
use x86_64::VirtAddr;

entry_point!(kernel_main);

fn kernel_main(boot_info: &'static BootInfo) -> ! {
    serial_println!("[paparaci] boot");
    gdt::init();
    interrupts::init();

    let (mut mapper, mut frames) = memory::init(boot_info);
    allocator::init(&mut mapper, &mut frames).expect("heap init failed");
    serial_println!("[paparaci] heap up");

    let mut squares = Vec::new();
    for i in 0..16u64 {
        squares.push(i * i);
    }
    println!(
        "shizoley / paparaci v0.3.0 (heap: sum 0..16 = {})",
        squares.iter().sum::<u64>()
    );

    syscall::init();

    let phys_offset = VirtAddr::new(boot_info.physical_memory_offset);
    let (entry, stack) = user::load(&mut mapper, &mut frames, phys_offset);
    serial_println!("[paparaci] entering ring 3");
    unsafe { user::enter(entry, stack) };
}

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    serial_println!("[paparaci] panic: {}", info);
    println!("[paparaci] panic: {}", info);
    loop {
        x86_64::instructions::hlt();
    }
}
