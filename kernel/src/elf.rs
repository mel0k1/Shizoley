use alloc::vec::Vec;
use core::fmt;
use x86_64::structures::paging::{Page, PageTableFlags, PhysFrame, Size4KiB};
use x86_64::VirtAddr;

use crate::memory;
use crate::task::AddressSpace;

const PT_LOAD: u32 = 1;
const PF_X: u32 = 1;
const PF_W: u32 = 2;

const ET_EXEC: u16 = 2;
const EM_X86_64: u16 = 62;

#[derive(Debug)]
pub enum ElfError {
    Truncated,
    Magic,
    NotElf64,
    NotLittleEndian,
    NotExec,
    NotX86_64,
    BadSegments,
    BadEntry,
    Overlap,
    OutOfMemory,
    Mapping,
}

impl fmt::Display for ElfError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let s = match self {
            ElfError::Truncated => "file truncated",
            ElfError::Magic => "bad magic",
            ElfError::NotElf64 => "not elf64",
            ElfError::NotLittleEndian => "not little endian",
            ElfError::NotExec => "not ET_EXEC",
            ElfError::NotX86_64 => "not x86_64",
            ElfError::BadSegments => "bad program headers",
            ElfError::BadEntry => "entry outside executable segment",
            ElfError::Overlap => "overlapping load segments",
            ElfError::OutOfMemory => "out of frames",
            ElfError::Mapping => "mapping failed",
        };
        f.write_str(s)
    }
}

fn u16_at(buf: &[u8], off: usize) -> Option<u16> {
    if off.checked_add(2)? > buf.len() {
        return None;
    }
    Some(u16::from_le_bytes([buf[off], buf[off + 1]]))
}

fn u32_at(buf: &[u8], off: usize) -> Option<u32> {
    if off.checked_add(4)? > buf.len() {
        return None;
    }
    Some(u32::from_le_bytes([
        buf[off],
        buf[off + 1],
        buf[off + 2],
        buf[off + 3],
    ]))
}

fn u64_at(buf: &[u8], off: usize) -> Option<u64> {
    if off.checked_add(8)? > buf.len() {
        return None;
    }
    let mut b = [0u8; 8];
    b.copy_from_slice(&buf[off..off + 8]);
    Some(u64::from_le_bytes(b))
}

// result of a successful load: entry point plus every mapped range, so the
// caller can reject images that collide with its own regions (stack, initrd)
pub struct LoadedImage {
    pub entry: VirtAddr,
    pub ranges: Vec<(u64, u64)>,
}

// maps every PT_LOAD of a static ELF64 into the given address space
// with user pages (R/W/X derived from p_flags), zero-fills BSS.
// all header fields are validated first; on any error the caller is expected
// to tear the address space down (partial mappings must not leak frames)
pub fn load(image: &[u8], aspace: &mut AddressSpace) -> Result<LoadedImage, ElfError> {
    if image.len() < 64 {
        return Err(ElfError::Truncated);
    }
    if image[0..4] != [0x7f, b'E', b'L', b'F'] {
        return Err(ElfError::Magic);
    }
    if image[4] != 2 {
        return Err(ElfError::NotElf64);
    }
    if image[5] != 1 {
        return Err(ElfError::NotLittleEndian);
    }
    let e_type = u16_at(image, 16).ok_or(ElfError::Truncated)?;
    let e_machine = u16_at(image, 18).ok_or(ElfError::Truncated)?;
    let entry = u64_at(image, 24).ok_or(ElfError::Truncated)?;
    let phoff = u64_at(image, 32).ok_or(ElfError::Truncated)?;
    let phentsize = u16_at(image, 54).ok_or(ElfError::Truncated)? as u64;
    let phnum = u16_at(image, 56).ok_or(ElfError::Truncated)? as u64;

    if e_type != ET_EXEC {
        return Err(ElfError::NotExec);
    }
    if e_machine != EM_X86_64 {
        return Err(ElfError::NotX86_64);
    }
    if phnum == 0 || phnum > 64 {
        return Err(ElfError::BadSegments);
    }
    if phentsize < 56 {
        return Err(ElfError::BadSegments);
    }
    // the whole program header table must sit inside the file
    let table_end = phoff
        .checked_add(phnum.checked_mul(phentsize).ok_or(ElfError::BadSegments)?)
        .ok_or(ElfError::BadSegments)?;
    if table_end > image.len() as u64 {
        return Err(ElfError::Truncated);
    }

    // first pass: validate every segment, collect ranges
    let mut ranges: Vec<(u64, u64)> = Vec::new();
    let mut exec_hit = false;
    for i in 0..phnum {
        let base = phoff + i * phentsize;
        let p_type = u32_at(image, base as usize).ok_or(ElfError::Truncated)?;
        if p_type != PT_LOAD {
            continue;
        }
        let p_flags = u32_at(image, base as usize + 4).ok_or(ElfError::Truncated)?;
        let p_offset = u64_at(image, base as usize + 8).ok_or(ElfError::Truncated)?;
        let p_vaddr = u64_at(image, base as usize + 16).ok_or(ElfError::Truncated)?;
        let p_filesz = u64_at(image, base as usize + 32).ok_or(ElfError::Truncated)?;
        let p_memsz = u64_at(image, base as usize + 40).ok_or(ElfError::Truncated)?;

        if p_memsz == 0 {
            continue;
        }
        if p_memsz < p_filesz {
            return Err(ElfError::BadSegments);
        }
        if p_offset
            .checked_add(p_filesz)
            .ok_or(ElfError::BadSegments)?
            > image.len() as u64
        {
            return Err(ElfError::Truncated);
        }
        // page-aligned lower half mappings only, and never the null page
        if p_vaddr < 0x1000 || p_vaddr & 0xFFF != 0 {
            return Err(ElfError::BadSegments);
        }
        let mem_end = p_vaddr.checked_add(p_memsz).ok_or(ElfError::BadSegments)?;
        if mem_end > USER_TOP {
            return Err(ElfError::BadSegments);
        }
        let range = (p_vaddr, mem_end);
        for (s, e) in &ranges {
            if range.0 < *e && *s < range.1 {
                return Err(ElfError::Overlap);
            }
        }
        if p_flags & PF_X != 0 && entry >= range.0 && entry < range.1 {
            exec_hit = true;
        }
        ranges.push(range);
    }
    if ranges.is_empty() {
        return Err(ElfError::BadSegments);
    }
    // the entry point must land inside an executable load segment
    if entry < 0x1000 || !exec_hit {
        return Err(ElfError::BadEntry);
    }

    // second pass: map (any failure leaves partial mappings — caller tears down)
    for i in 0..phnum {
        let base = phoff + i * phentsize;
        let p_type = u32_at(image, base as usize).ok_or(ElfError::Truncated)?;
        if p_type != PT_LOAD {
            continue;
        }
        let p_flags = u32_at(image, base as usize + 4).ok_or(ElfError::Truncated)?;
        let p_offset = u64_at(image, base as usize + 8).ok_or(ElfError::Truncated)?;
        let p_vaddr = u64_at(image, base as usize + 16).ok_or(ElfError::Truncated)?;
        let p_filesz = u64_at(image, base as usize + 32).ok_or(ElfError::Truncated)?;
        let p_memsz = u64_at(image, base as usize + 40).ok_or(ElfError::Truncated)?;
        if p_memsz == 0 {
            continue;
        }

        let mut flags = PageTableFlags::PRESENT | PageTableFlags::USER_ACCESSIBLE;
        if p_flags & PF_W != 0 {
            flags |= PageTableFlags::WRITABLE;
        }
        if p_flags & PF_X == 0 {
            flags |= PageTableFlags::NO_EXECUTE;
        }

        map_segment(image, p_offset, p_vaddr, p_filesz, p_memsz, flags, aspace)?;
    }

    Ok(LoadedImage {
        entry: VirtAddr::new(entry),
        ranges,
    })
}

const USER_TOP: u64 = 0x0000_8000_0000_0000;

fn map_segment(
    image: &[u8],
    p_offset: u64,
    p_vaddr: u64,
    p_filesz: u64,
    p_memsz: u64,
    flags: PageTableFlags,
    aspace: &mut AddressSpace,
) -> Result<(), ElfError> {
    let first = Page::containing_address(VirtAddr::new(p_vaddr));
    let last = Page::containing_address(VirtAddr::new(p_vaddr + p_memsz - 1));

    for page in Page::range_inclusive(first, last) {
        let frame: PhysFrame<Size4KiB> = memory::alloc_frame().ok_or(ElfError::OutOfMemory)?;
        let page_start = page.start_address().as_u64();
        let page_end = page_start + 4096;
        let fresh = aspace.map_user_page(page, frame, flags).map_err(|e| {
            let _ = e;
            ElfError::Mapping
        })?;

        let frame = aspace.translate_page(page).ok_or(ElfError::Mapping)?;
        let dst = memory::phys_to_virt(frame.start_address()).as_mut_ptr::<u8>();
        unsafe {
            if fresh {
                core::ptr::write_bytes(dst, 0, 4096);
            }
            // file-backed part
            let fs = page_start.max(p_vaddr);
            let fe = page_end.min(p_vaddr + p_filesz);
            if fe > fs {
                let src = p_offset + fs - p_vaddr;
                core::ptr::copy_nonoverlapping(
                    image.as_ptr().add(src as usize),
                    dst.add((fs - page_start) as usize),
                    (fe - fs) as usize,
                );
            }
            // bss part
            let zs = page_start.max(p_vaddr + p_filesz);
            let ze = page_end.min(p_vaddr + p_memsz);
            if ze > zs {
                core::ptr::write_bytes(dst.add((zs - page_start) as usize), 0, (ze - zs) as usize);
            }
        }
    }
    Ok(())
}
