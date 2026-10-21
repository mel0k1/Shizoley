#![no_std]
#![no_main]
#![feature(abi_x86_interrupt)]

extern crate alloc;

mod allocator;
mod gdt;
mod interrupts;
mod memory;
mod serial;
mod vga;

use alloc::vec::Vec;
use bootloader::{entry_point, BootInfo};

entry_point!(kernel_main);

fn kernel_main(boot_info: &'static BootInfo) -> ! {
    serial_println!("[paparaci] boot");
    gdt::init();
    interrupts::init();

    let (mut mapper, mut frames) = memory::init(boot_info);
    allocator::init(&mut mapper, &mut frames).expect("heap init failed");
    serial_println!("[paparaci] heap up");

    println!("shizoley / paparaci v0.1.0");

    let mut squares = Vec::new();
    for i in 0..16u64 {
        squares.push(i * i);
    }
    println!(
        "heap check: sum of squares 0..16 = {}",
        squares.iter().sum::<u64>()
    );

    serial_println!("[paparaci] ok");
    loop {
        x86_64::instructions::hlt();
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
