use bootloader::bootinfo::{BootInfo, MemoryMap, MemoryRegionType};
use x86_64::structures::paging::{
    FrameAllocator, OffsetPageTable, PageTable, PageTableFlags, PhysFrame, Size4KiB,
};
use x86_64::{PhysAddr, VirtAddr};

pub fn init(boot_info: &'static BootInfo) -> (OffsetPageTable<'static>, BootInfoFrameAllocator) {
    let phys_offset = VirtAddr::new(boot_info.physical_memory_offset);
    let mapper = unsafe { OffsetPageTable::new(active_pml4(phys_offset), phys_offset) };
    let frame_allocator = unsafe { BootInfoFrameAllocator::init(&boot_info.memory_map) };
    (mapper, frame_allocator)
}

pub unsafe fn active_pml4(physical_memory_offset: VirtAddr) -> &'static mut PageTable {
    use x86_64::registers::control::Cr3;

    let (level_4_table_frame, _) = Cr3::read();
    let phys = level_4_table_frame.start_address();
    let virt = physical_memory_offset + phys.as_u64();
    unsafe { &mut *(virt.as_mut_ptr()) }
}

// every ancestor of [start, end) must carry the user bit or ring 3 faults
pub unsafe fn mark_user_range(phys_offset: VirtAddr, start: u64, end: u64) {
    let pml4 = active_pml4(phys_offset);
    let mut base = start & !0x1F_FFFF;
    while base < end {
        let pml4e = &mut pml4[((base >> 39) & 0x1FF) as usize];
        pml4e.set_flags(pml4e.flags() | PageTableFlags::USER_ACCESSIBLE);
        let pdpt = &mut *((phys_offset + pml4e.addr().as_u64()).as_mut_ptr::<PageTable>());
        let pdpte = &mut pdpt[((base >> 30) & 0x1FF) as usize];
        pdpte.set_flags(pdpte.flags() | PageTableFlags::USER_ACCESSIBLE);
        let pd = &mut *((phys_offset + pdpte.addr().as_u64()).as_mut_ptr::<PageTable>());
        let pde = &mut pd[((base >> 21) & 0x1FF) as usize];
        pde.set_flags(pde.flags() | PageTableFlags::USER_ACCESSIBLE);
        base += 0x20_0000;
    }
}

pub struct BootInfoFrameAllocator {
    memory_map: &'static MemoryMap,
    next: usize,
}

impl BootInfoFrameAllocator {
    unsafe fn init(memory_map: &'static MemoryMap) -> Self {
        BootInfoFrameAllocator {
            memory_map,
            next: 0,
        }
    }
}

unsafe impl FrameAllocator<Size4KiB> for BootInfoFrameAllocator {
    fn allocate_frame(&mut self) -> Option<PhysFrame<Size4KiB>> {
        let mut remaining = self.next;
        for region in self.memory_map.iter() {
            if region.region_type != MemoryRegionType::Usable {
                continue;
            }
            let count = region.range.end_frame_number - region.range.start_frame_number;
            if remaining < count as usize {
                let frame_number = region.range.start_frame_number + remaining as u64;
                self.next += 1;
                return Some(PhysFrame::containing_address(PhysAddr::new(
                    frame_number * 4096,
                )));
            }
            remaining -= count as usize;
        }
        None
    }
}
