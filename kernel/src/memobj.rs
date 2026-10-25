use alloc::sync::Arc;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, Ordering};
use spin::Mutex;
use x86_64::structures::paging::{Page, PageTableFlags, PhysFrame, Size4KiB};
use x86_64::VirtAddr;

use crate::caps;
use crate::memory;
use crate::serial_println;
use crate::task::{Process, USER_LIMIT};

// managarm-style memory objects: the missing half of kernel->process data
// transfer. a memory object is a fixed set of frames the KERNEL owns; the
// creator maps it rw, derives an MMAP_R child over ipc and the peer maps
// the SAME frames read-only — no bytes move anywhere. this closes the
// managarm set: copies (uar), one-shot gifts (eq zcopy), persistent
// shareable objects (here).
//
// leaves carry BIT_9 like initrd mappings, so a holder's teardown can
// never free frames it does not own; close_process unmaps everything
// anyway before the reaper walks the tables. an object dies with its
// owner: every holder's mapping comes off, frames return to the pool,
// capabilities on the object go void.

const MAX_PAGES: usize = 256;

#[derive(Clone, Copy)]
struct MemMap {
    pid: u64,
    va: u64,
    npages: usize,
}

struct MemObj {
    mid: u64,
    owner: Arc<Process>,
    frames: Vec<PhysFrame<Size4KiB>>,
    // one mapping per holder process; a second shm_map from the same pid
    // is refused (map somewhere else is a re-share problem, not a need)
    maps: Mutex<Vec<MemMap>>,
}

static MEMS: Mutex<Vec<Arc<MemObj>>> = Mutex::new(Vec::new());
static NEXT_MID: AtomicU64 = AtomicU64::new(1);

fn find(mid: u64) -> Option<Arc<MemObj>> {
    MEMS.lock().iter().find(|o| o.mid == mid).cloned()
}

// shm_create(npages) -> root capability token; the caller's process owns
// the object for its whole life (the owner side of the data transfer)
pub fn create(npages: u64) -> i64 {
    if npages == 0 || npages as usize > MAX_PAGES {
        return -1;
    }
    let cur = match crate::sched::current() {
        Some(c) => c,
        None => return -1,
    };
    let mut frames: Vec<PhysFrame<Size4KiB>> = Vec::new();
    for _ in 0..npages {
        match memory::alloc_frame() {
            Some(f) => frames.push(f),
            None => {
                for f in frames {
                    memory::dealloc_frame(f);
                }
                return -1;
            }
        }
    }
    let mid = NEXT_MID.fetch_add(1, Ordering::AcqRel);
    let obj = Arc::new(MemObj {
        mid,
        owner: cur.proc.clone(),
        frames,
        maps: Mutex::new(Vec::new()),
    });
    MEMS.lock().push(obj.clone());
    let root = caps::install_root(caps::Obj::mem(mid), cur.proc.id);
    serial_println!(
        "[shm] mem {} created: {} pages for pid {} (root {:#x})",
        mid,
        npages,
        cur.proc.id,
        root
    );
    root as i64
}

// shm_map(handle, va) -> 0 | -1; map the object's frames into the caller's
// space. the caller's rights decide rw/ro — the kernel is the only writer
// of the page tables, so a ro holder physically cannot gain write access.
pub fn map(handle: u64, va: u64) -> i64 {
    let cur = match crate::sched::current() {
        Some(c) => c,
        None => return -1,
    };
    let (obj_id, rw) = match caps::lookup(handle, cur.proc.id, caps::K_MEM, caps::MMAP_W) {
        Some(o) => (o.id, true),
        None => match caps::lookup(handle, cur.proc.id, caps::K_MEM, caps::MMAP_R) {
            Some(o) => (o.id, false),
            None => {
                serial_println!(
                    "[shm] map denied: pid {} holds no capability {:#x} (need MMAP)",
                    cur.proc.id,
                    handle
                );
                return -1;
            }
        },
    };
    let Some(obj) = find(obj_id) else {
        return -1;
    };
    let npages = obj.frames.len();
    let bytes = npages as u64 * 0x1000;
    if va & 0xFFF != 0 || va < 0x1000 || va.checked_add(bytes).is_none_or(|end| end > USER_LIMIT) {
        return -1;
    }
    {
        let maps = obj.maps.lock();
        if maps.iter().any(|m| m.pid == cur.proc.id) {
            serial_println!("[shm] pid {} already maps mem {}", cur.proc.id, obj_id);
            return -1;
        }
    }
    let mut flags = PageTableFlags::PRESENT
        | PageTableFlags::USER_ACCESSIBLE
        | PageTableFlags::NO_EXECUTE
        | PageTableFlags::BIT_9;
    if rw {
        flags |= PageTableFlags::WRITABLE;
    }
    let mut installed = 0usize;
    {
        let mut aspace = cur.proc.aspace.lock();
        for (i, f) in obj.frames.iter().enumerate() {
            let page = Page::<Size4KiB>::containing_address(VirtAddr::new(va + i as u64 * 0x1000));
            match aspace.map_user_page(page, *f, flags) {
                Ok(true) => installed += 1,
                // already-mapped va (something lives there): refuse, never
                // alias foreign frames under a shm mapping
                Ok(false) => break,
                Err(_) => break,
            }
        }
    }
    if installed < npages {
        let mut aspace = cur.proc.aspace.lock();
        for i in 0..installed {
            let page = Page::<Size4KiB>::containing_address(VirtAddr::new(va + i as u64 * 0x1000));
            aspace.unmap_user_page(page);
        }
        serial_println!(
            "[shm] map failed: collision at page {} of mem {}",
            installed,
            obj_id
        );
        return -1;
    }
    obj.maps.lock().push(MemMap {
        pid: cur.proc.id,
        va,
        npages,
    });
    // the phys of frame 0 is the zero-copy proof: two processes mapping the
    // same object print the same frame number, no bytes ever moved
    serial_println!(
        "[shm] pid {} maps mem {} at {:#x} phys {:#x} ({} pages, {})",
        cur.proc.id,
        obj_id,
        va,
        obj.frames[0].start_address().as_u64(),
        npages,
        if rw { "rw" } else { "ro" }
    );
    0
}

// shm_unmap(handle) -> 0 | -1; take this process's mapping down; the
// frames stay with the object (unmap never frees, ownership is the object's)
pub fn unmap(handle: u64) -> i64 {
    let cur = match crate::sched::current() {
        Some(c) => c,
        None => return -1,
    };
    let obj_id = match caps::lookup(handle, cur.proc.id, caps::K_MEM, 0) {
        Some(o) => o.id,
        None => return -1,
    };
    let Some(obj) = find(obj_id) else {
        return -1;
    };
    let m = {
        let mut maps = obj.maps.lock();
        match maps.iter().position(|x| x.pid == cur.proc.id) {
            Some(pos) => maps.remove(pos),
            None => return -1,
        }
    };
    let mut aspace = cur.proc.aspace.lock();
    for i in 0..m.npages {
        let page = Page::<Size4KiB>::containing_address(VirtAddr::new(m.va + i as u64 * 0x1000));
        aspace.unmap_user_page(page);
    }
    serial_println!(
        "[shm] pid {} unmapped mem {} ({:#x}, {} pages)",
        cur.proc.id,
        obj_id,
        m.va,
        m.npages
    );
    0
}

// a process died: first its own mappings on live objects leave its tables
// (teardown would free frames it does not own), then every object it owns
// is dismantled — holders unmapped, frames freed, capabilities void
pub fn close_process(proc: &Arc<Process>) {
    let pid = proc.id;
    for obj in MEMS.lock().iter() {
        let mine: Vec<MemMap> = {
            let mut maps = obj.maps.lock();
            let mine: Vec<MemMap> = maps.iter().filter(|m| m.pid == pid).cloned().collect();
            maps.retain(|m| m.pid != pid);
            mine
        };
        for m in mine {
            let mut aspace = proc.aspace.lock();
            for i in 0..m.npages {
                let page =
                    Page::<Size4KiB>::containing_address(VirtAddr::new(m.va + i as u64 * 0x1000));
                aspace.unmap_user_page(page);
            }
        }
    }
    let dead: Vec<Arc<MemObj>> = {
        let mut mems = MEMS.lock();
        let (dead, rest): (Vec<_>, Vec<_>) = mems.drain(..).partition(|o| o.owner.id == pid);
        *mems = rest;
        dead
    };
    for obj in dead {
        let holders: Vec<MemMap> = core::mem::take(&mut *obj.maps.lock());
        for m in holders {
            if m.pid == pid {
                continue; // the dying tables are going away anyway
            }
            if let Some(h) = crate::ipc::find(m.pid) {
                let mut aspace = h.aspace.lock();
                for i in 0..m.npages {
                    let page = Page::<Size4KiB>::containing_address(VirtAddr::new(
                        m.va + i as u64 * 0x1000,
                    ));
                    aspace.unmap_user_page(page);
                }
                serial_println!(
                    "[shm] mem {} unmapped from pid {} (owner gone)",
                    obj.mid,
                    m.pid
                );
            }
        }
        let n = obj.frames.len();
        for f in obj.frames.iter() {
            memory::dealloc_frame(*f);
        }
        caps::drop_obj(caps::Obj::mem(obj.mid));
        serial_println!("[shm] mem {} freed: {} frames returned", obj.mid, n);
    }
}
