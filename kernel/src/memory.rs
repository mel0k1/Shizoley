use alloc::vec::Vec;
use bootloader::bootinfo::{BootInfo, MemoryMap, MemoryRegionType};
use core::sync::atomic::{AtomicU64, Ordering};
use spin::{Mutex, Once};
use x86_64::registers::control::Cr3;
use x86_64::structures::paging::page_table::PageTableEntry;
use x86_64::structures::paging::{
    FrameAllocator, Mapper, OffsetPageTable, PageTable, PageTableFlags, PhysFrame, Size4KiB,
};
use x86_64::{PhysAddr, VirtAddr};

pub static PHYS_OFFSET: Once<u64> = Once::new();
pub static KERNEL_PML4: Once<PhysFrame<Size4KiB>> = Once::new();

// bitmap frame allocator: unlike the boot bump allocator it can free, which
// process teardown needs. one bit per 4 KiB frame, up to 512 MiB of ram.
const BITMAP_WORDS: usize = 2048; // 2048 * 64 frames = 512 MiB
const FRAME_SIZE: u64 = 4096;

struct BitmapFrames {
    // frame number of bit 0
    base: u64,
    // bit i of bits[] = frame (base + i) is allocated
    bits: [u64; BITMAP_WORDS],
    // scan cursor so consecutive allocs do not rescan from zero
    cursor: usize,
}

impl BitmapFrames {
    // start with every covered frame free
    fn new(base_frame: u64) -> Self {
        BitmapFrames {
            base: base_frame,
            bits: [0u64; BITMAP_WORDS],
            cursor: 0,
        }
    }

    fn total_bits(&self) -> u64 {
        (BITMAP_WORDS * 64) as u64
    }

    fn mark_used(&mut self, frame_no: u64) {
        let Some(idx) = frame_no.checked_sub(self.base / FRAME_SIZE) else {
            return;
        };
        if idx >= self.total_bits() {
            return;
        }
        self.bits[idx as usize / 64] |= 1 << (idx % 64);
    }

    fn mark_free(&mut self, frame_no: u64) {
        let Some(idx) = frame_no.checked_sub(self.base / FRAME_SIZE) else {
            return;
        };
        if idx >= self.total_bits() {
            return;
        }
        self.bits[idx as usize / 64] &= !(1 << (idx % 64));
    }

    fn alloc(&mut self) -> Option<PhysFrame<Size4KiB>> {
        let total = self.total_bits() as usize;
        for off in 0..total {
            let i = (self.cursor + off) % total;
            if self.bits[i / 64] & (1 << (i % 64)) != 0 {
                continue;
            }
            self.bits[i / 64] |= 1 << (i % 64);
            self.cursor = (i + 1) % total;
            let addr = (self.base / FRAME_SIZE + i as u64) * FRAME_SIZE;
            return Some(PhysFrame::containing_address(PhysAddr::new(addr)));
        }
        None
    }

    // a contiguous run of `count` free frames with the first frame's
    // physical address `align`-aligned; atomic (all or nothing). used for
    // 2 mib ept leaves — a leaf maps its whole 2 mib region, so the run
    // base must carry the alignment
    fn alloc_run(&mut self, count: u64, align: u64) -> Option<Vec<PhysFrame<Size4KiB>>> {
        if count == 0 || align == 0 || !align.is_multiple_of(FRAME_SIZE) {
            return None;
        }
        let total = self.total_bits();
        let first_fno = self.base / FRAME_SIZE;
        let step = (align / FRAME_SIZE).max(1);
        // first candidate frame number that can satisfy the alignment
        let off0 = (step - (first_fno % step)) % step;
        let mut cand = off0;
        while cand < total {
            if cand + count <= total {
                let mut taken = false;
                for k in 0..count {
                    let i = (cand + k) as usize;
                    if self.bits[i / 64] & (1 << (i % 64)) != 0 {
                        taken = true;
                        break;
                    }
                }
                if !taken {
                    let mut v = Vec::new();
                    for k in 0..count {
                        let i = (cand + k) as usize;
                        self.bits[i / 64] |= 1 << (i % 64);
                        let addr = (first_fno + cand + k) * FRAME_SIZE;
                        v.push(PhysFrame::containing_address(PhysAddr::new(addr)));
                    }
                    self.cursor = ((cand + count) % total) as usize;
                    return Some(v);
                }
            }
            cand += step;
        }
        None
    }

    // returns false when the frame was never handed out (or is unknown):
    // double frees and kernel-frame frees are refused
    fn free(&mut self, frame: PhysFrame<Size4KiB>) -> bool {
        let frame_no = frame.start_address().as_u64() / FRAME_SIZE;
        let idx = match frame_no.checked_sub(self.base / FRAME_SIZE) {
            Some(i) if i < self.total_bits() => i as usize,
            _ => return false,
        };
        if self.bits[idx / 64] & (1 << (idx % 64)) == 0 {
            return false;
        }
        self.bits[idx / 64] &= !(1 << (idx % 64));
        true
    }
}

static FRAMES: Mutex<Option<BitmapFrames>> = Mutex::new(None);

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
    init_bitmap(&boot_info.memory_map, phys_offset);
    mapper
}

// program ia32_pat explicitly (linux layout): wb wt uc- uc wb wt uc- wc.
// pa2 is what map_mmio's NO_CACHE (pcd=1) lands on — uncached device
// access regardless of firmware defaults; pa7 = wc reserved for a future
// framebuffer map
pub fn init_pat() {
    use x86_64::registers::model_specific::Msr;
    unsafe { Msr::new(0x277).write(0x0107_0406_0007_0406) };
    serial_log("[pat] wb wt uc- uc wb wt uc- wc");
}

fn init_bitmap(map: &MemoryMap, phys_offset: u64) {
    // usable span covered by the bitmap
    let mut min_frame = u64::MAX;
    let mut max_frame = 0u64;
    for region in map.iter() {
        if region.region_type != MemoryRegionType::Usable {
            continue;
        }
        min_frame = min_frame.min(region.range.start_frame_number);
        max_frame = max_frame.max(region.range.end_frame_number);
    }
    if min_frame == u64::MAX {
        panic!("no usable memory regions");
    }
    let covered = BITMAP_WORDS as u64 * 64;
    if max_frame - min_frame > covered {
        serial_log("[memory] ram beyond bitmap limit stays unused");
        max_frame = min_frame + covered;
    }
    let mut bm = BitmapFrames::new(min_frame * FRAME_SIZE);
    // everything in a usable region starts free
    for region in map.iter() {
        if region.region_type != MemoryRegionType::Usable {
            continue;
        }
        let start = region.range.start_frame_number;
        let end = region.range.end_frame_number.min(max_frame);
        for f in start..end {
            bm.mark_free(f);
        }
    }
    // frames the bootloader/kernel already occupy are hidden behind boot page
    // tables: walk them and mark every referenced frame used. the physmap
    // subtree maps all of ram, so only its table frames are protected there.
    let physmap_pml4 = (phys_offset / (512 * 1024 * 1024 * 1024u64)) as usize;
    unsafe {
        let root: &PageTable =
            &*(phys_to_virt(KERNEL_PML4.get().unwrap().start_address()).as_ptr());
        bm.mark_used(KERNEL_PML4.get().unwrap().start_address().as_u64() / FRAME_SIZE);
        for i in 0..512 {
            let e = &root[i];
            if !e.flags().contains(PageTableFlags::PRESENT) {
                continue;
            }
            let depth = if i == physmap_pml4 { 3 } else { 4 };
            protect_subtree(&mut bm, e, depth);
        }
    }
    *FRAMES.lock() = Some(bm);
}

// mark the frame behind `entry`, then descend; depth counts how many table
// levels still follow (depth 0 = leaf frame, depth 3 = pml4-style entry)
unsafe fn protect_subtree(bm: &mut BitmapFrames, entry: &PageTableEntry, depth: u8) {
    if !entry.flags().contains(PageTableFlags::PRESENT) {
        return;
    }
    bm.mark_used(entry.addr().as_u64() / FRAME_SIZE);
    if depth == 0 || entry.flags().contains(PageTableFlags::HUGE_PAGE) {
        return;
    }
    let table: &PageTable = &*(phys_to_virt(entry.addr()).as_ptr());
    for i in 0..512 {
        protect_subtree(bm, &table[i], depth - 1);
    }
}

fn serial_log(s: &str) {
    crate::serial_println!("{}", s);
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

// device mmio window: upper-half pml4 entry 496, shared by every address
// space (user pml4s copy the kernel upper half verbatim), never used by
// the bootloader's physmap
const MMIO_BASE: u64 = 0xFFFF_E000_0000_0000;
static NEXT_MMIO: AtomicU64 = AtomicU64::new(MMIO_BASE);

// map `pages` frames rw, no-exec, uncached, and return a virtual address
// whose page offset matches `phys` (it may be unaligned: acpi tables sit
// anywhere). not unmapped: devices live as long as the kernel
pub fn map_mmio(phys: u64, pages: u64) -> VirtAddr {
    let off = phys % 4096;
    let base = phys - off;
    let total = (off + pages * 4096).div_ceil(4096);
    let start = NEXT_MMIO.fetch_add(total * 4096, Ordering::AcqRel);
    let mut mapper = unsafe { OffsetPageTable::new(active_pml4(phys_offset()), phys_offset()) };
    let flags = PageTableFlags::PRESENT
        | PageTableFlags::WRITABLE
        | PageTableFlags::NO_EXECUTE
        | PageTableFlags::NO_CACHE;
    for i in 0..total {
        let page = x86_64::structures::paging::Page::<Size4KiB>::containing_address(VirtAddr::new(
            start + i * 4096,
        ));
        let frame = PhysFrame::containing_address(PhysAddr::new(base + i * 4096));
        match unsafe { mapper.map_to(page, frame, flags, &mut GlobalFrames) } {
            Ok(flush) => flush.flush(),
            Err(x86_64::structures::paging::mapper::MapToError::PageAlreadyMapped(_)) => {}
            Err(e) => panic!("map_mmio {:#x}: {:?}", phys, e),
        }
    }
    VirtAddr::new(start + off)
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
    guard.as_mut()?.alloc()
}

// `count` consecutive frames whose first frame's physical address is
// `align`-aligned; atomic. falls back to per-frame allocs at the caller
// when the pool is too fragmented for the run
pub fn alloc_frames_aligned(count: u64, align: u64) -> Option<Vec<PhysFrame<Size4KiB>>> {
    let mut guard = FRAMES.lock();
    guard.as_mut()?.alloc_run(count, align)
}

// hand a frame back to the pool; returns false for unknown/already-free
// frames (kernel-owned frames are never in the free set)
pub fn dealloc_frame(frame: PhysFrame<Size4KiB>) -> bool {
    let mut guard = FRAMES.lock();
    match guard.as_mut() {
        Some(bm) => bm.free(frame),
        None => false,
    }
}

// frames currently allocated (for boot-time stats)
pub fn frames_used() -> u64 {
    let guard = FRAMES.lock();
    let Some(bm) = guard.as_ref() else {
        return 0;
    };
    bm.bits.iter().map(|w| w.count_ones() as u64).sum()
}
