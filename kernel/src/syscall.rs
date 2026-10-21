use core::arch::global_asm;
use x86_64::registers::model_specific::Msr;

use crate::gdt;
use crate::serial;
use crate::vga;
use crate::{println, serial_println};

// rcx = user rip, r11 = user rflags, rax = nr, rdi/rsi/rdx = args
global_asm!(
    r#"
    .section .text.syscall_entry, "ax"
    .p2align 4
    paparaci_syscall_entry:
        movabs $syscall_kstack_top, %rsp
        pushq %r11
        pushq %rcx
        pushq %rbp
        pushq %rax
        pushq %rdi
        pushq %rsi
        pushq %rdx
        pushq %r10
        pushq %r9
        pushq %r8
        movq %rax, %rdi
        movq 40(%rsp), %rsi
        movq 32(%rsp), %rdx
        movq 24(%rsp), %rcx
        sti
        call {dispatch}
        cli
        popq %r8
        popq %r9
        popq %r10
        popq %rdx
        popq %rsi
        popq %rdi
        popq %rbp
        addq $8, %rsp
        popq %rcx
        popq %r11
        sysretq

    .section .bss, "aw", @nobits
    .p2align 4
    syscall_kstack:
        .space 65536
    syscall_kstack_top:
    "#,
    dispatch = sym syscall_dispatch,
    options(att_syntax)
);

unsafe extern "C" {
    fn paparaci_syscall_entry();
}

const EFER: u32 = 0xC000_0080;
const STAR: u32 = 0xC000_0081;
const LSTAR: u32 = 0xC000_0082;
const SFMASK: u32 = 0xC000_0084;

pub fn init() {
    unsafe {
        let sce = Msr::new(EFER).read() | 1;
        Msr::new(EFER).write(sce);
        // syscall: cs=kernel_code, ss=kernel_data; sysret: cs=+16 user_code, ss=+8 user_data
        Msr::new(STAR).write(((gdt::KERNEL_DATA as u64) << 48) | ((gdt::KERNEL_CODE as u64) << 32));
        Msr::new(LSTAR).write(paparaci_syscall_entry as unsafe extern "C" fn() as usize as u64);
        // clear tf/if/df/iopl/nt on entry
        Msr::new(SFMASK).write(0x7700);
    }
    serial_println!("[paparaci] syscall msr up");
}

extern "C" fn syscall_dispatch(nr: u64, a1: u64, _a2: u64, _a3: u64) -> u64 {
    match nr {
        0 => {
            let b = a1 as u8;
            vga::put_byte(b);
            serial::put_byte(b);
            0
        }
        1 => {
            let code = a1 as i64;
            println!("[init] exit({})", code);
            serial_println!("[init] exit({})", code);
            user_halt()
        }
        _ => {
            println!("[paparaci] bad syscall {}", nr);
            serial_println!("[paparaci] bad syscall {}", nr);
            user_halt()
        }
    }
}

fn user_halt() -> ! {
    loop {
        x86_64::instructions::hlt();
    }
}
