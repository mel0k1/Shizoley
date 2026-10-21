use core::arch::global_asm;
use core::ptr::addr_of_mut;
use x86_64::registers::model_specific::Msr;

use crate::gdt;
use crate::serial;
use crate::vga;
use crate::{println, sched, serial_println};

// rcx = user rip, r11 = user rflags, rax = nr, rdi/rsi/rdx = args
// the scheduler points SYSCALL_KSTACK_TOP at the current task's kernel stack,
// so every task gets its own syscall/irq depth instead of one shared stack
global_asm!(
    r#"
    .section .text.syscall_entry, "ax"
    .p2align 4
    paparaci_syscall_entry:
        movq %rsp, SAVED_USER_RSP(%rip)
        movq SYSCALL_KSTACK_TOP(%rip), %rsp
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
        movq SAVED_USER_RSP(%rip), %rsp
        sysretq

    .section .bss, "aw", @nobits
    .p2align 4
    SAVED_USER_RSP:
        .quad 0
    SYSCALL_KSTACK_TOP:
        .quad 0
    "#,
    dispatch = sym syscall_dispatch,
    options(att_syntax)
);

unsafe extern "C" {
    fn paparaci_syscall_entry();
    static mut SAVED_USER_RSP: u64;
    static mut SYSCALL_KSTACK_TOP: u64;
}

// called by the scheduler with interrupts off
pub fn enter_task(kstack_top: u64, user_rsp: u64) {
    unsafe {
        addr_of_mut!(SYSCALL_KSTACK_TOP).write_volatile(kstack_top);
        addr_of_mut!(SAVED_USER_RSP).write_volatile(user_rsp);
    }
}

pub fn saved_user_rsp() -> u64 {
    unsafe { addr_of_mut!(SAVED_USER_RSP).read_volatile() }
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
            sched::exit_current(code)
        }
        _ => {
            println!("[paparaci] bad syscall {}", nr);
            serial_println!("[paparaci] bad syscall {}", nr);
            sched::exit_current(-1)
        }
    }
}
