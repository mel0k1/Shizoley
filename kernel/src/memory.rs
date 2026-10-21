use bootloader::bootinfo::{BootInfo, MemoryMap, MemoryRegionType};
use spin::{Mutex, Once};
use x86_64::registers::control::Cr3;
use x86_64::structures::paging::{FrameAllocator, OffsetPageTable, PageTable, PhysFrame, Size4KiB};
use x86_64::{PhysAddr, VirtAddr};

pub static PHYS_OFFSET: Once<u64> = Once::new();
pub static KERNEL_PML4: Once<PhysFrame<Size4KiB>> = Once::new();

static FRAMES: Mutex<Option<BootInfoFrameAllocator>> = Mutex::new(None);

pub fn init(boot_info: &'static BootInfo) -> OffsetPageTable<'static> {
    let phys_offset = boot_info.physical_memory_offset;
    PHYS_OFFSET.call_once(|| phys_offset);
    KERNEL_PML4.call_once(|| Cr3::read().0);

    let mapper = unsafe {
        OffsetPageTable::new(
            active_pml4(VirtAddr::new(phys_offset)),
            VirtAddr::new(phys_offset),
        )
    };
    let frame_allocator = unsafe { BootInfoFrameAllocator::init(&boot_info.memory_map) };
    *FRAMES.lock() = Some(frame_allocator);
    mapper
}

pub unsafe fn active_pml4(physical_memory_offset: VirtAddr) -> &'static mut PageTable {
    let (level_4_table_frame, _) = Cr3::read();
    let phys = level_4_table_frame.start_address();
    let virt = physical_memory_offset + phys.as_u64();
    unsafe { &mut *(virt.as_mut_ptr()) }
}

pub fn phys_offset() -> VirtAddr {
    VirtAddr::new(*PHYS_OFFSET.get().expect("phys offset not set"))
}

pub fn phys_to_virt(phys: PhysAddr) -> VirtAddr {
    phys_offset() + phys.as_u64()
}

// global frame pool: usable from any address space via the physmap
pub struct GlobalFrames;

unsafe impl FrameAllocator<Size4KiB> for GlobalFrames {
    fn allocate_frame(&mut self) -> Option<PhysFrame<Size4KiB>> {
        alloc_frame()
    }
}

pub fn alloc_frame() -> Option<PhysFrame<Size4KiB>> {
    let mut guard = FRAMES.lock();
    guard.as_mut()?.allocate_frame()
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
