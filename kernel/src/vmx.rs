use core::arch::asm;
use x86_64::registers::control::{Cr4, Cr4Flags};
use x86_64::registers::model_specific::Msr;

use crate::{memory, println, serial_println};

// hypervisor begin (Veda-style): probe VT-x, enter VMX root operation and
// report the capability MSRs; guest create/run comes on top of this later
// (Veda runs a driver-Linux VM through GUEST_CREATE/VCPU_RUN syscalls)

const IA32_VMX_BASIC: u32 = 0x480;
const IA32_VMX_PINBASED_CTLS: u32 = 0x481;
const IA32_VMX_PROCBASED_CTLS: u32 = 0x482;
const IA32_VMX_PROCBASED_CTLS2: u32 = 0x48B;
const IA32_VMX_EPT_VPID_CAP: u32 = 0x48C;

// physical address of the live VMXON region while VMX root is on
static mut VMXON_PHYS: Option<u64> = None;

pub fn init() {
    if !cpu_has_vmx() {
        serial_println!("[vmx] probe: not supported by cpu");
        println!("[vmx] probe: not supported by cpu");
        return;
    }
    match enter_root() {
        Ok(rev) => {
            // EPT lives behind secondary proc-based controls
            let (ept, ept_cap) = unsafe {
                let proc = Msr::new(IA32_VMX_PROCBASED_CTLS).read();
                let sec_allowed = (proc >> 32) & (1 << 31) != 0;
                if sec_allowed {
                    let c2 = Msr::new(IA32_VMX_PROCBASED_CTLS2).read();
                    let on = (c2 >> 32) & (1 << 1) != 0;
                    let cap = on && Msr::new(IA32_VMX_EPT_VPID_CAP).read() != 0;
                    (on, cap)
                } else {
                    (false, false)
                }
            };
            serial_println!(
                "[vmx] on: rev {:#x} ept {} ept_cap {} — hypervisor begin",
                rev,
                ept,
                ept_cap
            );
            println!("[vmx] on: rev {:#x} ept {} — hypervisor begin", rev, ept);
        }
        Err(e) => {
            serial_println!("[vmx] enter failed: {}", e);
            println!("[vmx] enter failed: {}", e);
        }
    }
}

fn cpu_features_ecx() -> u32 {
    let mut c: u32;
    unsafe {
        asm!(
            // cpuid trashes ebx; save/restore it for LLVM
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

// set CR4.VMXE, prepare the VMXON region and issue vmxon
fn enter_root() -> Result<u32, &'static str> {
    unsafe {
        let mut cr4 = Cr4::read();
        if cr4.contains(Cr4Flags::VIRTUAL_MACHINE_EXTENSIONS) {
            return Err("vmxe already set");
        }
        cr4.insert(Cr4Flags::VIRTUAL_MACHINE_EXTENSIONS);
        Cr4::write(cr4);
    }

    let basic = unsafe { Msr::new(IA32_VMX_BASIC).read() };
    let rev = basic as u32;
    let _ = unsafe { Msr::new(IA32_VMX_PINBASED_CTLS).read() }; // sanity: msr range alive

    // 4 KiB VMXON region; the instruction operand holds its physical address
    let frame = memory::alloc_frame().ok_or("no frame for vmxon region")?;
    let region_virt = memory::phys_to_virt(frame.start_address());
    unsafe {
        core::ptr::write_bytes(region_virt.as_mut_ptr::<u8>(), 0, 0x1000);
        region_virt.as_mut_ptr::<u32>().write_volatile(rev);
    }
    let region_phys = frame.start_address().as_u64();

    match vmxon(region_phys) {
        Ok(()) => {
            // our frame allocator never reuses frames, the region stays put
            unsafe { VMXON_PHYS = Some(region_phys) };
            Ok(rev)
        }
        Err(()) => {
            unsafe {
                let mut cr4 = Cr4::read();
                cr4.remove(Cr4Flags::VIRTUAL_MACHINE_EXTENSIONS);
                Cr4::write(cr4);
            }
            Err("vmxon failed (cpu or bios locked?)")
        }
    }
}

// vmxon takes a memory operand whose content is the region's physical
// address; CF=1 signals VMfail
fn vmxon(region_phys: u64) -> Result<(), ()> {
    let mut slot = region_phys;
    let cf: u8;
    unsafe {
        asm!(
            "vmxon [{p}]",
            "setc {c}",
            p = in(reg) &mut slot as *mut u64,
            c = lateout(reg_byte) cf,
            options(nostack)
        );
    }
    if cf == 0 {
        Ok(())
    } else {
        Err(())
    }
}
