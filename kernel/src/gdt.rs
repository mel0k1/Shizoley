use core::ptr::addr_of;
use spin::Lazy;
use x86_64::instructions::segmentation::{Segment, CS};
use x86_64::instructions::tables::load_tss;
use x86_64::structures::gdt::{Descriptor, GlobalDescriptorTable, SegmentSelector};
use x86_64::structures::tss::TaskStateSegment;
use x86_64::VirtAddr;

pub const DOUBLE_FAULT_IST_INDEX: u16 = 0;

static TSS: Lazy<TaskStateSegment> = Lazy::new(|| {
    static STACK: [u8; 16384] = [0; 16384];
    let mut tss = TaskStateSegment::new();
    tss.interrupt_stack_table[DOUBLE_FAULT_IST_INDEX as usize] = {
        let top = unsafe { addr_of!(STACK).cast::<u8>().add(STACK.len()) };
        VirtAddr::from_ptr(top)
    };
    tss
});

static GDT: Lazy<(GlobalDescriptorTable, [SegmentSelector; 2])> = Lazy::new(|| {
    let mut gdt = GlobalDescriptorTable::new();
    let code = gdt.append(Descriptor::kernel_code_segment());
    let tss = gdt.append(Descriptor::tss_segment(&TSS));
    (gdt, [code, tss])
});

pub fn init() {
    GDT.0.load();
    unsafe {
        CS::set_reg(GDT.1[0]);
        load_tss(GDT.1[1]);
    }
}
