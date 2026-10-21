use alloc::sync::Arc;
use alloc::vec;
use alloc::vec::Vec;
use spin::Mutex;
use x86_64::instructions::interrupts;

use crate::serial_println;
use crate::task::{Process, Task, TaskState, USER_LIMIT};

// managarm-style event queue: a ring of chunks in USER memory the kernel
// writes into directly; the consumer reads elements with plain loads (no
// syscall), releases chunk credit by bumping cons_seq and only enters the
// kernel to sleep on the doorbell (eq_wait) or to release credit (eq_kick).
//
// head (32 bytes at base):
//   +0  u32 magic       +4 u32 chunk_size   +8 u32 nchunks
//   +12 u32 user_notify (kernel ORs 1, user ANDs out)
//   +16 u64 prod_seq    (kernel-owned)
//   +24 u64 cons_seq    (consumer-owned: full chunks fully drained)
//   +32 u64 cons_off    (consumer-owned: bytes read in the open chunk)
// chunk i at base + 32 + i*chunk_size:
//   +0 u32 reserved     +4 u32 progress (bytes valid | FULL)   +8 data[]
// element in data (8-aligned):
//   +0 u32 len (16+payload) +4 u32 opcode +8 u64 sender pid +16 payload

pub const EQ_MAGIC: u32 = 0x5A48_5345;
pub const EQ_FULL: u32 = 0x8000_0000;
pub const EQ_HDR: u64 = 40;
pub const EQ_ELEM_HDR: usize = 16;
pub const EQ_MAX_PAYLOAD: usize = 2048;

struct EqProd {
    seq: u64, // chunks opened
    woff: u32,
}

struct Eq {
    owner: Arc<Process>,
    base: u64,
    chunk_size: u32,
    nchunks: u32,
    prod: Mutex<EqProd>,
    // threads parked in eq_wait, woken by the doorbell
    drivers: Mutex<Vec<Arc<Task>>>,
    // posters parked on chunk credit, woken by eq_kick
    producers: Mutex<Vec<Arc<Task>>>,
}

static EQS: Mutex<Vec<(u64, Arc<Eq>)>> = Mutex::new(Vec::new());
static NEXT_QID: Mutex<u64> = Mutex::new(1);

fn find(qid: u64) -> Option<Arc<Eq>> {
    EQS.lock()
        .iter()
        .find(|(id, _)| *id == qid)
        .map(|(_, eq)| eq.clone())
}

// owner-aspace scalar helpers; work from any active CR3 via the physmap
fn rd32(proc: &Process, base: u64, off: u64) -> Option<u32> {
    let mut b = [0u8; 4];
    if !proc.aspace.lock().read_bytes(base + off, &mut b) {
        return None;
    }
    Some(u32::from_le_bytes(b))
}

fn rd64(proc: &Process, base: u64, off: u64) -> Option<u64> {
    let mut b = [0u8; 8];
    if !proc.aspace.lock().read_bytes(base + off, &mut b) {
        return None;
    }
    Some(u64::from_le_bytes(b))
}

fn wr32(proc: &Process, base: u64, off: u64, v: u32) -> bool {
    proc.aspace.lock().write_bytes(base + off, &v.to_le_bytes())
}

fn wr64(proc: &Process, base: u64, off: u64, v: u64) -> bool {
    proc.aspace.lock().write_bytes(base + off, &v.to_le_bytes())
}

fn doorbell(eq: &Eq) {
    let cur = rd32(&eq.owner, eq.base, 12).unwrap_or(0);
    let _ = wr32(&eq.owner, eq.base, 12, cur | 1);
}

fn wake_drivers(eq: &Eq) {
    let list: Vec<Arc<Task>> = core::mem::take(&mut *eq.drivers.lock());
    for t in list {
        crate::sched::wake_parked(&t, 1);
    }
}

fn wake_producers(eq: &Eq) {
    let list: Vec<Arc<Task>> = core::mem::take(&mut *eq.producers.lock());
    for t in list {
        crate::sched::wake_parked(&t, 1);
    }
}

// register a fresh queue owned by the caller; the user pre-zeroes the
// memory and fills magic/chunk_size/nchunks, the kernel zeroes the rest
pub fn setup(base: u64, size: u64) -> i64 {
    if base < 0x1000 || base.checked_add(size).is_none_or(|end| end > USER_LIMIT) {
        return -1;
    }
    let cur = match crate::sched::current() {
        Some(c) => c,
        None => return -1,
    };
    let mut rd = [0u8; 12];
    if !cur.proc.aspace.lock().read_bytes(base, &mut rd) {
        return -1;
    }
    let magic = u32::from_le_bytes([rd[0], rd[1], rd[2], rd[3]]);
    let chunk_size = u32::from_le_bytes([rd[4], rd[5], rd[6], rd[7]]);
    let nchunks = u32::from_le_bytes([rd[8], rd[9], rd[10], rd[11]]);
    if magic != EQ_MAGIC || nchunks == 0 || nchunks > 64 || chunk_size < 64 || chunk_size % 8 != 0 {
        return -1;
    }
    if size < EQ_HDR + nchunks as u64 * chunk_size as u64 {
        return -1;
    }
    let proc = &cur.proc;
    // doorbell + all chunk progress words start at zero
    if !wr32(proc, base, 12, 0) {
        return -1;
    }
    for i in 0..nchunks {
        let off = EQ_HDR + i as u64 * chunk_size as u64 + 4;
        if !wr32(proc, base, off, 0) {
            return -1;
        }
    }
    if !wr64(proc, base, 16, 0) || !wr64(proc, base, 24, 0) || !wr64(proc, base, 32, 0) {
        return -1;
    }

    let eq = Arc::new(Eq {
        owner: cur.proc.clone(),
        base,
        chunk_size,
        nchunks,
        prod: Mutex::new(EqProd { seq: 0, woff: 0 }),
        drivers: Mutex::new(Vec::new()),
        producers: Mutex::new(Vec::new()),
    });
    let mut reg = EQS.lock();
    let mut nid = NEXT_QID.lock();
    let qid = *nid;
    *nid += 1;
    drop(nid);
    reg.push((qid, eq));
    serial_println!(
        "[eq] queue {} registered: pid {} base {:#x} chunk {} x {}",
        qid,
        cur.proc.id,
        base,
        chunk_size,
        nchunks
    );
    qid as i64
}

// append one element; parks the poster when chunk credit runs out until
// the consumer drains a full chunk and kicks the queue
pub fn post(qid: u64, opcode: u32, buf: u64, len: u64) -> i64 {
    if len as usize > EQ_MAX_PAYLOAD {
        return -1;
    }
    let cur = match crate::sched::current() {
        Some(c) => c,
        None => return -1,
    };
    let eq = match find(qid) {
        Some(e) => e,
        None => return -1,
    };
    // stage the payload in kernel memory first (the source may be reused)
    let mut payload = vec![0u8; len as usize];
    if !cur.proc.aspace.lock().read_bytes(buf, &mut payload) {
        return -1;
    }
    let elem_len = (EQ_ELEM_HDR + payload.len() + 7) & !7usize;
    if elem_len as u32 > eq.chunk_size - 8 {
        return -1;
    }
    let mut elem = vec![0u8; elem_len];
    elem[0..4].copy_from_slice(&(elem_len as u32).to_le_bytes());
    elem[4..8].copy_from_slice(&opcode.to_le_bytes());
    elem[8..16].copy_from_slice(&cur.proc.id.to_le_bytes());
    elem[EQ_ELEM_HDR..EQ_ELEM_HDR + payload.len()].copy_from_slice(&payload);

    interrupts::without_interrupts(|| loop {
        // credit: the chunk being filled must not have lapped the consumer
        let cons = rd64(&eq.owner, eq.base, 24).unwrap_or(0);
        let mut prod = eq.prod.lock();
        if prod.seq < cons {
            serial_println!(
                "[eq] seq/cons skew: seq {} cons {} — clamping",
                prod.seq,
                cons
            );
            prod.seq = cons;
        }
        if prod.seq - cons >= eq.nchunks as u64 {
            drop(prod);
            // no credit: register and park; eq_kick resumes us; the syscall
            // trampoline state must survive the switch, same as recv
            {
                let t = cur.inner();
                t.user_rsp_saved = crate::syscall::saved_user_rsp();
                t.state = TaskState::Blocked;
            }
            eq.producers.lock().push(cur.clone());
            if !crate::sched::switch_away_parked() {
                // nobody to switch to: stay runnable, report the stall
                cur.inner().state = TaskState::Running;
                eq.producers.lock().retain(|t| !Arc::ptr_eq(t, &cur));
                return -1;
            }
            continue;
        }
        let chunk_idx = (prod.seq % eq.nchunks as u64) as u32;
        let chunk = eq.base + EQ_HDR + chunk_idx as u64 * eq.chunk_size as u64;
        let data_cap = eq.chunk_size - 8;
        if prod.woff + elem_len as u32 <= data_cap {
            let dst = chunk + 8 + prod.woff as u64;
            if !eq.owner.aspace.lock().write_bytes(dst, &elem) {
                return -1;
            }
            prod.woff += elem_len as u32;
            // publish: progress tells the consumer how many bytes are valid
            if !wr32(&eq.owner, eq.base, chunk - eq.base + 4, prod.woff) {
                return -1;
            }
            drop(prod);
            doorbell(&eq);
            wake_drivers(&eq);
            return 0;
        }
        // element does not fit: close the chunk and hand over the next one
        if prod.woff > 0 {
            if !wr32(&eq.owner, eq.base, chunk - eq.base + 4, prod.woff | EQ_FULL) {
                return -1;
            }
            prod.seq += 1;
            prod.woff = 0;
            drop(prod);
            doorbell(&eq);
            wake_drivers(&eq);
            continue;
        }
        return -1; // element alone exceeds chunk data (already validated)
    })
}

// block the current thread until the doorbell has bits; returns 1
pub fn wait(qid: u64) -> i64 {
    let cur = match crate::sched::current() {
        Some(c) => c,
        None => return -1,
    };
    let eq = match find(qid) {
        Some(e) => e,
        None => return -1,
    };
    if eq.owner.id != cur.proc.id {
        return -1;
    }
    interrupts::without_interrupts(|| {
        let n = rd32(&eq.owner, eq.base, 12).unwrap_or(0);
        if n != 0 {
            return 1i64;
        }
        {
            let t = cur.inner();
            t.user_rsp_saved = crate::syscall::saved_user_rsp();
            t.state = TaskState::Blocked;
        }
        eq.drivers.lock().push(cur.clone());
        if crate::sched::switch_away_parked() {
            cur.inner().wake_ret as i64
        } else {
            // nothing to switch to: stay runnable
            cur.inner().state = TaskState::Running;
            eq.drivers.lock().retain(|t| !Arc::ptr_eq(t, &cur));
            -1
        }
    })
}

// called by the consumer after cons_seq++: release parked posters
pub fn kick(qid: u64) -> i64 {
    let eq = match find(qid) {
        Some(e) => e,
        None => return -1,
    };
    interrupts::without_interrupts(|| {
        wake_producers(&eq);
        0
    })
}
