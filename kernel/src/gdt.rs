use core::ptr::addr_of;
use spin::Lazy;
use x86_64::instructions::segmentation::{Segment, CS};
use x86_64::instructions::tables::load_tss;
use x86_64::structures::gdt::{Descriptor, GlobalDescriptorTable, SegmentSelector};
use x86_64::structures::tss::TaskStateSegment;
use x86_64::VirtAddr;

pub const DOUBLE_FAULT_IST_INDEX: u16 = 0;

// fixed layout: sysret loads cs = star[63:48]+16, ss = star[63:48]+8
pub const KERNEL_CODE: u16 = 0x08;
pub const KERNEL_DATA: u16 = 0x10;
pub const USER_DATA: u16 = 0x18;
pub const USER_CODE: u16 = 0x20;

#[repr(align(16))]
struct IstStack([u8; 16384]);
#[repr(align(16))]
struct Rsp0Stack([u8; 32768]);

static IST_STACK: IstStack = IstStack([0; 16384]);
static RSP0_STACK: Rsp0Stack = Rsp0Stack([0; 32768]);

static TSS: Lazy<TaskStateSegment> = Lazy::new(|| {
    let mut tss = TaskStateSegment::new();
    tss.interrupt_stack_table[DOUBLE_FAULT_IST_INDEX as usize] = {
        let top = unsafe { addr_of!(IST_STACK.0).cast::<u8>().add(IST_STACK.0.len()) };
        VirtAddr::from_ptr(top)
    };
    // interrupts from ring 3 land here
    tss.privilege_stack_table[0] = {
        let top = unsafe { addr_of!(RSP0_STACK.0).cast::<u8>().add(RSP0_STACK.0.len()) };
        VirtAddr::from_ptr(top)
    };
    tss
});

static GDT: Lazy<(GlobalDescriptorTable, [SegmentSelector; 5])> = Lazy::new(|| {
    let mut gdt = GlobalDescriptorTable::new();
    let kcode = gdt.append(Descriptor::kernel_code_segment());
    let kdata = gdt.append(Descriptor::kernel_data_segment());
    let udata = gdt.append(Descriptor::user_data_segment());
    let ucode = gdt.append(Descriptor::user_code_segment());
    let tss = gdt.append(Descriptor::tss_segment(&TSS));
    (gdt, [kcode, kdata, udata, ucode, tss])
});

pub fn init() {
    GDT.0.load();
    unsafe {
        CS::set_reg(GDT.1[0]);
        load_tss(GDT.1[4]);
    }
}
