use core::arch::{asm, global_asm};
use core::ptr::{addr_of, addr_of_mut};
use x86_64::registers::control::{Cr4, Cr4Flags};

use crate::serial_println;
use crate::task::USER_LIMIT;

// managarm-style user access: syscall paths copy straight through the
// active user cr3 with smap stac/clac, and a #pf inside the fixed copy
// window rewinds rip to the recovery label instead of killing the kernel
// — a bad pointer costs the syscall an error, nothing else. remote-cr3
// and irq-context copies keep the physmap walk (task::read_bytes).

pub fn access_ok(addr: u64, len: u64) -> bool {
    addr <= USER_LIMIT && len <= USER_LIMIT - addr
}

// 0 = all bytes moved, nonzero = bytes left at the faulting page
// (both stubs copy rsi→rdi, so the kernel-side buffer always rides rsi)
pub fn copy_from_user(src: u64, dst: &mut [u8]) -> usize {
    if dst.is_empty() {
        return 0;
    }
    if !access_ok(src, dst.len() as u64) {
        return dst.len();
    }
    unsafe { paparaci_uar_from(dst.as_mut_ptr(), src, dst.len() as u64) as usize }
}

pub fn copy_to_user(dst: u64, src: &[u8]) -> usize {
    if src.is_empty() {
        return 0;
    }
    if !access_ok(dst, src.len() as u64) {
        return src.len();
    }
    unsafe { paparaci_uar_to(dst, src.as_ptr(), src.len() as u64) as usize }
}

pub fn read_u32(addr: u64) -> Option<u32> {
    let mut b = [0u8; 4];
    if copy_from_user(addr, &mut b) != 0 {
        return None;
    }
    Some(u32::from_le_bytes(b))
}

// the #pf handler asks: does this rip sit in a copy window, and where
// should it resume instead?
pub fn recovery_rip(rip: u64) -> Option<u64> {
    let f = paparaci_uar_from as unsafe extern "C" fn(*mut u8, u64, u64) -> u64 as usize as u64;
    let t = paparaci_uar_to as unsafe extern "C" fn(u64, *const u8, u64) -> u64 as usize as u64;
    let fe = addr_of!(paparaci_uar_from_end) as usize as u64;
    let te = addr_of!(paparaci_uar_to_end) as usize as u64;
    if (f..fe).contains(&rip) {
        return Some(addr_of!(paparaci_uar_from_fault) as usize as u64);
    }
    if (t..te).contains(&rip) {
        return Some(addr_of!(paparaci_uar_to_fault) as usize as u64);
    }
    None
}

// smap/smep go on before vmx::init so the vmcs host-cr4 field picks them
// up; the asm stubs gate stac/clac on the SMAP_OK bss flag (on a cpu
// without smap the instruction itself would #ud)
pub fn init() {
    let ebx = leaf7_ebx();
    let smap = ebx & (1 << 20) != 0;
    let smep = ebx & (1 << 7) != 0;
    let mut cr4 = Cr4::read();
    if smap {
        cr4.insert(Cr4Flags::SUPERVISOR_MODE_ACCESS_PREVENTION);
    }
    if smep {
        cr4.insert(Cr4Flags::SUPERVISOR_MODE_EXECUTION_PROTECTION);
    }
    unsafe { Cr4::write(cr4) };
    unsafe {
        addr_of_mut!(SMAP_OK).write_volatile(u64::from(smap));
    }
    serial_println!(
        "[uar] smap {}, smep {} — user copies guarded by the fault window",
        on_off(smap),
        on_off(smep)
    );
}

fn on_off(v: bool) -> &'static str {
    if v {
        "on"
    } else {
        "off"
    }
}

// leaf 7 subleaf 0: ebx bit 7 = smep, bit 20 = smap (stac/clac ride along)
fn leaf7_ebx() -> u32 {
    let b: u32;
    unsafe {
        asm!(
            "push rbx",
            "mov eax, 7",
            "xor ecx, ecx",
            "cpuid",
            "mov r11d, ebx",
            "pop rbx",
            out("r11") b,
            out("eax") _,
            out("ecx") _,
            out("edx") _,
            clobber_abi("C"),
        );
    }
    b
}

global_asm!(
    r#"
    .section .text.uar, "ax"
    .p2align 4
    .globl paparaci_uar_from
paparaci_uar_from:
    cmpq $0, SMAP_OK(%rip)
    je 1f
    stac
1:  cld
    movq %rdx, %rcx
    rep movsb
    xorl %eax, %eax
    jmp 3f
    .globl paparaci_uar_from_fault
paparaci_uar_from_fault:
    cmpq $0, SMAP_OK(%rip)
    je 2f
    clac
2:  movq %rcx, %rax
    ret
3:  cmpq $0, SMAP_OK(%rip)
    je 4f
    clac
4:  ret
    .globl paparaci_uar_from_end
paparaci_uar_from_end:

    .p2align 4
    .globl paparaci_uar_to
paparaci_uar_to:
    cmpq $0, SMAP_OK(%rip)
    je 1f
    stac
1:  cld
    movq %rdx, %rcx
    rep movsb
    xorl %eax, %eax
    jmp 3f
    .globl paparaci_uar_to_fault
paparaci_uar_to_fault:
    cmpq $0, SMAP_OK(%rip)
    je 2f
    clac
2:  movq %rcx, %rax
    ret
3:  cmpq $0, SMAP_OK(%rip)
    je 4f
    clac
4:  ret
    .globl paparaci_uar_to_end
paparaci_uar_to_end:

    .section .bss, "aw", @nobits
    .globl SMAP_OK
SMAP_OK:
    .quad 0
    "#,
    options(att_syntax)
);

unsafe extern "C" {
    fn paparaci_uar_from(dst: *mut u8, src: u64, len: u64) -> u64;
    fn paparaci_uar_to(dst: u64, src: *const u8, len: u64) -> u64;
    static paparaci_uar_from_end: u8;
    static paparaci_uar_from_fault: u8;
    static paparaci_uar_to_end: u8;
    static paparaci_uar_to_fault: u8;
    static mut SMAP_OK: u64;
}
