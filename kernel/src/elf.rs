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

// maps every PT_LOAD of a static ELF64 into the given address space
// with user pages (R/W/X derived from p_flags), zero-fills BSS
pub fn load(image: &[u8], aspace: &mut AddressSpace) -> Result<VirtAddr, ElfError> {
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
    if phentsize < 56 {
        return Err(ElfError::BadSegments);
    }

    for i in 0..phnum {
        let base = phoff
            .checked_add(i.checked_mul(phentsize).ok_or(ElfError::BadSegments)?)
            .ok_or(ElfError::BadSegments)?;
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
        if p_offset
            .checked_add(p_filesz)
            .ok_or(ElfError::BadSegments)?
            > image.len() as u64
        {
            return Err(ElfError::Truncated);
        }
        let mem_end = p_vaddr.checked_add(p_memsz).ok_or(ElfError::BadSegments)?;
        if mem_end > 0x0000_8000_0000_0000 {
            return Err(ElfError::BadSegments);
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

    Ok(VirtAddr::new(entry))
}

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
