use alloc::boxed::Box;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::arch::{asm, global_asm};
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use spin::{Mutex, Once};
use x86_64::registers::control::{Cr0, Cr3, Cr3Flags, Cr4};
use x86_64::registers::model_specific::Msr;
use x86_64::structures::paging::{Page, PageTableFlags, PhysFrame, Size4KiB};
use x86_64::VirtAddr;

use crate::serial_println;
use crate::task::Process;
use crate::{caps, gdt, memory, println};

// the Veda-style heart: VMCS + vcpu + EPT behind GUEST_CREATE/VCPU_RUN
// syscalls. one vcpu per guest, unrestricted real-mode entry, EPT shadows
// the guest ram which is also mapped into the creator's address space so
// ring 3 can load images and read results (e.g. the guest's "VGA" text).

const IA32_FEATURE_CONTROL: u32 = 0x3A;
const IA32_VMX_BASIC: u32 = 0x480;
// raw control-capability msrs (always present); the TRUE_* variants are
// only valid when IA32_VMX_BASIC[55] is set (see TRUE_CTLS)
const IA32_VMX_PIN: u32 = 0x481;
const IA32_VMX_PROC: u32 = 0x482;
const IA32_VMX_EXIT: u32 = 0x483;
const IA32_VMX_ENTRY: u32 = 0x484;
const IA32_VMX_CR0_FIXED0: u32 = 0x486;
const IA32_VMX_CR0_FIXED1: u32 = 0x487;
const IA32_VMX_CR4_FIXED0: u32 = 0x488;
const IA32_VMX_CR4_FIXED1: u32 = 0x489;
const IA32_VMX_TRUE_PIN: u32 = 0x48D;
const IA32_VMX_TRUE_PROC: u32 = 0x48E;
const IA32_VMX_TRUE_EXIT: u32 = 0x48F;
const IA32_VMX_TRUE_ENTRY: u32 = 0x490;
const IA32_VMX_PROC2: u32 = 0x48B; // secondary proc-based ctl caps (no true variant)
const IA32_VMX_EPT_VPID_CAP: u32 = 0x48C;
const IA32_VMX_MISC: u32 = 0x485;

// ept_vpid_cap bits: 32 = invvpid exists (vpid usable), 26 = all-context invept
const EPTVPID_VPID: u64 = 1 << 32;
const EPTVPID_INVEPT_ALL: u64 = 1 << 26;
const EPTVPID_EPT2M: u64 = 1 << 16;

// gpr slots in the Vcpu layout (see OFF_G_* below; rsp lives in the vmcs)
const IDX_RAX: usize = 0;
const IDX_RCX: usize = 2;
const IDX_RDX: usize = 3;

// vmcs field encodings (sdm vol 3d tables 25-x)
const GUEST_ES_SEL: u64 = 0x0800;
const GUEST_CS_SEL: u64 = 0x0802;
const GUEST_SS_SEL: u64 = 0x0804;
const GUEST_DS_SEL: u64 = 0x0806;
const GUEST_FS_SEL: u64 = 0x0808;
const GUEST_GS_SEL: u64 = 0x080A;
const GUEST_LDTR_SEL: u64 = 0x080C;
const GUEST_TR_SEL: u64 = 0x080E;
const GUEST_CR0: u64 = 0x6800;
const GUEST_CR3: u64 = 0x6802;
const GUEST_CR4: u64 = 0x6804;
const HOST_ES_SEL: u64 = 0x0C00;
const HOST_CS_SEL: u64 = 0x0C02;
const HOST_SS_SEL: u64 = 0x0C04;
const HOST_DS_SEL: u64 = 0x0C06;
const HOST_FS_SEL: u64 = 0x0C08;
const HOST_GS_SEL: u64 = 0x0C0A;
const HOST_TR_SEL: u64 = 0x0C0E;
const GUEST_ES_LIMIT: u64 = 0x4800;
const GUEST_CS_LIMIT: u64 = 0x4802;
const GUEST_SS_LIMIT: u64 = 0x4804;
const GUEST_DS_LIMIT: u64 = 0x4806;
const GUEST_FS_LIMIT: u64 = 0x4808;
const GUEST_GS_LIMIT: u64 = 0x480A;
const GUEST_LDTR_LIMIT: u64 = 0x480C;
const GUEST_TR_LIMIT: u64 = 0x480E;
const GUEST_GDTR_LIMIT: u64 = 0x4810;
const GUEST_IDTR_LIMIT: u64 = 0x4812;
const GUEST_ES_AR: u64 = 0x4814;
const GUEST_CS_AR: u64 = 0x4816;
const GUEST_SS_AR: u64 = 0x4818;
const GUEST_DS_AR: u64 = 0x481A;
const GUEST_FS_AR: u64 = 0x481C;
const GUEST_GS_AR: u64 = 0x481E;
const GUEST_LDTR_AR: u64 = 0x4820;
const GUEST_TR_AR: u64 = 0x4822;
const GUEST_INTERRUPTIBILITY: u64 = 0x4824;
const GUEST_ACTIVITY_STATE: u64 = 0x4826;
// gpa 0x4828 would be guest smbase — kvm's vmcs12 has no slot for it ("SMM
// is not supported") and we never deliver SMIs, so it is never written
const GUEST_SYSENTER_CS: u64 = 0x482A;
const VM_INSTRUCTION_ERROR: u64 = 0x4400;
const VM_EXIT_REASON: u64 = 0x4402;
// 0x4404 is vm-exit interruption information (its error code is 0x4406);
// exit qualification lives in the natural-width read-only area (0x6400)
const VM_EXIT_INTR_INFO: u64 = 0x4404;
const VM_EXIT_QUALIFICATION: u64 = 0x6400;
const VM_ENTRY_INTR_INFO: u64 = 0x4016;
const VM_ENTRY_EXCEPTION_ERR_CODE: u64 = 0x4018;
const VM_ENTRY_INSTRUCTION_LEN: u64 = 0x401A;
const VMX_PIN_CTLS: u64 = 0x4000;
const VMX_PROC_CTLS: u64 = 0x4002;
const VMX_EXCEPTION_BITMAP: u64 = 0x4004;
const VMX_EXIT_CTLS: u64 = 0x400C;
const VMX_ENTRY_CTLS: u64 = 0x4012;
const VMX_MSR_BITMAPS: u64 = 0x2004;
const VMX_VPID: u64 = 0x0000;
const VMX_PREEMPT_TIMER: u64 = 0x482E;
const VMX_VIRTUAL_APIC: u64 = 0x2012;
const VMX_APIC_ACCESS: u64 = 0x2014;
const VMX_POSTED_INTR_DESC: u64 = 0x2016; // posted-interrupt descriptor addr
const VMX_POSTED_INTR_NV: u64 = 0x0002; // notification vector (16-bit)
const GUEST_INTR_STATUS: u64 = 0x482; // RVI 7:0 / SVI 15:8 (needs vid)
                                      // p4: pin-ctl bit 7 = process posted interrupts; secondary bit 9 =
                                      // virtual-interrupt delivery. vm-entry refuses the pair without each
                                      // other (and without apicv behind them), so both are probed together
const PIN_POSTED: u64 = 1 << 7;
const PROC2_VID: u64 = 1 << 9;
// the notification vector the cpu consumes in non-root instead of
// exiting; dodges the pic range, exceptions and guest vectors
const PI_VECTOR: u64 = 0xF0;
// vAPIC register offsets (sdm 29.1): isr 0x100, irr 0x200, 32 bytes each
const VAPIC_ISR: u64 = 0x100;
const VAPIC_IRR: u64 = 0x200;
// sdm table b-9 (32-bit control fields): 0x401e is the tpr threshold,
// 0x4022 is secondary proc ctls — the pair 0x401c/0x401e would put proc2
// into the tpr-threshold slot (reserved bits set -> vm-entry fails) and
// leave secondary controls disabled entirely (no ept/urg/vid on real hw)
const VMX_TPR_THRESHOLD: u64 = 0x401E;
const VM_EXIT_INSTRUCTION_LEN: u64 = 0x440C;
// cr4 mask/shadow are natural-width control fields (0x6002/0x6006); the
// 0x40xx slot aliases are 32-bit control fields — writing there clobbers
// live execution controls
const VMX_CR4_GUEST_HOST_MASK: u64 = 0x6002;
const VMX_CR4_READ_SHADOW: u64 = 0x6006;
const VMX_EPTP: u64 = 0x201A;
const VMX_PROC_CTLS2: u64 = 0x4022;
const GUEST_PHYSICAL_ADDR: u64 = 0x2400;
const VMCS_LINK_PTR: u64 = 0x2800;
const GUEST_DEBUGCTL: u64 = 0x2802;
const GUEST_EFER: u64 = 0x2806;
const HOST_EFER: u64 = 0x2C02;
const GUEST_ES_BASE: u64 = 0x6806;
const GUEST_CS_BASE: u64 = 0x6808;
const GUEST_SS_BASE: u64 = 0x680A;
const GUEST_DS_BASE: u64 = 0x680C;
const GUEST_FS_BASE: u64 = 0x680E;
const GUEST_GS_BASE: u64 = 0x6810;
const GUEST_LDTR_BASE: u64 = 0x6812;
const GUEST_TR_BASE: u64 = 0x6814;
const GUEST_GDTR_BASE: u64 = 0x6816;
const GUEST_IDTR_BASE: u64 = 0x6818;
const GUEST_DR7: u64 = 0x681A;
const GUEST_RSP: u64 = 0x681C;
const GUEST_RIP: u64 = 0x681E;
const GUEST_RFLAGS: u64 = 0x6820;
const GUEST_PENDING_DBG: u64 = 0x6822;
const GUEST_SYSENTER_ESP: u64 = 0x6824;
const GUEST_SYSENTER_EIP: u64 = 0x6826;
const HOST_CR0: u64 = 0x6C00;
const HOST_CR3: u64 = 0x6C02;
const HOST_CR4: u64 = 0x6C04;
const HOST_FS_BASE: u64 = 0x6C06;
const HOST_GS_BASE: u64 = 0x6C08;
const HOST_TR_BASE: u64 = 0x6C0A;
const HOST_SYSENTER_ESP: u64 = 0x6C10;
const HOST_SYSENTER_EIP: u64 = 0x6C12;
const HOST_RSP: u64 = 0x6C14;
const HOST_RIP: u64 = 0x6C16;

// exit reasons we act on
const R_EXTERNAL_INT: u64 = 1;
const R_VMCALL: u64 = 18;
const R_RDMSR: u64 = 31;
const R_WRMSR: u64 = 32;
const R_IO: u64 = 30;
const R_APIC_ACCESS: u64 = 44;
const R_EPT_VIOLATION: u64 = 49;
const R_EPT_MISCONFIG: u64 = 50;
const R_PREEMPT_TIMER: u64 = 52;

// guest-visible apic page (the vAPIC shadow the cpu serves mmio from)
const APIC_GPA: u64 = 0xFEE0_0000;
const APIC_BASE_MSR: u32 = 0x1B;
const GUEST_EFER_MSR: u32 = 0xC000_0080;

// msr bitmap: 4 pages — read low / read high / write low / write high;
// bit set = vm exit, bit clear = pass straight through to the real msr
const MSR_BM_RD_LOW: usize = 0x000;

// ---------------------------------------------------------------- vcpu

// asm-visible layout; offsets are load-bearing for the stub
#[repr(C)]
pub struct Vcpu {
    _vmcs: u64,              // +0
    _launched: u64,          // +8
    pub gprs: [u64; 16],     // +16 .. +144
    pub out: [u64; 20],      // +144 .. +304: reason rip qual gpa + 16 gprs
    _exit_stack_top: u64,    // +304
    _orig_rsp: u64,          // +312
    pub pi_desc: u64,        // +320: posted-interrupt descriptor phys (0 = off)
    pub vapic: u64,          // +328: virtual-apic page phys
    pub pi_routed: [u64; 4], // +336: host vectors routed into the guest
}
const _: () = assert!(core::mem::size_of::<Vcpu>() == 368);

#[repr(C, align(16))]
struct ExitStack([u8; 4096]);

struct Guest {
    id: u64,
    owner: Arc<Process>, // creator: the ram window lives in its space
    ram: Vec<PhysFrame<Size4KiB>>,
    tables: Vec<PhysFrame<Size4KiB>>, // ept pml4+pdpt+pd+pts + apic pd/pt/access/vapic (+pi desc)
    vmcs: PhysFrame<Size4KiB>,
    exit_stack: Box<ExitStack>,
    vcpu: Vcpu,
    launched: bool,
    mode: u8, // 0 = unrestricted real mode, 1 = long mode (guest_mode)
    pi_desc: Option<PhysFrame<Size4KiB>>, // posted-interrupt descriptor (apicv)
    pi_routed: [u64; 4], // host vectors routed into the guest
    domain: Option<crate::iommu::Domain>, // device-assignment: mirrors the ept
}

static GUESTS: Mutex<Vec<Guest>> = Mutex::new(Vec::new());
static NEXT_GUEST: AtomicU64 = AtomicU64::new(1);

// p2 machinery, probed once at init:
// msr bitmap phys (tsc reads pass through, everything else exits)
static MSR_BM: Once<u64> = Once::new();
static VPID_OK: AtomicBool = AtomicBool::new(false);
static INVEPT_OK: AtomicBool = AtomicBool::new(false);
static EPT2M_OK: AtomicBool = AtomicBool::new(false);
// p3 preemption timer: armed = pin-ctl bit 6 allowed, rate = vmx_misc[4:0]
// (the timer decrements once per 2^rate tsc ticks)
static PREEMPT_ARMED: AtomicBool = AtomicBool::new(false);
static PREEMPT_RATE: AtomicU64 = AtomicU64::new(0);
static SLICE_TICKS: Once<u64> = Once::new();
// p4 posted interrupts: armed = pin bit 7 (process posted interrupts) +
// secondary bit 9 (virtual-interrupt delivery) + apic-access virtualization
// all allowed-1; without apicv the cpu refuses the pair at vm-entry
static PI_ARMED: AtomicBool = AtomicBool::new(false);

// asm-visible state (single core: one guest runs at a time)
#[no_mangle]
static VMX_ACTIVE: AtomicU64 = AtomicU64::new(0);
#[no_mangle]
static VMX_G_R10: AtomicU64 = AtomicU64::new(0);
#[no_mangle]
static VMX_G_R11: AtomicU64 = AtomicU64::new(0);

const OFF_G_RAX: u64 = 16;
const OFF_G_RBX: u64 = 24;
const OFF_G_RCX: u64 = 32;
const OFF_G_RDX: u64 = 40;
const OFF_G_RSI: u64 = 48;
const OFF_G_RDI: u64 = 56;
const OFF_G_RBP: u64 = 64;
const OFF_G_R8: u64 = 72;
const OFF_G_R9: u64 = 80;
const OFF_G_R10: u64 = 88;
const OFF_G_R11: u64 = 96;
const OFF_G_R12: u64 = 104;
const OFF_G_R13: u64 = 112;
const OFF_G_R14: u64 = 120;
const OFF_G_R15: u64 = 128;
const OFF_ORIG_RSP: u64 = 312;

global_asm!(
    r#"
    .section .text.vmx_launch_asm, "ax"
    .p2align 4
    .globl vmx_launch_asm
vmx_launch_asm:
    # rdi = *mut Vcpu, esi = first
    push rbp
    push rbx
    push r12
    push r13
    push r14
    push r15
    mov qword ptr [rip + VMX_ACTIVE], rdi
    mov rbx, rdi
    mov [rbx + {orig}], rsp
    mov rax, [rbx + {g_rax}]
    mov rcx, [rbx + {g_rcx}]
    mov rdx, [rbx + {g_rdx}]
    mov rsi, [rbx + {g_rsi}]
    mov r8,  [rbx + {g_r8}]
    mov r9,  [rbx + {g_r9}]
    mov r10, [rbx + {g_r10}]
    mov r11, [rbx + {g_r11}]
    mov r12, [rbx + {g_r12}]
    mov r13, [rbx + {g_r13}]
    mov r14, [rbx + {g_r14}]
    mov r15, [rbx + {g_r15}]
    mov rbp, [rbx + {g_rbp}]
    mov rdi, [rbx + {g_rdi}]
    mov rbx, [rbx + {g_rbx}]
    test esi, esi
    jnz 1f
    vmlaunch
    mov eax, 2          # vmfail: never entered the guest
    jmp 3f
1:
    vmresume
    mov eax, 3          # vmfail on resume
3:
    pop r15
    pop r14
    pop r13
    pop r12
    pop rbx
    pop rbp
    ret

    .section .text.vmx_exit_stub, "ax"
    .p2align 4
    .globl vmx_exit_stub
vmx_exit_stub:
    # all guest gprs are live in the registers; r10/r11 are our scratch, so
    # they go to side slots before we load anything else
    mov qword ptr [rip + VMX_G_R10], r10
    mov qword ptr [rip + VMX_G_R11], r11
    mov r10, qword ptr [rip + VMX_ACTIVE]
    mov [r10 + {g_rax}], rax
    mov [r10 + {g_rbx}], rbx
    mov [r10 + {g_rcx}], rcx
    mov [r10 + {g_rdx}], rdx
    mov [r10 + {g_rsi}], rsi
    mov [r10 + {g_rdi}], rdi
    mov [r10 + {g_rbp}], rbp
    mov [r10 + {g_r8}], r8
    mov [r10 + {g_r9}], r9
    mov rax, qword ptr [rip + VMX_G_R10]
    mov [r10 + {g_r10}], rax
    mov rax, qword ptr [rip + VMX_G_R11]
    mov [r10 + {g_r11}], rax
    mov [r10 + {g_r12}], r12
    mov [r10 + {g_r13}], r13
    mov [r10 + {g_r14}], r14
    mov [r10 + {g_r15}], r15
    # back onto the launch frame and into the rust handler
    mov rsp, [r10 + {orig}]
    mov rdi, r10
    call vmx_after_exit
    test rax, rax
    jnz 2f
    # resume the guest
    mov r10, qword ptr [rip + VMX_ACTIVE]
    mov rax, [r10 + {g_rax}]
    mov rcx, [r10 + {g_rcx}]
    mov rdx, [r10 + {g_rdx}]
    mov rsi, [r10 + {g_rsi}]
    mov r8,  [r10 + {g_r8}]
    mov r9,  [r10 + {g_r9}]
    mov r11, [r10 + {g_r11}]
    mov r12, [r10 + {g_r12}]
    mov r13, [r10 + {g_r13}]
    mov r14, [r10 + {g_r14}]
    mov r15, [r10 + {g_r15}]
    mov rbp, [r10 + {g_rbp}]
    mov rbx, [r10 + {g_rbx}]
    mov rdi, [r10 + {g_rdi}]
    mov r10, [r10 + {g_r10}]
    vmresume
    ud2
2:
    # stop: unwind the launch frame and return to rust
    xor eax, eax
    pop r15
    pop r14
    pop r13
    pop r12
    pop rbx
    pop rbp
    ret
    "#,
    g_rax = const OFF_G_RAX,
    g_rbx = const OFF_G_RBX,
    g_rcx = const OFF_G_RCX,
    g_rdx = const OFF_G_RDX,
    g_rsi = const OFF_G_RSI,
    g_rdi = const OFF_G_RDI,
    g_rbp = const OFF_G_RBP,
    g_r8 = const OFF_G_R8,
    g_r9 = const OFF_G_R9,
    g_r10 = const OFF_G_R10,
    g_r11 = const OFF_G_R11,
    g_r12 = const OFF_G_R12,
    g_r13 = const OFF_G_R13,
    g_r14 = const OFF_G_R14,
    g_r15 = const OFF_G_R15,
    orig = const OFF_ORIG_RSP,
);

unsafe extern "C" {
    fn vmx_launch_asm(vcpu: *mut Vcpu, first: u32) -> u64;
    fn vmx_exit_stub();
}

// returns 0 = resume handled by stub, 1 = stop and return to caller
#[no_mangle]
extern "C" fn vmx_after_exit(vcpu: *mut Vcpu) -> u64 {
    let vcpu = unsafe { &mut *vcpu };
    let reason = vmread(VM_EXIT_REASON);
    match reason {
        R_EXTERNAL_INT => {
            // ack'd by the cpu (exit ctl bit 15): eoi and keep counters
            let info = vmread(VM_EXIT_INTR_INFO);
            let vector = (info & 0xFF) as u8;
            crate::interrupts::dispatch_in_root(vector);
            // p4: a routed vector is posted into the guest's descriptor
            // instead of being left to ring 3; re-entry runs the
            // PIR->vIRR sync and virtual-interrupt delivery injects it
            // while the guest runs (out[3] carries the posted vector)
            let v = vector as usize;
            if vcpu.pi_desc != 0 && vcpu.pi_routed[v / 64] >> (v % 64) & 1 != 0 {
                let vapic = memory::phys_to_virt(x86_64::PhysAddr::new(vcpu.vapic));
                unsafe {
                    // kernel-managed virtual EOI, same as the entry sync:
                    // the real-mode guest cannot clear its own vISR bit
                    let isr = vapic
                        .as_mut_ptr::<u32>()
                        .add((VAPIC_ISR as usize) / 4 + v / 32);
                    isr.write_volatile(isr.read_volatile() & !(1 << (v % 32)));
                }
                let desc = memory::phys_to_virt(x86_64::PhysAddr::new(vcpu.pi_desc));
                unsafe {
                    let pir = desc.as_mut_ptr::<u32>().add(v / 32);
                    pir.write_volatile(pir.read_volatile() | 1 << (v % 32));
                }
                vcpu.out[0] = reason;
                vcpu.out[1] = vmread(GUEST_RIP);
                vcpu.out[2] = 0;
                vcpu.out[3] = vector as u64;
                vcpu.out[4..20].copy_from_slice(&vcpu.gprs);
                return 1; // reportable: the monitor re-enters
            }
            // fairness: a timer tick ends this guest slice so the scheduler
            // decides who owns the cpu next; ext-int never retires an
            // instruction, so the monitor just re-enters (vmresume)
            if vector == crate::interrupts::TIMER_VECTOR {
                vcpu.out[0] = reason;
                vcpu.out[1] = vmread(GUEST_RIP);
                vcpu.out[2] = 0;
                vcpu.out[3] = 0;
                vcpu.out[4..20].copy_from_slice(&vcpu.gprs);
                return 1;
            }
            0
        }
        R_RDMSR => {
            let msr = (vcpu.gprs[IDX_RCX] & 0xFFFF_FFFF) as u32;
            match emulate_rdmsr(msr) {
                Some(val) => {
                    vcpu.gprs[IDX_RAX] = val & 0xFFFF_FFFF;
                    vcpu.gprs[IDX_RDX] = val >> 32;
                    advance_rip();
                }
                None => inject_gp(),
            }
            0
        }
        R_WRMSR => {
            let msr = (vcpu.gprs[IDX_RCX] & 0xFFFF_FFFF) as u32;
            let val = (vcpu.gprs[IDX_RDX] << 32) | (vcpu.gprs[IDX_RAX] & 0xFFFF_FFFF);
            if emulate_wrmsr(msr, val) {
                advance_rip();
            } else {
                inject_gp();
            }
            0
        }
        R_VMCALL => {
            // hypercall: consumed, not re-executed — rip moves past it so a
            // re-enter continues the guest instead of looping on vmcall
            advance_rip();
            let qual = 0;
            vcpu.out[0] = reason;
            vcpu.out[1] = vmread(GUEST_RIP);
            vcpu.out[2] = qual;
            vcpu.out[3] = 0;
            vcpu.out[4..20].copy_from_slice(&vcpu.gprs);
            1
        }
        R_PREEMPT_TIMER => {
            // p3: the slice ended — never resume in root, the scheduler
            // decides who owns the cpu next; reportable so ring 3 re-enters
            vcpu.out[0] = reason;
            vcpu.out[1] = vmread(GUEST_RIP);
            vcpu.out[2] = 0;
            vcpu.out[3] = 0;
            vcpu.out[4..20].copy_from_slice(&vcpu.gprs);
            1
        }
        _ => {
            let qual = vmread(VM_EXIT_QUALIFICATION);
            vcpu.out[0] = reason;
            vcpu.out[1] = vmread(GUEST_RIP);
            vcpu.out[2] = qual;
            vcpu.out[3] = if reason == R_EPT_VIOLATION || reason == R_EPT_MISCONFIG {
                vmread(GUEST_PHYSICAL_ADDR)
            } else if reason == R_IO {
                // io exit: the guest tried a real port; it stays blocked —
                // qualification bits 31:16 carry the port number
                (qual >> 16) & 0xFFFF
            } else if reason == R_APIC_ACCESS {
                // apic-access exit: the offset inside the apic page
                qual
            } else {
                0
            };
            vcpu.out[4..20].copy_from_slice(&vcpu.gprs);
            1
        }
    }
}

// ring-3 visible msr reads; everything else is the guest's problem (#gp)
fn emulate_rdmsr(msr: u32) -> Option<u64> {
    match msr {
        APIC_BASE_MSR => Some(APIC_GPA | 0x800), // enabled, at the standard page
        GUEST_EFER_MSR => Some(vmread(GUEST_EFER)),
        _ => None,
    }
}

// efer is the one msr a guest may set; only the benign bits pass, the
// rest is masked off so lma/lme games cannot break vm-entry state
fn emulate_wrmsr(msr: u32, val: u64) -> bool {
    if msr != GUEST_EFER_MSR {
        return false;
    }
    vmwrite(GUEST_EFER, val & 0x801); // sce | nxe
    true
}

fn advance_rip() {
    let len = vmread(VM_EXIT_INSTRUCTION_LEN);
    let rip = vmread(GUEST_RIP);
    vmwrite(GUEST_RIP, rip + len);
}

fn inject_gp() {
    // hardware exception #gp, error code valid, value 0; the faulting
    // instruction length travels along so the guest handler can skip it
    // instead of iret'ing right back into the same rdmsr/wrmsr
    let len = vmread(VM_EXIT_INSTRUCTION_LEN);
    vmwrite(VM_ENTRY_INTR_INFO, 0x8000_0B0D);
    vmwrite(VM_ENTRY_EXCEPTION_ERR_CODE, 0);
    vmwrite(VM_ENTRY_INSTRUCTION_LEN, len);
}

// ---------------------------------------------------------------- vmcs io

fn vmread(field: u64) -> u64 {
    let mut val: u64;
    let mut cf: u8;
    let mut zf: u8;
    unsafe {
        asm!(
            "vmread {v}, {f}",
            "setc {c}",
            "sete {z}",
            f = in(reg) field,
            v = out(reg) val,
            c = out(reg_byte) cf,
            z = out(reg_byte) zf,
            options(nostack)
        );
    }
    if cf | zf != 0 {
        0
    } else {
        val
    }
}

fn vmwrite(field: u64, value: u64) -> bool {
    let mut cf: u8;
    let mut zf: u8;
    unsafe {
        asm!(
            "vmwrite {f}, {v}",
            "setc {c}",
            "sete {z}",
            f = in(reg) field,
            v = in(reg) value,
            c = out(reg_byte) cf,
            z = out(reg_byte) zf,
            options(nostack)
        );
    }
    cf | zf == 0
}

fn vmptrld(phys: u64) -> bool {
    let mut slot = phys;
    let mut cf: u8;
    let mut zf: u8;
    unsafe {
        asm!(
            "vmptrld [{p}]",
            "setc {c}",
            "sete {z}",
            p = in(reg) &mut slot as *mut u64,
            c = out(reg_byte) cf,
            z = out(reg_byte) zf,
            options(nostack)
        );
    }
    cf | zf == 0
}

fn vmclear(phys: u64) -> bool {
    let mut slot = phys;
    let mut cf: u8;
    unsafe {
        asm!(
            "vmclear [{p}]",
            "setc {c}",
            p = in(reg) &mut slot as *mut u64,
            c = out(reg_byte) cf,
            options(nostack)
        );
    }
    cf == 0
}

// adjust a control field against its TRUE_xxx capability msr:
// value = allowed1 & (desired | !allowed0)
fn adjust_ctl(msr: u32, desired: u64) -> u64 {
    let v = unsafe { Msr::new(msr).read() };
    let allowed0 = v as u32 as u64;
    let allowed1 = v >> 32;
    allowed1 & (desired | !allowed0)
}

// ---------------------------------------------------------------- init

// IA32_VMX_BASIC[55]: the TRUE_* control msrs exist and allow changing
// defaults; on older cpus only the raw capability msrs are there
static TRUE_CTLS: AtomicBool = AtomicBool::new(false);
static VMX_READY: Once<()> = Once::new();

pub fn vmx_ready() -> bool {
    VMX_READY.get().is_some()
}

pub fn init() {
    if !cpu_has_vmx() {
        serial_println!("[vmx] probe: not supported by cpu");
        println!("[vmx] probe: not supported by cpu");
        return;
    }
    match enter_root() {
        Ok(rev) => {
            let (ept, ept_cap, urg) = probe_ept();
            if ept && ept_cap && urg {
                // msr bitmap: start all-ones (every access exits), then let
                // exactly one benign read through — the tsc. writes never
                // pass: a guest must not reach the host's real msrs
                let mut msr_armed = false;
                if let Some(f) = memory::alloc_frame() {
                    let v = memory::phys_to_virt(f.start_address());
                    unsafe {
                        core::ptr::write_bytes(v.as_mut_ptr::<u8>(), 0xFF, 0x1000);
                        // rdmsr(0x10): page 0 (read low), byte 2, bit 0
                        let p = v.as_mut_ptr::<u8>();
                        *p.add(MSR_BM_RD_LOW + 2) &= !1;
                    }
                    MSR_BM.call_once(|| f.start_address().as_u64());
                    msr_armed = true;
                }
                let caps = unsafe { Msr::new(IA32_VMX_EPT_VPID_CAP).read() };
                VPID_OK.store(caps & EPTVPID_VPID != 0, Ordering::Relaxed);
                INVEPT_OK.store(caps & EPTVPID_INVEPT_ALL != 0, Ordering::Relaxed);
                EPT2M_OK.store(caps & EPTVPID_EPT2M != 0, Ordering::Relaxed);
                // p3: preemption timer — pin-ctl bit 6 must be allowed-1;
                // the tick rate lives in vmx_misc[4:0]
                let pin_msr = if TRUE_CTLS.load(Ordering::Relaxed) {
                    IA32_VMX_TRUE_PIN
                } else {
                    IA32_VMX_PIN
                };
                let pin_allowed1 = unsafe { Msr::new(pin_msr).read() } >> 32;
                if pin_allowed1 & (1 << 6) != 0 {
                    let rate = unsafe { Msr::new(IA32_VMX_MISC).read() } & 0x1F;
                    PREEMPT_RATE.store(rate, Ordering::Relaxed);
                    PREEMPT_ARMED.store(true, Ordering::Relaxed);
                }
                // p4: posted interrupts — the pair pin bit 7 + vid bit 9 plus
                // the apic machinery must all be allowed-1
                let proc2_allowed1 = unsafe { Msr::new(0x48B).read() } >> 32;
                PI_ARMED.store(
                    pin_allowed1 & PIN_POSTED != 0
                        && proc2_allowed1 & PROC2_VID != 0
                        && proc2_allowed1 & (1 << 5) != 0,
                    Ordering::Relaxed,
                );
                VMX_READY.call_once(|| {});
                serial_println!(
                    "[vmx] on: rev {:#x} ept {} urg {} msrbm {} vpid {} ept2m {} preempt {}(rate {}) pi {} — guest_create/vcpu_run armed",
                    rev,
                    ept,
                    urg,
                    msr_armed,
                    VPID_OK.load(Ordering::Relaxed),
                    EPT2M_OK.load(Ordering::Relaxed),
                    PREEMPT_ARMED.load(Ordering::Relaxed),
                    PREEMPT_RATE.load(Ordering::Relaxed),
                    PI_ARMED.load(Ordering::Relaxed)
                );
                println!(
                    "[vmx] on: rev {:#x} ept {} urg {} msrbm {} vpid {} ept2m {} preempt {} pi {} — vm armed",
                    rev,
                    ept,
                    urg,
                    msr_armed,
                    VPID_OK.load(Ordering::Relaxed),
                    EPT2M_OK.load(Ordering::Relaxed),
                    PREEMPT_ARMED.load(Ordering::Relaxed),
                    PI_ARMED.load(Ordering::Relaxed)
                );
            } else {
                serial_println!(
                    "[vmx] on: rev {:#x} but ept {} ept_cap {} urg {} — guests disabled",
                    rev,
                    ept,
                    ept_cap,
                    urg
                );
                println!("[vmx] on but no ept/unrestricted-guest — guests disabled");
            }
        }
        Err(e) => {
            serial_println!("[vmx] enter failed: {}", e);
            println!("[vmx] enter failed: {}", e);
        }
    }
}

// p3: tsc ticks per scheduler timer tick, measured over two ticks on the
// first guest (the timer is certainly running by then); a quiet timer
// falls back to 32 m tsc ticks so the slice stays bounded
#[inline]
fn rdtsc() -> u64 {
    let (lo, hi): (u32, u32);
    unsafe {
        asm!("rdtsc", out("eax") lo, out("edx") hi, options(nostack, nomem));
    }
    ((hi as u64) << 32) | lo as u64
}

fn calibrate_slice_ticks() -> u64 {
    let t0 = crate::interrupts::tick_count();
    let c0 = rdtsc();
    let mut spins = 0u64;
    while crate::interrupts::tick_count() < t0 + 2 {
        spins += 1;
        if spins > 50_000_000 {
            return 1 << 25;
        }
        core::hint::spin_loop();
    }
    let per_tick = (rdtsc() - c0) / 2;
    per_tick.clamp(1 << 20, 1 << 30)
}

// preemption timer ticks for a ~one-scheduler-tick guest slice; the value
// field is 32-bit and counts down once per 2^rate tsc ticks
fn preempt_slice(rate: u64) -> u64 {
    let per_tick = SLICE_TICKS.call_once(calibrate_slice_ticks);
    (*per_tick >> rate).clamp(1, u32::MAX as u64)
}

// p4: transfer posted interrupts from the descriptor into the vAPIC.
// called on every entry with the guest's vmcs current; the descriptor is
// kernel-owned memory, plain volatile access suffices (single cpu). PIR
// bits clear, vIRR bits set (plus a kernel-managed virtual EOI that
// clears stale vISR bits — our real-mode guests never write the vAPIC
// EOI register), and RVI carries the highest vector so virtual-interrupt
// delivery injects it while the guest runs.
fn sync_posted(vcpu: &mut Vcpu) {
    if vcpu.pi_desc == 0 {
        return;
    }
    let desc = memory::phys_to_virt(x86_64::PhysAddr::new(vcpu.pi_desc));
    let vapic = memory::phys_to_virt(x86_64::PhysAddr::new(vcpu.vapic));
    let mut rvi = 0u64;
    unsafe {
        let pir = desc.as_mut_ptr::<u32>();
        for w in 0..4usize {
            let mut bits = pir.add(w).read_volatile();
            while bits != 0 {
                let bit = bits.trailing_zeros();
                bits &= bits - 1;
                pir.add(w)
                    .write_volatile(pir.add(w).read_volatile() & !(1 << bit));
                let v = w * 32 + bit as usize;
                let irr = vapic
                    .as_mut_ptr::<u32>()
                    .add((VAPIC_IRR as usize) / 4 + v / 32);
                irr.write_volatile(irr.read_volatile() | 1 << (v % 32));
                let isr = vapic
                    .as_mut_ptr::<u32>()
                    .add((VAPIC_ISR as usize) / 4 + v / 32);
                isr.write_volatile(isr.read_volatile() & !(1 << (v % 32)));
                rvi = v as u64; // ascending scan: ends on the highest
            }
        }
    }
    if rvi != 0 {
        // guest interrupt status: RVI bits 7:0, SVI (15:8) untouched
        let cur = vmread(GUEST_INTR_STATUS) & 0xFF00;
        let _ = vmwrite(GUEST_INTR_STATUS, cur | rvi);
    }
}

fn probe_ept() -> (bool, bool, bool) {
    unsafe {
        let proc = Msr::new(0x482).read();
        let sec_allowed = (proc >> 32) & (1 << 31) != 0;
        if !sec_allowed {
            return (false, false, false);
        }
        let c2 = Msr::new(0x48B).read();
        let ept = (c2 >> 32) & (1 << 1) != 0;
        let urg = (c2 >> 32) & (1 << 7) != 0;
        let cap = ept && Msr::new(0x48C).read() != 0;
        (ept, cap, urg)
    }
}

fn cpu_features_ecx() -> u32 {
    let mut c: u32;
    unsafe {
        asm!(
            "push rbx",
            "mov eax, 1",
            "cpuid",
            "pop rbx",
            lateout("ecx") c,
            clobber_abi("C"),
        );
    }
    c
}

fn cpu_has_vmx() -> bool {
    cpu_features_ecx() & (1 << 5) != 0
}

fn enter_root() -> Result<u32, &'static str> {
    unsafe {
        // a bios that locked feature control without vmx would make vmxon
        // die with a bare #gp; detect it and tell the story instead
        let fc = Msr::new(IA32_FEATURE_CONTROL).read();
        if fc & 1 != 0 && fc & 0b110 == 0 {
            return Err("feature control locked without vmx (bios)");
        }
        // nothing locked the msr yet: act as our own firmware and lock it
        // with vmx enabled. kvm boots an l1 with feature control = 0 and
        // its vmxon demands lock|vmx-on (0x5) — bare metal behaves the
        // same when firmware left the msr unlocked
        if fc & 1 == 0 {
            Msr::new(IA32_FEATURE_CONTROL).write(0b101);
        }
        // cr0/cr4 must satisfy the vmx fixed msrs before vmxon (kvm checks
        // them right at vmxon — cr0.ne is the bit firmware usually forgets;
        // real cpus check at vm-entry): force must-be-1, clear must-be-0
        let c0f0 = Msr::new(IA32_VMX_CR0_FIXED0).read();
        let c0f1 = Msr::new(IA32_VMX_CR0_FIXED1).read();
        Cr0::write_raw((Cr0::read_raw() | c0f0) & c0f1);
        let cr4 = Cr4::read_raw();
        if cr4 & (1 << 13) != 0 {
            return Err("vmxe already set");
        }
        let c4f0 = Msr::new(IA32_VMX_CR4_FIXED0).read();
        let c4f1 = Msr::new(IA32_VMX_CR4_FIXED1).read();
        Cr4::write_raw((cr4 | c4f0 | (1 << 13)) & c4f1);
    }

    let basic = unsafe { Msr::new(IA32_VMX_BASIC).read() };
    TRUE_CTLS.store(basic & (1 << 55) != 0, Ordering::Relaxed);
    let rev = basic as u32;

    let frame = memory::alloc_frame().ok_or("no frame for vmxon region")?;
    let region_virt = memory::phys_to_virt(frame.start_address());
    unsafe {
        core::ptr::write_bytes(region_virt.as_mut_ptr::<u8>(), 0, 0x1000);
        region_virt.as_mut_ptr::<u32>().write_volatile(rev);
    }
    let region_phys = frame.start_address().as_u64();

    if vmxon(region_phys) {
        Ok(rev)
    } else {
        unsafe {
            let mut cr4 = Cr4::read();
            cr4.remove(x86_64::registers::control::Cr4Flags::VIRTUAL_MACHINE_EXTENSIONS);
            Cr4::write(cr4);
        }
        Err("vmxon failed (cpu or bios locked?)")
    }
}

fn vmxon(region_phys: u64) -> bool {
    let mut slot = region_phys;
    let mut cf: u8;
    unsafe {
        asm!(
            "vmxon [{p}]",
            "setc {c}",
            p = in(reg) &mut slot as *mut u64,
            c = out(reg_byte) cf,
            options(nostack)
        );
    }
    cf == 0
}

// all-context tlb invalidations; best-effort: a missing capability leaves
// the flag clear and the callers skip the call entirely
#[repr(C, align(16))]
struct InveptDesc([u64; 2]);

fn invept_all(eptp: u64) {
    if !INVEPT_OK.load(Ordering::Relaxed) {
        return;
    }
    let mut d = InveptDesc([eptp & !0xFFF, 0]);
    unsafe {
        asm!(
            "invept {t}, [{p}]",
            t = in(reg) 2u64, // all-context
            p = in(reg) &mut d as *mut InveptDesc,
            options(nostack)
        );
    }
}

#[repr(C, align(16))]
struct InvvpidDesc([u64; 2]);

fn invvpid_all() {
    if !VPID_OK.load(Ordering::Relaxed) {
        return;
    }
    let mut d = InvvpidDesc([0, 0]);
    unsafe {
        asm!(
            "invvpid {t}, [{p}]",
            t = in(reg) 2u64, // all-context
            p = in(reg) &mut d as *mut InvvpidDesc,
            options(nostack)
        );
    }
}

// ---------------------------------------------------------------- ept

const EPT_R: u64 = 1;
const EPT_W: u64 = 2;
const EPT_X: u64 = 4;
const EPT_WB: u64 = 6 << 3;
const EPT_TABLE: u64 = EPT_R | EPT_W | EPT_X;
const EPT_LEAF: u64 = EPT_R | EPT_W | EPT_X | EPT_WB;
const EPT_PS_2M: u64 = 1 << 7;
const EPT_LEAF_2M: u64 = EPT_LEAF | EPT_PS_2M;
const TWO_MIB: u64 = 2 * 1024 * 1024;

// guest phys window inside the creator's address space
pub const GUEST_WINDOW: u64 = 0x3000_0000;
pub const GUEST_MAX_PAGES: u64 = 8192; // 32 MiB

// ---------------------------------------------------------------- syscalls

// a 2 mib leaf maps a whole region, so its 512 frames must be physically
// contiguous — a scattered run with an aligned first frame must never
// collapse (it would hand the guest physical memory it does not own)
fn region_contiguous(ram: &[PhysFrame<Size4KiB>], start: usize) -> bool {
    let base = ram[start].start_address().as_u64();
    ram[start..start + 512]
        .iter()
        .enumerate()
        .all(|(k, f)| f.start_address().as_u64() == base + k as u64 * 0x1000)
}

pub fn guest_create(pages: u64) -> i64 {
    if !vmx_ready() {
        return -1;
    }
    if pages == 0 || pages > GUEST_MAX_PAGES {
        return -1;
    }
    let cur = match crate::sched::current() {
        Some(c) => c,
        None => return -1,
    };
    // reserved up front: the id doubles as the non-zero vpid
    let id = NEXT_GUEST.fetch_add(1, Ordering::AcqRel);

    // guest ram: prefer one aligned run — 2 mib ept leaves cover a whole
    // 2 mib region, so the run base must carry the alignment; fall back to
    // per-frame allocs (4k leaves) when unsupported or the pool is shredded
    let aligned = if EPT2M_OK.load(Ordering::Relaxed) {
        memory::alloc_frames_aligned(pages, TWO_MIB)
    } else {
        None
    };
    let ram = match aligned {
        Some(v) => v, // alloc_run is atomic: all or nothing
        None => {
            let mut v = Vec::new();
            for _ in 0..pages {
                match memory::alloc_frame() {
                    Some(f) => v.push(f),
                    None => {
                        for f in &v {
                            memory::dealloc_frame(*f);
                        }
                        return -1;
                    }
                }
            }
            v
        }
    };

    // ept tables: pml4 + pdpt + pd + pts, then 2 pages for the apic page
    // tables and 2 data pages (apic-access + virtual-apic)
    let mut tables = Vec::new();
    let npts = pages.div_ceil(512);
    for _ in 0..(3 + npts + 4) {
        match memory::alloc_frame() {
            Some(f) => {
                unsafe { zero_frame(f) };
                tables.push(f);
            }
            None => {
                for f in ram.iter() {
                    memory::dealloc_frame(*f);
                }
                for f in &tables {
                    memory::dealloc_frame(*f);
                }
                return -1;
            }
        }
    }
    let ept_pml4 = tables[0];
    let ept_pdpt = tables[1];
    let ept_pd = tables[2];
    let pts = &tables[3..3 + npts as usize];
    let apic_pd = tables[3 + npts as usize];
    let apic_pt = tables[3 + npts as usize + 1];
    let apic_access = tables[3 + npts as usize + 2];
    let virtual_apic = tables[3 + npts as usize + 3];

    // p4: the posted-interrupt descriptor (PIR 256 bit + ON + NV), kept in
    // its own frame — it goes into tables so destroy frees it like the rest
    let mut pi_desc: Option<PhysFrame<Size4KiB>> = None;
    if PI_ARMED.load(Ordering::Relaxed) {
        if let Some(f) = memory::alloc_frame() {
            unsafe { zero_frame(f) };
            // NV lives at byte 34: informational, but keep it in sync with
            // the vmcs control so software reading the descriptor sees the
            // vector the cpu would consume
            unsafe {
                memory::phys_to_virt(f.start_address())
                    .as_mut_ptr::<u16>()
                    .add(17)
                    .write_volatile(PI_VECTOR as u16);
            }
            pi_desc = Some(f);
        } else {
            serial_println!(
                "[vmx] no frame for pi descriptor — guest runs without posted interrupts"
            );
        }
    }
    let pi_desc_phys = pi_desc.map_or(0, |f| f.start_address().as_u64());

    unsafe {
        let wr = |frame: PhysFrame<Size4KiB>, idx: usize, val: u64| {
            let p = memory::phys_to_virt(frame.start_address()).as_mut_ptr::<u64>();
            p.add(idx).write_volatile(val);
        };
        wr(ept_pml4, 0, ept_pdpt.start_address().as_u64() | EPT_TABLE);
        wr(ept_pdpt, 0, ept_pd.start_address().as_u64() | EPT_TABLE);
        for (k, pt) in pts.iter().enumerate() {
            wr(ept_pd, k, pt.start_address().as_u64() | EPT_TABLE);
        }
        // 2 mib leaves for full, physically aligned regions; the pt frame
        // stays allocated (uniform indexing) but unreferenced. everything
        // else keeps the 4k-leaf path
        let two_m = EPT2M_OK.load(Ordering::Relaxed);
        let mut i = 0usize;
        while i < ram.len() {
            let base = ram[i].start_address().as_u64();
            if two_m
                && i.is_multiple_of(512)
                && i + 512 <= ram.len()
                && base.is_multiple_of(TWO_MIB)
                && region_contiguous(&ram, i)
            {
                wr(ept_pd, i / 512, base | EPT_LEAF_2M);
                i += 512;
            } else {
                let end = (((i / 512) + 1) * 512).min(ram.len());
                for (j, rf) in ram[i..end].iter().enumerate() {
                    wr(
                        pts[i / 512],
                        i % 512 + j,
                        rf.start_address().as_u64() | EPT_LEAF,
                    );
                }
                i = end;
            }
        }
        // the guest apic page: gpa APIC_GPA lands on the access frame (rw,
        // no x); with "virtualize apic accesses" the cpu serves mmio reads
        // from the virtual-apic page instead of a real lapic
        wr(
            ept_pdpt,
            (APIC_GPA >> 30) as usize,
            apic_pd.start_address().as_u64() | EPT_TABLE,
        );
        wr(
            apic_pd,
            ((APIC_GPA >> 21) & 0x1FF) as usize,
            apic_pt.start_address().as_u64() | EPT_TABLE,
        );
        wr(
            apic_pt,
            ((APIC_GPA >> 12) & 0x1FF) as usize,
            apic_access.start_address().as_u64() | EPT_R | EPT_W | EPT_WB,
        );
    }

    // map guest ram into the creator: flat window, rw, no exec
    let mut mapped: u64 = 0;
    {
        let mut aspace = cur.proc.aspace.lock();
        let flags = PageTableFlags::PRESENT
            | PageTableFlags::WRITABLE
            | PageTableFlags::USER_ACCESSIBLE
            | PageTableFlags::NO_EXECUTE;
        for (i, rf) in ram.iter().enumerate() {
            let page = Page::<Size4KiB>::containing_address(VirtAddr::new(
                GUEST_WINDOW + i as u64 * 0x1000,
            ));
            if aspace.map_user_page(page, *rf, flags).is_err() {
                serial_println!("[vmx] guest window map failed at {}", i);
                mapped = i as u64;
                break;
            }
            mapped = i as u64 + 1;
        }
    }
    if (mapped as usize) < ram.len() {
        // unmap the pages we installed, return every frame, bail out
        let mut aspace = cur.proc.aspace.lock();
        for i in 0..mapped {
            let page =
                Page::<Size4KiB>::containing_address(VirtAddr::new(GUEST_WINDOW + i * 0x1000));
            aspace.unmap_user_page(page);
        }
        drop(aspace);
        for f in &ram {
            memory::dealloc_frame(*f);
        }
        for f in &tables {
            memory::dealloc_frame(*f);
        }
        return -1;
    }

    // vmcs
    let Some(vmcs) = memory::alloc_frame() else {
        for f in &ram {
            memory::dealloc_frame(*f);
        }
        for f in &tables {
            memory::dealloc_frame(*f);
        }
        return -1;
    };
    unsafe {
        core::ptr::write_bytes(
            memory::phys_to_virt(vmcs.start_address()).as_mut_ptr::<u8>(),
            0,
            0x1000,
        );
        let basic = Msr::new(IA32_VMX_BASIC).read();
        memory::phys_to_virt(vmcs.start_address())
            .as_mut_ptr::<u32>()
            .write_volatile(basic as u32);
    }
    if !vmptrld(vmcs.start_address().as_u64()) {
        memory::dealloc_frame(vmcs);
        for f in &ram {
            memory::dealloc_frame(*f);
        }
        for f in &tables {
            memory::dealloc_frame(*f);
        }
        return -1;
    }
    if !setup_vmcs(
        vmcs,
        ept_pml4,
        MSR_BM.get().copied().unwrap_or(0),
        id,
        apic_access.start_address().as_u64(),
        virtual_apic.start_address().as_u64(),
        pi_desc_phys,
    ) {
        serial_println!("[vmx] vmcs setup failed");
        let _ = vmclear(vmcs.start_address().as_u64());
        memory::dealloc_frame(vmcs);
        for f in &ram {
            memory::dealloc_frame(*f);
        }
        for f in &tables {
            memory::dealloc_frame(*f);
        }
        return -1;
    }
    let _ = vmclear(vmcs.start_address().as_u64());

    let vcpu = Vcpu {
        _vmcs: vmcs.start_address().as_u64(),
        _launched: 0,
        gprs: [0; 16],
        out: [0; 20],
        _exit_stack_top: 0,
        _orig_rsp: 0,
        pi_desc: pi_desc_phys,
        vapic: virtual_apic.start_address().as_u64(),
        pi_routed: [0; 4],
    };
    let exit_stack = Box::new(ExitStack([0; 4096]));
    if let Some(f) = pi_desc {
        tables.push(f); // freed by guest_destroy with the rest
    }
    GUESTS.lock().push(Guest {
        id,
        owner: cur.proc.clone(),
        ram,
        tables,
        vmcs,
        exit_stack,
        vcpu,
        launched: false,
        mode: 0,
        pi_desc,
        pi_routed: [0; 4],
        domain: None,
    });
    serial_println!(
        "[vmx] guest {} created: pid {} pages {} eptp {:#x} vpid {} pi {}",
        id,
        cur.proc.id,
        pages,
        ept_pml4.start_address().as_u64(),
        id & 0xFFFF,
        pi_desc_phys != 0
    );
    // the caller's root capability on the guest: unguessable, every right
    let root = caps::install_root(caps::Obj::guest(id), cur.proc.id);
    root as i64
}

// write every field a real-mode unrestricted guest needs; any field the
// host vmcs has no slot for is reported by encoding so the serial log
// names the culprit instead of failing the whole setup blind
fn setup_vmcs(
    vmcs: PhysFrame<Size4KiB>,
    ept_pml4: PhysFrame<Size4KiB>,
    msr_bm: u64,
    vpid: u64,
    apic_access: u64,
    virtual_apic: u64,
    pi_desc: u64,
) -> bool {
    let _ = vmcs;
    let mut failed: Vec<u64> = Vec::new();
    let mut w = |f: u64, v: u64| {
        if !vmwrite(f, v) {
            failed.push(f);
        }
    };

    // controls; the capability msrs depend on basic[55] (true ctls or raw)
    let (m_pin, m_proc, m_exit, m_entry) = if TRUE_CTLS.load(Ordering::Relaxed) {
        (
            IA32_VMX_TRUE_PIN,
            IA32_VMX_TRUE_PROC,
            IA32_VMX_TRUE_EXIT,
            IA32_VMX_TRUE_ENTRY,
        )
    } else {
        (IA32_VMX_PIN, IA32_VMX_PROC, IA32_VMX_EXIT, IA32_VMX_ENTRY)
    };
    let mut pin_desired = 1 | (1 << 3); // external interrupt exiting | nmi exiting
    if PREEMPT_ARMED.load(Ordering::Relaxed) {
        pin_desired |= 1 << 6; // p3: preemption timer bounds every guest slice
    }
    // p4: posted interrupts ride on the apic-virt suite — pin bit 7 needs
    // vid (proc2 bit 9), vid needs the tpr shadow and access-virt (proc2
    // bit 0). without access-virt the whole stack must stay off, or vm
    // entry fails the control dependency checks on real hardware
    let apicv_allowed = adjust_ctl(IA32_VMX_PROC2, 1 << 0) & (1 << 0) != 0;
    if pi_desc != 0 && apicv_allowed {
        pin_desired |= PIN_POSTED; // p4: process posted interrupts (needs vid)
    }
    let pin = adjust_ctl(m_pin, pin_desired);
    w(VMX_PIN_CTLS, pin);
    if PREEMPT_ARMED.load(Ordering::Relaxed) {
        // must be non-zero before the first entry: an armed timer at 0
        // would exit instantly. vcpu_run reloads it on every entry
        w(
            VMX_PREEMPT_TIMER,
            preempt_slice(PREEMPT_RATE.load(Ordering::Relaxed)),
        );
    }
    // secondary first: ept | unrestricted guest | virtualize apic accesses
    // (bit 0 — NOT bit 5, which is vpid) | apic-register virtualization
    // (bit 8 is only legal under bit 0) | virtual-interrupt delivery (p4,
    // when a descriptor exists); dependent primary bits are added only when
    // the secondary ones actually survived the capability clamp
    let mut proc2_desired = (1 << 1)
        | (1 << 7)
        | (1 << 0)
        | (1 << 8)
        | if pi_desc != 0 && apicv_allowed {
            PROC2_VID
        } else {
            0
        };
    if VPID_OK.load(Ordering::Relaxed) {
        proc2_desired |= 1 << 5; // vpid: tagged tlb, no flush between entries
    }
    let mut proc2 = adjust_ctl(IA32_VMX_PROC2, proc2_desired);
    let apic_virt = proc2 & (1 << 0) != 0;
    if !apic_virt {
        // reg-virt and vid cannot stand without access-virt
        proc2 &= !((1 << 8) | (1 << 9));
    }
    w(VMX_PROC_CTLS2, proc2);
    if pi_desc != 0 && proc2 & PROC2_VID != 0 {
        // the pair survived the clamp: wire the descriptor and the
        // notification vector. vid alone already delivers from vIRR/RVI at
        // entry; the pin bit only adds the cpu-side notification path
        w(VMX_POSTED_INTR_DESC, pi_desc);
        w(VMX_POSTED_INTR_NV, PI_VECTOR);
    }
    let mut proc_desired = (1 << 7) | (1 << 24) | (1 << 31); // hlt | unconditional io | secondary;
                                                             // io exiting keeps guest ports away
                                                             // from the real pic/pit/keyboard
    if msr_bm != 0 {
        proc_desired |= 1 << 28; // use the msr bitmap
    }
    // tpr/cr8 shadow rides with apic-virt and is mandatory under vid
    if apic_virt || proc2 & PROC2_VID != 0 {
        proc_desired |= 1 << 21;
    }
    let proc = adjust_ctl(m_proc, proc_desired);
    w(VMX_PROC_CTLS, proc);
    let exit_ctl = adjust_ctl(
        m_exit,
        (1 << 9)    // host address-space size (64-bit)
            | (1 << 15) // acknowledge interrupt on exit
            | (1 << 20) // save ia32_efer on exit
            | (1 << 21), // load ia32_efer on exit
    );
    w(VMX_EXIT_CTLS, exit_ctl);
    let entry_ctl = adjust_ctl(m_entry, 1 << 15); // load ia32_efer
    w(VMX_ENTRY_CTLS, entry_ctl);
    w(VMX_EXCEPTION_BITMAP, (1 << 6) | (1 << 13)); // #ud, #gp exit to host
    w(VMX_MSR_BITMAPS, msr_bm); // 0 = all msr access exits (bitmap not armed)
    if proc2 & (1 << 5) != 0 {
        // vpid is secondary proc-ctl bit 5 (primary bit 5 is not vpid)
        w(VMX_VPID, vpid & 0xFFFF); // guest ids start at 1: never zero
    }
    if apic_virt {
        w(VMX_APIC_ACCESS, apic_access);
        w(VMX_VIRTUAL_APIC, virtual_apic);
        w(VMX_TPR_THRESHOLD, 0);
    }

    // host state
    w(HOST_CR0, Cr0::read_raw());
    w(
        HOST_CR3,
        memory::KERNEL_PML4
            .get()
            .map_or(0, |f| f.start_address().as_u64()),
    );
    w(HOST_CR4, Cr4::read_raw());
    w(HOST_RSP, 0); // patched below with the real stack top
    w(
        HOST_RIP,
        vmx_exit_stub as unsafe extern "C" fn() as usize as u64,
    );
    w(HOST_CS_SEL, gdt::KERNEL_CODE as u64);
    w(HOST_SS_SEL, gdt::KERNEL_DATA as u64);
    w(HOST_DS_SEL, gdt::KERNEL_DATA as u64);
    w(HOST_ES_SEL, gdt::KERNEL_DATA as u64);
    w(HOST_FS_SEL, 0);
    w(HOST_GS_SEL, 0);
    w(HOST_TR_SEL, gdt::TSS_SEL as u64);
    w(HOST_FS_BASE, 0);
    w(HOST_GS_BASE, 0);
    w(HOST_TR_BASE, gdt::tss_base());
    w(HOST_SYSENTER_ESP, 0);
    w(HOST_SYSENTER_EIP, 0);
    w(HOST_EFER, unsafe { Msr::new(0xC000_0080).read() });

    // guest state: flat real mode
    let cr0_fixed0 = unsafe { Msr::new(IA32_VMX_CR0_FIXED0).read() };
    let cr0_fixed1 = unsafe { Msr::new(IA32_VMX_CR0_FIXED1).read() };
    let cr4_fixed0 = unsafe { Msr::new(IA32_VMX_CR4_FIXED0).read() };
    let cr4_fixed1 = unsafe { Msr::new(IA32_VMX_CR4_FIXED1).read() };
    let gcr0 = ((0x6000_0010u64 | cr0_fixed0) & cr0_fixed1) & !0x8000_0001; // no PE/PG
    let gcr4 = (0x2000u64 | cr4_fixed0) & cr4_fixed1; // vmxe per fixed bits
    w(GUEST_CR0, gcr0);
    w(GUEST_CR3, 0); // guest cr3
    w(GUEST_CR4, gcr4);
    // vmxe is kernel-owned: guest writes to it exit (reason 28, reportable)
    // and reads see it set — a guest can never drop itself out of the ept
    // walk (cr4 mask/shadow must agree with the cr4 field on masked bits)
    w(VMX_CR4_GUEST_HOST_MASK, 1 << 13); // vmxe
    w(VMX_CR4_READ_SHADOW, 0x2000);
    w(GUEST_DR7, 0x400);
    w(GUEST_RSP, 0);
    w(GUEST_RIP, 0);
    w(GUEST_RFLAGS, if pi_desc != 0 { 0x202 } else { 0x2 }); // if=1 for vid delivery
    w(GUEST_PENDING_DBG, 0);
    w(GUEST_SYSENTER_ESP, 0);
    w(GUEST_SYSENTER_EIP, 0);
    w(GUEST_ACTIVITY_STATE, 0);
    w(GUEST_INTERRUPTIBILITY, 0);
    // no guest smbase: only smi delivery reads it, we never inject smis,
    // and kvm's vmcs12 rejects the field outright (root cause of the
    // v0.10.0 "vmcs setup failed" on -enable-kvm hosts)
    w(GUEST_SYSENTER_CS, 0);
    w(GUEST_DEBUGCTL, 0);
    w(GUEST_EFER, 0);
    w(VMCS_LINK_PTR, u64::MAX);
    w(GUEST_GDTR_BASE, 0);
    w(GUEST_GDTR_LIMIT, 0xFFFF);
    w(GUEST_IDTR_BASE, 0);
    w(GUEST_IDTR_LIMIT, 0xFFFF);

    // segments: cs code-ish, the rest data-ish, ldtr unusable
    let segs: [(u64, u64, u64, u64); 6] = [
        (GUEST_CS_SEL, GUEST_CS_LIMIT, GUEST_CS_AR, GUEST_CS_BASE),
        (GUEST_SS_SEL, GUEST_SS_LIMIT, GUEST_SS_AR, GUEST_SS_BASE),
        (GUEST_DS_SEL, GUEST_DS_LIMIT, GUEST_DS_AR, GUEST_DS_BASE),
        (GUEST_ES_SEL, GUEST_ES_LIMIT, GUEST_ES_AR, GUEST_ES_BASE),
        (GUEST_FS_SEL, GUEST_FS_LIMIT, GUEST_FS_AR, GUEST_FS_BASE),
        (GUEST_GS_SEL, GUEST_GS_LIMIT, GUEST_GS_AR, GUEST_GS_BASE),
    ];
    for (sel, lim, ar, base) in segs {
        w(sel, 0);
        w(lim, 0xFFFF);
        w(ar, if ar == GUEST_CS_AR { 0x9B } else { 0x93 });
        w(base, 0);
    }
    w(GUEST_LDTR_SEL, 0);
    w(GUEST_LDTR_LIMIT, 0);
    w(GUEST_LDTR_AR, 0x1_0000); // unusable
    w(GUEST_LDTR_BASE, 0);
    w(GUEST_TR_SEL, 0);
    w(GUEST_TR_LIMIT, 0xFFFF);
    w(GUEST_TR_AR, 0x8B);
    w(GUEST_TR_BASE, 0);

    // ept pointer: bits 2:0 = page-walk length 3 (4-level walk), bits 5:3
    // = memory type wb (6)
    let eptp = ept_pml4.start_address().as_u64() | 3 | (6 << 3);
    w(VMX_EPTP, eptp);
    if failed.is_empty() {
        return true;
    }
    let ierr = vmread(VM_INSTRUCTION_ERROR);
    serial_println!(
        "[vmx] vmcs write failed: fields {:?} (vm-instruction error {})",
        failed,
        ierr
    );
    false
}

// switch a guest to the long-mode preset (mode 1): the monitor preloads
// page tables / gdt / 64-bit code into guest ram through its window and
// points rip/rsp/cr3 at them via the vcpu_run control half (flags bit2).
// real-mode guests are unaffected; the switch is idempotent (GMODE right)
pub fn guest_mode(handle: u64, mode: u64) -> i64 {
    if !vmx_ready() || mode != 1 {
        return -1;
    }
    let cur = match crate::sched::current() {
        Some(c) => c,
        None => return -1,
    };
    let Some(obj) = caps::lookup(handle, cur.proc.id, caps::K_GUEST, caps::GMODE) else {
        return -1;
    };
    let mut guests = GUESTS.lock();
    let Some(g) = guests.iter_mut().find(|g| g.id == obj.id) else {
        return -1;
    };
    if g.mode == 1 {
        return 0;
    }
    if !vmptrld(g.vmcs.start_address().as_u64()) {
        return -1;
    }
    let ok = apply_long_mode();
    let _ = vmclear(g.vmcs.start_address().as_u64());
    if !ok {
        serial_println!("[vmx] guest {}: long mode setup failed", g.id);
        return -1;
    }
    g.mode = 1;
    serial_println!("[vmx] guest {}: long mode armed", g.id);
    0
}

// rewrite the guest-state block for ia-32e entry: pe|pg, pae, efer lme|lma,
// a 64-bit cs and flat data segments. gdtr/idtr point at monitor-provided
// tables in guest ram (fixed layout, see gdemo64). every value satisfies
// the vm-entry long-mode checks (sdm vol 3d 26.2.1.3).
fn apply_long_mode() -> bool {
    let mut ok = true;
    let mut w = |f: u64, v: u64| {
        ok &= vmwrite(f, v);
    };
    let m_entry = if TRUE_CTLS.load(Ordering::Relaxed) {
        IA32_VMX_TRUE_ENTRY
    } else {
        IA32_VMX_ENTRY
    };
    // load ia32_efer | ia-32e mode guest
    w(VMX_ENTRY_CTLS, adjust_ctl(m_entry, (1 << 15) | (1 << 9)));
    let cr0_fixed0 = unsafe { Msr::new(IA32_VMX_CR0_FIXED0).read() };
    let cr0_fixed1 = unsafe { Msr::new(IA32_VMX_CR0_FIXED1).read() };
    let cr4_fixed0 = unsafe { Msr::new(IA32_VMX_CR4_FIXED0).read() };
    let cr4_fixed1 = unsafe { Msr::new(IA32_VMX_CR4_FIXED1).read() };
    w(GUEST_CR0, (0x8005_0033u64 | cr0_fixed0) & cr0_fixed1); // pe|pg now
    w(GUEST_CR4, ((1 << 5) | 0x2000u64 | cr4_fixed0) & cr4_fixed1); // pae|vmxe
    w(GUEST_EFER, 0x500); // lme | lma
    w(GUEST_CS_SEL, 0x08);
    w(GUEST_CS_LIMIT, 0xFFFF_FFFF);
    w(GUEST_CS_AR, 0xA09B); // l=1 g=1 p=1 exec/read accessed, dpl 0
    w(GUEST_CS_BASE, 0);
    let flat: [(u64, u64, u64, u64); 5] = [
        (GUEST_DS_SEL, GUEST_DS_LIMIT, GUEST_DS_AR, GUEST_DS_BASE),
        (GUEST_ES_SEL, GUEST_ES_LIMIT, GUEST_ES_AR, GUEST_ES_BASE),
        (GUEST_SS_SEL, GUEST_SS_LIMIT, GUEST_SS_AR, GUEST_SS_BASE),
        (GUEST_FS_SEL, GUEST_FS_LIMIT, GUEST_FS_AR, GUEST_FS_BASE),
        (GUEST_GS_SEL, GUEST_GS_LIMIT, GUEST_GS_AR, GUEST_GS_BASE),
    ];
    for (sel, lim, ar, base) in flat {
        w(sel, 0);
        w(lim, 0xFFFF_FFFF);
        w(ar, 0xC093); // g=1 p=1 r/w accessed, dpl 0 (== cs dpl)
        w(base, 0);
    }
    // monitor-provided descriptor tables in guest ram (fixed convention)
    w(GUEST_GDTR_BASE, 0x5000);
    w(GUEST_GDTR_LIMIT, 0x17);
    w(GUEST_IDTR_BASE, 0x6000);
    w(GUEST_IDTR_LIMIT, 0xFFF); // nmi-exiting demands >= 0xfff
    w(GUEST_RFLAGS, 0x2);
    w(GUEST_INTERRUPTIBILITY, 0);
    ok
}

// two-way exit buffer in user memory: the kernel writes 20 u64 of exit
// info (guest→host), ring 3 writes the control half back before the next
// run (host→guest): flags, an optional event injection, entry state and
// gpr overrides
const OFF_IN_FLAGS: usize = 20; // bit0 = load gprs, bit1 = inject event, bit2 = load entry
const OFF_IN_INJECT: usize = 21; // raw vm-entry interruption-info field
const OFF_IN_ERR: usize = 22; // exception error code (inject bit 11)
const OFF_IN_LEN: usize = 23; // instruction length for exceptions
const OFF_IN_RIP: usize = 24; // entry rip (flags bit2)
const OFF_IN_RSP: usize = 25; // entry rsp
const OFF_IN_CR3: usize = 26; // entry cr3 (guest phys, validated)
const OFF_IN_GPRS: usize = 28; // 16 u64 loaded when flags bit0
const EXIT_BUF_BYTES: usize = 352; // 44 u64 total

// enter the guest until a reportable exit; returns the reason.
// `handle` is the caller's guest capability (GRUN right)
pub fn vcpu_run(handle: u64, out_buf: u64) -> i64 {
    if !vmx_ready() {
        return -1;
    }
    if out_buf & 7 != 0 || out_buf + EXIT_BUF_BYTES as u64 > 0x0000_8000_0000_0000 {
        return -1;
    }
    let cur = match crate::sched::current() {
        Some(c) => c,
        None => return -1,
    };
    let Some(obj) = caps::lookup(handle, cur.proc.id, caps::K_GUEST, caps::GRUN) else {
        serial_println!(
            "[vmx] vcpu_run denied: pid {} holds no guest capability {:#x}",
            cur.proc.id,
            handle
        );
        return -1;
    };
    let mut guests = GUESTS.lock();
    let Some(g) = guests.iter_mut().find(|g| g.id == obj.id) else {
        return -1;
    };

    // host→guest control half: must be read while the caller's address
    // space is still live (the launch switches to kernel cr3)
    let mut ctl = [0u8; EXIT_BUF_BYTES - 160];
    let have_ctl = cur.proc.aspace.lock().read_bytes(out_buf + 160, &mut ctl);

    // park the active vmcs and point host rsp at this guest's exit stack
    if !vmptrld(g.vmcs.start_address().as_u64()) {
        return -1;
    }
    g.vcpu._exit_stack_top = g.exit_stack.0.as_ptr() as u64 + 4096;
    // host rsp must point at the stack top when the exit lands
    let _ = vmwrite(HOST_RSP, g.vcpu._exit_stack_top);
    // p3: reload the preemption timer on every entry — the countdown
    // persists across exits, a stale value would shorten the next slice
    if PREEMPT_ARMED.load(Ordering::Relaxed) {
        let _ = vmwrite(
            VMX_PREEMPT_TIMER,
            preempt_slice(PREEMPT_RATE.load(Ordering::Relaxed)),
        );
    }
    // p4: hand anything the host posted to the vAPIC before the guest sees
    // the cpu again; the stub's pi_routed copy drives the exit-side posting
    g.vcpu.pi_routed = g.pi_routed;
    if g.pi_desc.is_some() {
        sync_posted(&mut g.vcpu);
    }

    // apply the ring-3 requests: entry state, gpr overrides, then injection
    if have_ctl {
        let flags = u64::from_le_bytes(ctl[(OFF_IN_FLAGS * 8 - 160)..][..8].try_into().unwrap());
        if flags & 4 != 0 {
            // long-mode entry point: rip/rsp/cr3 point at images the monitor
            // preloaded into guest ram; cr3 must be inside the guest
            let rd = |off: usize| u64::from_le_bytes(ctl[off * 8 - 160..][..8].try_into().unwrap());
            let (rip, rsp, cr3) = (rd(OFF_IN_RIP), rd(OFF_IN_RSP), rd(OFF_IN_CR3));
            if cr3 < g.ram.len() as u64 * 0x1000 {
                let _ = vmwrite(GUEST_CR3, cr3);
                let _ = vmwrite(GUEST_RIP, rip);
                let _ = vmwrite(GUEST_RSP, rsp);
            } else {
                serial_println!("[vmx] bad entry cr3 {:#x} — ignored", cr3);
            }
        }
        if flags & 1 != 0 {
            for (i, slot) in g.vcpu.gprs.iter_mut().enumerate() {
                let b = &ctl[OFF_IN_GPRS * 8 - 160 + i * 8..][..8];
                *slot = u64::from_le_bytes(b.try_into().unwrap());
            }
        }
        if flags & 2 != 0 {
            let inj = u64::from_le_bytes(ctl[(OFF_IN_INJECT * 8 - 160)..][..8].try_into().unwrap());
            let typ = (inj >> 8) & 7;
            if inj & 0x8000_0000 != 0 && (typ == 0 || typ == 2 || typ == 3) {
                vmwrite(VM_ENTRY_INTR_INFO, inj);
                if inj & (1 << 11) != 0 {
                    let err =
                        u64::from_le_bytes(ctl[(OFF_IN_ERR * 8 - 160)..][..8].try_into().unwrap());
                    vmwrite(VM_ENTRY_EXCEPTION_ERR_CODE, err);
                }
                let mut ilen =
                    u64::from_le_bytes(ctl[(OFF_IN_LEN * 8 - 160)..][..8].try_into().unwrap());
                if typ == 3 && ilen == 0 {
                    ilen = 1; // intercepted faults need a non-zero length
                }
                vmwrite(VM_ENTRY_INSTRUCTION_LEN, ilen);
            } else {
                vmwrite(VM_ENTRY_INTR_INFO, 0); // bad field: enter clean
                serial_println!("[vmx] bad injection request {:#x} — ignored", inj);
            }
        } else {
            vmwrite(VM_ENTRY_INTR_INFO, 0);
        }
    }

    // run with kernel cr3: the stub and handler live in the higher half
    unsafe {
        Cr3::write(
            *memory::KERNEL_PML4.get().expect("kernel pml4"),
            Cr3Flags::empty(),
        )
    };
    let first = !g.launched as u32;
    let vcpu_ptr: *mut Vcpu = &mut g.vcpu;
    let err = unsafe { vmx_launch_asm(vcpu_ptr, first) };
    g.launched = true;
    let reason = g.vcpu.out[0];
    // the out half only: the user's control fields stay untouched for the
    // next vcpu_run call
    let mut buf = [0u8; 160];
    for (i, v) in g.vcpu.out.iter().enumerate() {
        buf[i * 8..i * 8 + 8].copy_from_slice(&v.to_le_bytes());
    }
    // release the guest list BEFORE interrupts come back: a timer tick
    // here can switch tasks, and holding the lock across a yield would
    // spin every other guest syscall until this one runs again
    drop(guests);
    // interrupts were cleared by the vm-exit path
    x86_64::instructions::interrupts::enable();
    // hand the address space back to the caller before touching user memory
    cur.proc.switch();

    if err != 0 {
        let ierr = vmread(VM_INSTRUCTION_ERROR);
        serial_println!("[vmx] launch/resume failed: err {} vmfail {}", err, ierr);
        return -1;
    }
    if !cur.proc.aspace.lock().write_bytes(out_buf, &buf) {
        serial_println!("[vmx] bad out buffer");
        return -1;
    }
    reason as i64
}

// grow a guest at runtime: `count` fresh pages append to guest phys memory
// (flat gpa layout), the ept and the creator's window; returns the new size
// (GMAP right)
pub fn guest_map(handle: u64, count: u64) -> i64 {
    if !vmx_ready() || count == 0 {
        return -1;
    }
    let cur = match crate::sched::current() {
        Some(c) => c,
        None => return -1,
    };
    let Some(obj) = caps::lookup(handle, cur.proc.id, caps::K_GUEST, caps::GMAP) else {
        return -1;
    };
    let mut guests = GUESTS.lock();
    let Some(g) = guests.iter_mut().find(|g| g.id == obj.id) else {
        return -1;
    };
    let old_len = g.ram.len() as u64;
    if old_len + count > GUEST_MAX_PAGES {
        return -1;
    }

    // fresh ram frames, rolled back on any failure below; aligned runs
    // preferred so completed regions can collapse into 2 mib leaves
    let mut new_ram: Vec<PhysFrame<Size4KiB>> = Vec::new();
    let aligned = if EPT2M_OK.load(Ordering::Relaxed) {
        memory::alloc_frames_aligned(count, TWO_MIB)
    } else {
        None
    };
    if let Some(v) = aligned {
        new_ram = v; // atomic: all or nothing
    } else {
        for _ in 0..count {
            match memory::alloc_frame() {
                Some(f) => new_ram.push(f),
                None => {
                    for f in &new_ram {
                        memory::dealloc_frame(*f);
                    }
                    return -1;
                }
            }
        }
    }

    // page tables for the gpa range the new pages land in
    let old_npts = old_len.div_ceil(512);
    let new_npts = (old_len + count).div_ceil(512);
    let mut new_pts: Vec<PhysFrame<Size4KiB>> = Vec::new();
    for _ in old_npts..new_npts {
        match memory::alloc_frame() {
            Some(f) => {
                unsafe { zero_frame(f) };
                new_pts.push(f);
            }
            None => {
                for f in &new_ram {
                    memory::dealloc_frame(*f);
                }
                for f in &new_pts {
                    memory::dealloc_frame(*f);
                }
                return -1;
            }
        }
    }
    // extend the creator's window; unmap on partial failure. all ept
    // mutations happen only after this succeeds — a rolled-back growth
    // must never leave stale translations behind
    let mut mapped: u64 = 0;
    {
        let mut aspace = g.owner.aspace.lock();
        let flags = PageTableFlags::PRESENT
            | PageTableFlags::WRITABLE
            | PageTableFlags::USER_ACCESSIBLE
            | PageTableFlags::NO_EXECUTE;
        for (i, rf) in new_ram.iter().enumerate() {
            let page = Page::<Size4KiB>::containing_address(VirtAddr::new(
                GUEST_WINDOW + (old_len as usize + i) as u64 * 0x1000,
            ));
            if aspace.map_user_page(page, *rf, flags).is_err() {
                mapped = i as u64;
                break;
            }
            mapped = i as u64 + 1;
        }
    }
    if mapped < count {
        let mut aspace = g.owner.aspace.lock();
        for i in 0..mapped {
            let page = Page::<Size4KiB>::containing_address(VirtAddr::new(
                GUEST_WINDOW + (old_len + i) * 0x1000,
            ));
            aspace.unmap_user_page(page);
        }
        drop(aspace);
        for f in &new_ram {
            memory::dealloc_frame(*f);
        }
        for f in &new_pts {
            memory::dealloc_frame(*f);
        }
        return -1;
    }

    // commit, then wire the ept: new cacheable translations appear below,
    // so the flush comes last
    g.ram.extend(new_ram);
    g.tables.extend(new_pts);
    let ept_pd = g.tables[2];
    unsafe {
        let pd_ptr = memory::phys_to_virt(ept_pd.start_address()).as_mut_ptr::<u64>();
        for (k, pt) in g.tables[3 + old_npts as usize..3 + new_npts as usize]
            .iter()
            .enumerate()
        {
            pd_ptr
                .add(old_npts as usize + k)
                .write_volatile(pt.start_address().as_u64() | EPT_TABLE);
        }
        for (i, rf) in g.ram[old_len as usize..].iter().enumerate() {
            let gi = old_len as usize + i;
            let p =
                memory::phys_to_virt(g.tables[3 + gi / 512].start_address()).as_mut_ptr::<u64>();
            p.add(gi % 512)
                .write_volatile(rf.start_address().as_u64() | EPT_LEAF);
        }
        // p3: collapse completed, physically aligned regions into 2 mib
        // leaves — the old tail finished by this growth and full new
        // regions; pt frames stay allocated for uniform indexing
        if EPT2M_OK.load(Ordering::Relaxed) {
            let total = g.ram.len();
            for k in 0..total.div_ceil(512) {
                let start = k * 512;
                if start + 512 <= total {
                    let base = g.ram[start].start_address().as_u64();
                    if base.is_multiple_of(TWO_MIB) && region_contiguous(&g.ram, start) {
                        pd_ptr.add(k).write_volatile(base | EPT_LEAF_2M);
                    }
                }
            }
        }
    }
    let eptp = g.tables[0].start_address().as_u64();
    // a device-assignment domain mirrors the growth too
    if let Some(d) = g.domain.as_mut() {
        for (i, rf) in g.ram[old_len as usize..].iter().enumerate() {
            if !d.map((old_len as usize + i) as u64 * 0x1000, rf.start_address()) {
                serial_println!(
                    "[iommu] guest {}: mirror growth failed at page {}",
                    obj.id,
                    old_len as usize + i
                );
                break;
            }
        }
    }
    drop(guests);
    invept_all(eptp);
    serial_println!(
        "[vmx] guest {} mapped: +{} pages -> {} total",
        obj.id,
        count,
        old_len + count
    );
    (old_len + count) as i64
}

// p4: route host irq `vector` into the guest's posted-interrupt
// descriptor: on the next ext-int exit with that vector the kernel posts
// it instead of reporting it to ring 3, and virtual-interrupt delivery
// injects it into the guest without further exits. vector 0 clears every
// routing. needs apicv (pin bit 7 + vid allowed); refused elsewhere.
pub fn guest_pi(handle: u64, vector: u64) -> i64 {
    if !vmx_ready() || vector > 255 {
        return -1;
    }
    if !PI_ARMED.load(Ordering::Relaxed) {
        serial_println!("[vmx] guest_pi: posted interrupts unavailable (no apicv/vid)");
        return -1;
    }
    let cur = match crate::sched::current() {
        Some(c) => c,
        None => return -1,
    };
    let Some(obj) = caps::lookup(handle, cur.proc.id, caps::K_GUEST, caps::GPI) else {
        return -1;
    };
    let mut guests = GUESTS.lock();
    let Some(g) = guests.iter_mut().find(|g| g.id == obj.id) else {
        return -1;
    };
    if vector == 0 {
        g.pi_routed = [0; 4];
        serial_println!("[vmx] guest {}: posted-interrupt routing cleared", g.id);
        return 0;
    }
    g.pi_routed[(vector >> 6) as usize] |= 1 << (vector & 63);
    serial_println!(
        "[vmx] guest {}: host vector {:#x} -> posted interrupts (nv {:#x})",
        g.id,
        vector,
        PI_VECTOR
    );
    0
}

// device assignment: the guest's iommu domain mirrors the ept — device
// dma walks the same guest-physical memory the cpu does. the domain is
// built on first attach and follows guest_map growth afterwards
pub fn guest_device_attach(handle: u64, sid: u64) -> i64 {
    if sid > 0xFFFF || sid == 0 {
        return -1;
    }
    let cur = match crate::sched::current() {
        Some(c) => c,
        None => return -1,
    };
    let Some(obj) = caps::lookup(handle, cur.proc.id, caps::K_GUEST, caps::GDEV) else {
        return -1;
    };
    let mut guests = GUESTS.lock();
    let Some(g) = guests.iter_mut().find(|g| g.id == obj.id) else {
        return -1;
    };
    if g.domain.is_none() {
        let mut dom = match crate::iommu::Domain::new() {
            Some(d) => d,
            None => {
                serial_println!("[iommu] guest {}: no domain (no unit or ids out)", obj.id);
                return -1;
            }
        };
        let mut ok = true;
        for (i, f) in g.ram.iter().enumerate() {
            if !dom.map(i as u64 * 0x1000, f.start_address()) {
                serial_println!("[iommu] guest {}: mirror failed at page {}", obj.id, i);
                ok = false;
                break;
            }
        }
        if !ok {
            drop(dom);
            return -1;
        }
        serial_println!(
            "[iommu] guest {}: domain {} mirrors {} pages",
            obj.id,
            dom.id,
            g.ram.len()
        );
        g.domain = Some(dom);
    }
    match g.domain.as_mut().unwrap().attach(sid as u16) {
        true => 0,
        false => -1,
    }
}

// give the device back: its context entry leaves the domain (dma from it
// faults until it is attached elsewhere)
pub fn guest_device_detach(handle: u64, sid: u64) -> i64 {
    if sid > 0xFFFF || sid == 0 {
        return -1;
    }
    let cur = match crate::sched::current() {
        Some(c) => c,
        None => return -1,
    };
    let Some(obj) = caps::lookup(handle, cur.proc.id, caps::K_GUEST, caps::GDEV) else {
        return -1;
    };
    let mut guests = GUESTS.lock();
    let Some(g) = guests.iter_mut().find(|g| g.id == obj.id) else {
        return -1;
    };
    match g.domain.as_mut().map(|d| d.detach(sid as u16)) {
        Some(true) => 0,
        _ => -1,
    }
}

pub fn guest_destroy(handle: u64) -> i64 {
    let cur = match crate::sched::current() {
        Some(c) => c,
        None => return -1,
    };
    let Some(obj) = caps::lookup(handle, cur.proc.id, caps::K_GUEST, caps::GDESTROY) else {
        return -1;
    };
    let mut guests = GUESTS.lock();
    let Some(pos) = guests.iter().position(|g| g.id == obj.id) else {
        return -1;
    };
    let mut g = guests.remove(pos);
    drop(guests);
    caps::drop_obj(caps::Obj::guest(g.id));
    let nram = g.ram.len();
    // the domain's devices stop first: their dma must never reach frames
    // that are about to return to the pool
    let _ = g.domain.take();
    // detach the window from the creator's space, then free everything
    let mut freed: u64 = 0;
    {
        let mut aspace = g.owner.aspace.lock();
        for i in 0..nram as u64 {
            let page =
                Page::<Size4KiB>::containing_address(VirtAddr::new(GUEST_WINDOW + i * 0x1000));
            aspace.unmap_user_page(page);
        }
    }
    for f in &g.ram {
        if memory::dealloc_frame(*f) {
            freed += 1;
        }
    }
    for f in &g.tables {
        if memory::dealloc_frame(*f) {
            freed += 1;
        }
    }
    // clear the vmcs before its frame returns to the pool: the stale
    // current-vmcs pointer must not reference freed memory when the next
    // guest's vmptrld lands
    let _ = vmclear(g.vmcs.start_address().as_u64());
    if memory::dealloc_frame(g.vmcs) {
        freed += 1;
    }
    // the frames go back to the pool: stale vpid/ept translations must not
    // survive a future guest that reuses the numbers
    invvpid_all();
    invept_all(g.tables[0].start_address().as_u64());
    serial_println!(
        "[vmx] guest {} destroyed: {} ram frames, {} ept/vmcs frames freed",
        g.id,
        nram,
        freed
    );
    0
}

// a process died: its guests go with it — the window dies with the space,
// every ram/ept/vmcs frame goes back to the pool. runs before aspace
// teardown so the window mappings are gone before the sweep reaches them.
// without this the GUESTS entry leaked tables/vmcs on every kill (and the
// teardown would free the ram frames through the window a second time)
pub fn close_process(proc: &Arc<Process>) {
    let dead: Vec<Guest> = {
        let mut guests = GUESTS.lock();
        let (mine, rest): (Vec<_>, Vec<_>) = guests.drain(..).partition(|g| g.owner.id == proc.id);
        *guests = rest;
        mine
    };
    for mut g in dead {
        caps::drop_obj(caps::Obj::guest(g.id));
        let _ = g.domain.take();
        let mut aspace = proc.aspace.lock();
        for i in 0..g.ram.len() as u64 {
            let page =
                Page::<Size4KiB>::containing_address(VirtAddr::new(GUEST_WINDOW + i * 0x1000));
            aspace.unmap_user_page(page);
        }
        drop(aspace);
        for f in &g.ram {
            memory::dealloc_frame(*f);
        }
        for f in &g.tables {
            memory::dealloc_frame(*f);
        }
        let _ = vmclear(g.vmcs.start_address().as_u64());
        memory::dealloc_frame(g.vmcs);
        invvpid_all();
        invept_all(g.tables[0].start_address().as_u64());
        serial_println!("[vmx] guest {} closed (owner {} gone)", g.id, proc.id);
    }
}

unsafe fn zero_frame(frame: PhysFrame<Size4KiB>) {
    let virt = memory::phys_to_virt(frame.start_address());
    core::ptr::write_bytes(virt.as_mut_ptr::<u8>(), 0, 4096);
}
