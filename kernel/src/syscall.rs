use core::arch::global_asm;
use core::ptr::addr_of_mut;
use x86_64::registers::model_specific::Msr;
use x86_64::VirtAddr;

use crate::gdt;
use crate::ipc;
use crate::serial;
use crate::vga;
use crate::{println, sched, serial_println, task};

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
        movq 16(%rsp), %r8
        sti
        call {dispatch}
        cli
        popq %r8
        popq %r9
        popq %r10
        popq %rdx
        popq %rsi
        popq %rdi
        addq $8, %rsp
        popq %rbp
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

const ERR: u64 = (-1i64) as u64;

extern "C" fn syscall_dispatch(nr: u64, a1: u64, a2: u64, a3: u64, a4: u64) -> u64 {
    let _ = a4;
    let cur = match sched::current() {
        Some(c) => c,
        None => {
            serial_println!("[paparaci] syscall without task");
            return ERR;
        }
    };
    match nr {
        // 0: putc(char)
        0 => {
            vga::put_byte(a1 as u8);
            serial::put_byte(a1 as u8);
            0
        }
        // 1: exit(code) — ends the thread; process dies with its last thread
        1 => {
            let code = a1 as i64;
            if cur.proc.id != 0 {
                serial_println!(
                    "[sched] thread {} of '{}' ({}) exit({})",
                    cur.id,
                    cur.proc.name,
                    cur.proc.id,
                    code
                );
            }
            sched::exit_current(code)
        }
        // 2: spawn(name, name_len) -> pid; elf comes from the initrd
        2 => {
            let len = a2.min(32) as usize;
            let mut namebuf = [0u8; 32];
            if len == 0 || crate::uar::copy_from_user(a1, &mut namebuf[..len]) != 0 {
                return ERR;
            }
            match core::str::from_utf8(&namebuf[..len]) {
                Ok(name) => crate::user::spawn_process(name).unwrap_or(ERR),
                Err(_) => ERR,
            }
        }
        // 3: thread_create(entry, stack_top, arg) -> tid; same address space
        3 => {
            let valid = |x: u64| (0x1000..task::USER_LIMIT).contains(&x);
            if !valid(a1) || !valid(a2) {
                return ERR;
            }
            let proc = cur.proc.clone();
            sched::spawn_user("thread", proc, VirtAddr::new(a1), VirtAddr::new(a2), a3)
        }
        // 4: ipc_send(dest_pid, buf, len) -> 0 | -1
        4 => ipc::send(&cur, a1, a2, a3) as u64,
        // 5: ipc_recv(src_pid /*0 = any*/, buf, max) -> sender pid | -1
        5 => sched::recv_or_block(a1, a2, a3),
        // 6: getpid() -> pid
        6 => cur.proc.id,
        // 7: gettid() -> tid
        7 => cur.id,
        // 8: eq_setup(base, size) -> root capability handle; the qid stays
        //    kernel-internal, callers only ever see unguessable tokens
        8 => crate::eq::setup(a1, a2) as u64,
        // 9: eq_post(qid, opcode, buf, len) -> 0 | -1
        9 => crate::eq::post(a1, a2 as u32, a3, a4) as u64,
        // 10: eq_wait(qid) -> 1 | -1; parks until the doorbell
        10 => crate::eq::wait(a1) as u64,
        // 11: eq_kick(qid) -> 0 | -1; release chunk credit to posters
        11 => crate::eq::kick(a1) as u64,
        // 12: initrd_size() -> bytes (mapped read-only at INITRD_BASE)
        12 => crate::initrd::INITRD_SIZE as u64,
        // 13: write(buf, len) -> len; atomic console line (vga+serial)
        13 => {
            let len = a2.min(4096) as usize;
            let mut bytes = alloc::vec![0u8; len];
            if crate::uar::copy_from_user(a1, &mut bytes) != 0 {
                ERR
            } else {
                x86_64::instructions::interrupts::without_interrupts(|| {
                    for b in &bytes {
                        vga::put_byte(*b);
                        serial::put_byte(*b);
                    }
                });
                len as u64
            }
        }
        // 14: kill(pid) -> 0 | -1; ends every thread of the process
        14 => sched::kill_process(a1) as u64,
        // 15: guest_create(pages) -> root capability handle | -1; vmcs +
        //     ept + ram window behind an unguessable token
        15 => crate::vmx::guest_create(a1) as u64,
        // 16: vcpu_run(handle, out16x u64) -> exit reason | -1 (GRUN)
        16 => crate::vmx::vcpu_run(a1, a2) as u64,
        // 17: guest_destroy(handle) -> 0 | -1; frees ram, ept, vmcs
        //     (GDESTROY; every capability on the guest dies with it)
        17 => crate::vmx::guest_destroy(a1) as u64,
        // 18: eq_listen(qid, which) -> 0 | -1; register a kernel event sink
        //     (LISTEN right): which 0 = keyboard, 1 = timer fires
        18 => crate::eq::listen(a1, a2) as u64,
        // 19: cap_grant(handle, pid, rights) -> child handle | -1; derive
        //     a capability for pid on ANY object kind (would_cycle-checked
        //     delegation; the granter must hold the kind's grant right)
        19 => {
            let target = a2;
            let bits = a3 as u32;
            match crate::caps::grant(a1, cur.proc.id, target, bits) {
                Ok(child) => {
                    let obj = crate::caps::obj_of(a1, cur.proc.id);
                    let what = match obj.map(|o| o.kind).unwrap_or(0) {
                        crate::caps::K_EQ => "queue",
                        crate::caps::K_GUEST => "guest",
                        _ => "gift",
                    };
                    serial_println!(
                        "[caps] granted {:#x} on {} {} to pid {} (handle {:#x})",
                        bits,
                        what,
                        obj.map(|o| o.id).unwrap_or(0),
                        target,
                        child
                    );
                    child as i64 as u64
                }
                Err(crate::caps::GrantErr::Cycle) => {
                    serial_println!(
                        "[caps] would_cycle refused: grant to pid {} (chain reaches the holder)",
                        target
                    );
                    ERR
                }
                Err(crate::caps::GrantErr::NoRight) => {
                    serial_println!(
                        "[caps] grant denied: pid {} lacks the grant right or the bits",
                        cur.proc.id
                    );
                    ERR
                }
                Err(crate::caps::GrantErr::Bad) => ERR,
            }
        }
        // 20: cap_revoke(handle) -> n revoked | -1; cascade the subtree on
        //     any object kind
        20 => match crate::caps::revoke(a1, cur.proc.id) {
            Ok(n) => {
                serial_println!(
                    "[caps] revoked handle {:#x}: {} derived handles died",
                    a1,
                    n
                );
                n as u64
            }
            Err(_) => ERR,
        },
        // 21: guest_map(handle, count) -> new size | -1; grow guest ram +
        //     ept (GMAP)
        21 => crate::vmx::guest_map(a1, a2) as u64,
        // 22: eq_alert(qid) -> 0 | -1; cancel parked eq_wait (SIGNAL right)
        22 => crate::eq::alert(a1) as u64,
        // 23: eq_zpost(qid, buf, len) -> handle | -1; zero-copy page gift
        23 => crate::eq::zpost(a1, a2, a3) as u64,
        // 24: eq_zmap(handle) -> va | -1; map the gift into the owner
        24 => crate::eq::zmap(a1) as u64,
        // 25: eq_zdone(handle) -> 0 | -1; consume the gift, free the frames
        25 => crate::eq::zdone(a1) as u64,
        // 26: guest_mode(handle, mode) -> 0 | -1; mode 1 = long-mode guest
        //     (GMODE)
        26 => crate::vmx::guest_mode(a1, a2) as u64,
        // 27: guest_pi(handle, vector) -> 0 | -1; route a host irq vector
        //     into the guest's posted-interrupt descriptor (0 = clear all;
        //     GPI)
        27 => crate::vmx::guest_pi(a1, a2) as u64,
        // 28: ps(buf, max_entries) -> n; 40-byte records (pid, threads,
        //     state 0=run/1=dead, name[16])
        28 => crate::ipc::ps(a1, a2) as u64,
        // 29: ticks() -> scheduler tick count (the PIT clock)
        29 => crate::interrupts::tick_count(),
        // 30: timer_arm(deadline_ticks) -> 0 | -1; one-shot deadline for
        //     the timer sink (only its owner may arm; 0 disarms)
        30 => crate::eq::arm(a1) as u64,
        // 31: futex_wait(uaddr, expected) -> 0 = slept, 1 = value mismatch
        //     (no sleep), -1 = bad address
        31 => crate::futex::wait(a1, a2 as u32) as u64,
        // 32: futex_wake(uaddr, n) -> threads woken
        32 => crate::futex::wake(a1, a2) as u64,
        // 33: guest_device_attach(handle, sid) -> 0 | -1; put a pci device
        //     (bus<<8 | dev<<3 | fn) in the guest's iommu domain (GDEV)
        33 => crate::vmx::guest_device_attach(a1, a2) as u64,
        // 34: guest_device_detach(handle, sid) -> 0 | -1; the device's dma
        //     faults until it is attached elsewhere (GDEV)
        34 => crate::vmx::guest_device_detach(a1, a2) as u64,
        // 35: guest_mmio(handle, sid, bar) -> bar size | -1; identity uc map
        //     of a pci memory bar into the guest ept + iommu mirror (GMAP)
        35 => crate::vmx::guest_mmio_map(a1, a2, a3) as u64,
        _ => {
            println!("[paparaci] bad syscall {}", nr);
            serial_println!("[paparaci] bad syscall {}", nr);
            sched::exit_current(-1)
        }
    }
}
