use alloc::sync::Arc;
use alloc::vec;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, Ordering};
use spin::Mutex;
use x86_64::instructions::interrupts;
use x86_64::structures::paging::{Page, PageTableFlags, PhysFrame, Size4KiB};
use x86_64::VirtAddr;

use crate::caps;
use crate::memory;
use crate::serial_println;
use crate::task::{Process, Task, TaskState, USER_LIMIT};

// managarm-style event queue v2: a chain of chunks in USER memory the
// kernel writes into directly; the consumer reads elements with plain
// loads (no syscall), returns chunks by zeroing their progress word and
// only enters the kernel to sleep on the doorbell (eq_wait), to release
// credit (eq_kick) or to cancel a parked wait (eq_alert).
//
// head (48 bytes at base):
//   +0  u32 magic       +4 u32 chunk_size   +8 u32 nchunks
//   +12 u32 user_notify (kernel ORs EVENT|ALERT|ERROR, user ANDs out)
//   +16 u64 prod_seq    (kernel-owned: chunks opened)
//   +24 u64 cons_seq    (consumer-owned: chunks drained and returned)
//   +32 u32 error_code  (kernel: first channel error, 0 = none)
//   +36 u32 err_count   (kernel: errors raised so far)
//   +40 u64 cons_off    (consumer-owned: bytes read in the open chunk)
// chunk i at base + 48 + i*chunk_size:
//   +0 u32 next      (producer: index of the following chunk, set at FULL)
//   +4 u32 progress  (bytes valid | FULL; consumer zeroes it on return)
//   +8 data[]
// element in data (8-aligned):
//   +0 u32 len (16+payload) +4 u32 opcode +8 u64 sender pid +16 payload
//
// v2 removes the lap/FULL ambiguity: the consumer follows the producer's
// `next` chain instead of assuming index order, so a chunk is only ever
// read while FULL, and only ever reused after the consumer zeroed it.
//
// access is capability-guarded: the creator gets an unguessable root
// handle from caps (every right, incl. QGRANT for delegation); anyone
// else only gets a derived handle through eq_grant — full capability
// semantics, see caps.rs.

pub const EQ_MAGIC: u32 = 0x5A48_5632; // "ZHv2"
pub const EQ_FULL: u32 = 0x8000_0000;
pub const EQ_HDR: u64 = 48;
pub const EQ_ELEM_HDR: usize = 16;
pub const EQ_MAX_PAYLOAD: usize = 2048;
// user_notify bits (kernel sets, consumer clears)
pub const NOTIFY_EVENT: u32 = 1;
pub const NOTIFY_ALERT: u32 = 2;
pub const NOTIFY_ERROR: u32 = 4;
// error_code values: the kernel-side channel state, read + cleared by the consumer
pub const E_DROP: u32 = 1; // kernel dropped an event (credit/lock shortage)
pub const E_CLOSED: u32 = 2; // queue closed, owner gone
pub const E_PROTO: u32 = 3; // producer protocol violation
                            // opcodes used by the kernel side
pub const OP_KERNEL_EVENT: u32 = 1; // keyboard scancode+ascii pair
pub const OP_BOOT: u32 = 2; // "queue is listening" ping
pub const OP_ZMSG: u32 = 4; // zero-copy gift: payload = handle + bytes

struct EqProd {
    seq: u64, // chunks opened
    woff: u32,
}

struct Eq {
    owner: Arc<Process>,
    base: u64,
    size: u64,
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
// the queue kernel events (keyboard) flow into, set by eq_listen
static SINK_QID: Mutex<u64> = Mutex::new(0);

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
    let _ = wr32(&eq.owner, eq.base, 12, cur | NOTIFY_EVENT);
}

// mark the channel broken: the first error code wins, the count ticks and
// the consumer learns about it through the ERROR notify bit
fn raise_error(eq: &Eq, code: u32) {
    if rd32(&eq.owner, eq.base, 32).unwrap_or(E_DROP) == 0 {
        let _ = wr32(&eq.owner, eq.base, 32, code);
    }
    let cnt = rd32(&eq.owner, eq.base, 36).unwrap_or(0);
    let _ = wr32(&eq.owner, eq.base, 36, cnt.wrapping_add(1));
    let n = rd32(&eq.owner, eq.base, 12).unwrap_or(0);
    let _ = wr32(&eq.owner, eq.base, 12, n | NOTIFY_ERROR);
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
    // one queue per memory region: refuse overlap with the caller's other
    // queues so two heads can never fight over the same bytes
    {
        let reg = EQS.lock();
        for (_, e) in reg.iter() {
            if e.owner.id != cur.proc.id {
                continue;
            }
            if base < e.base + e.size && e.base < base + size {
                serial_println!(
                    "[eq] setup refused: range {:#x}..{:#x} overlaps queue {}",
                    base,
                    base + size,
                    find_qid(&reg, e)
                );
                return -1;
            }
        }
    }
    let proc = &cur.proc;
    // doorbell + seqs + error channel + cons_off + all chunk words start at zero
    if !wr32(proc, base, 12, 0)
        || !wr64(proc, base, 16, 0)
        || !wr64(proc, base, 24, 0)
        || !wr32(proc, base, 32, 0)
        || !wr32(proc, base, 36, 0)
        || !wr64(proc, base, 40, 0)
    {
        return -1;
    }
    for i in 0..nchunks {
        let off = EQ_HDR + i as u64 * chunk_size as u64;
        if !wr32(proc, base, off, 0) || !wr32(proc, base, off + 4, 0) {
            return -1;
        }
    }

    let eq = Arc::new(Eq {
        owner: cur.proc.clone(),
        base,
        size,
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
    // the caller's root capability: unguessable, every right
    let root = caps::install_root(qid, cur.proc.id);
    serial_println!(
        "[eq] queue {} registered: pid {} base {:#x} chunk {} x {} root {:#x}",
        qid,
        cur.proc.id,
        base,
        chunk_size,
        nchunks,
        root
    );
    root as i64
}

// resolve a capability token for the caller into the qid it names; the
// denial carries the right that was missing so the serial log tells the
// whole story
fn resolve(token: u64, pid: u64, need: u32, what: &str) -> Option<u64> {
    match caps::lookup(token, pid, need) {
        Some(qid) => Some(qid),
        None => {
            serial_println!(
                "[eq] {} denied: pid {} holds no capability {:#x} (need {:#x})",
                what,
                pid,
                token,
                need
            );
            None
        }
    }
}

fn find_qid(reg: &[(u64, Arc<Eq>)], eq: &Arc<Eq>) -> u64 {
    reg.iter()
        .find(|(_, e)| Arc::ptr_eq(e, eq))
        .map(|(id, _)| *id)
        .unwrap_or(0)
}

// append one element; parks the poster when chunk credit runs out until
// the consumer drains a full chunk and kicks the queue. `handle` is the
// caller's capability token (root or derived), not a guessable id.
pub fn post(handle: u64, opcode: u32, buf: u64, len: u64) -> i64 {
    if len as usize > EQ_MAX_PAYLOAD {
        return -1;
    }
    let cur = match crate::sched::current() {
        Some(c) => c,
        None => return -1,
    };
    // capability first: the token must be the caller's and carry POST
    let qid = match resolve(handle, cur.proc.id, caps::QPOST, "post") {
        Some(q) => q,
        None => return -1,
    };
    // stage the payload in kernel memory first (the source may be reused)
    let mut payload = vec![0u8; len as usize];
    if !cur.proc.aspace.lock().read_bytes(buf, &mut payload) {
        return -1;
    }
    post_bytes(qid, opcode, &payload)
}

// kernel-side append used by post() and the zero-copy path: payload comes
// prepared, no user-memory access below this point. the capability was
// resolved by the caller — post_bytes itself is kernel-trusted.
fn post_bytes(qid: u64, opcode: u32, payload: &[u8]) -> i64 {
    let cur = match crate::sched::current() {
        Some(c) => c,
        None => return -1,
    };
    let eq = match find(qid) {
        Some(e) => e,
        None => return -1,
    };
    let elem_len = (EQ_ELEM_HDR + payload.len() + 7) & !7usize;
    if elem_len as u32 > eq.chunk_size - 8 {
        return -1;
    }
    let mut elem = vec![0u8; elem_len];
    elem[0..4].copy_from_slice(&(elem_len as u32).to_le_bytes());
    elem[4..8].copy_from_slice(&opcode.to_le_bytes());
    elem[8..16].copy_from_slice(&cur.proc.id.to_le_bytes());
    elem[EQ_ELEM_HDR..EQ_ELEM_HDR + payload.len()].copy_from_slice(payload);

    interrupts::without_interrupts(|| loop {
        if find(qid).is_none() {
            // queue was closed while we were parked
            return -1;
        }
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
        if prod.woff == 0 {
            // fresh lap over this chunk: clear the previous FULL marker
            // BEFORE writing elements, so the consumer cannot re-read the
            // old flag and double-count the whole chunk
            if !wr32(&eq.owner, eq.base, chunk - eq.base + 4, 0) {
                raise_error(&eq, E_PROTO);
                return -1;
            }
        }
        if prod.woff + elem_len as u32 <= data_cap {
            let dst = chunk + 8 + prod.woff as u64;
            if !eq.owner.aspace.lock().write_bytes(dst, &elem) {
                raise_error(&eq, E_PROTO);
                return -1;
            }
            prod.woff += elem_len as u32;
            // publish: progress tells the consumer how many bytes are valid
            if !wr32(&eq.owner, eq.base, chunk - eq.base + 4, prod.woff) {
                raise_error(&eq, E_PROTO);
                return -1;
            }
            drop(prod);
            doorbell(&eq);
            wake_drivers(&eq);
            return 0;
        }
        // element does not fit: close the chunk, chain it to the next one
        // and hand that one over; the consumer follows the chain instead of
        // assuming index order
        if prod.woff > 0 {
            let next = ((chunk_idx as u64 + 1) % eq.nchunks as u64) as u32;
            if !wr32(&eq.owner, eq.base, chunk - eq.base, next)
                || !wr32(&eq.owner, eq.base, chunk - eq.base + 4, prod.woff | EQ_FULL)
            {
                raise_error(&eq, E_PROTO);
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

// park-free post used by the kernel itself (keyboard irq, boot ping):
// drops the event on any contention or credit shortage, never blocks
pub fn post_nb(qid: u64, opcode: u32, payload: &[u8]) -> bool {
    if payload.len() > 64 {
        return false;
    }
    let Some(eq) = find(qid) else {
        return false;
    };
    let elem_len = EQ_ELEM_HDR + payload.len();
    if elem_len as u32 > eq.chunk_size - 8 {
        return false;
    }
    let mut elem = [0u8; EQ_ELEM_HDR + 64];
    elem[0..4].copy_from_slice(&(elem_len as u32).to_le_bytes());
    elem[4..8].copy_from_slice(&opcode.to_le_bytes());
    elem[8..16].copy_from_slice(&0u64.to_le_bytes()); // sender: kernel
    elem[EQ_ELEM_HDR..EQ_ELEM_HDR + payload.len()].copy_from_slice(payload);

    // try_lock everywhere: we can run on top of an interrupted syscall that
    // legitimately holds these locks, so blocking from here is forbidden
    let Some(mut prod) = eq.prod.try_lock() else {
        return false;
    };
    let Some(mut aspace) = eq.owner.aspace.try_lock() else {
        return false;
    };
    // error-channel writer for the paths below; we already hold the aspace,
    // and from an irq context this is the only legal way to record state
    let mark_err = |aspace: &mut crate::task::AddressSpace, code: u32| {
        let mut b = [0u8; 4];
        if aspace.read_bytes(eq.base + 32, &mut b) && u32::from_le_bytes(b) == 0 {
            let _ = aspace.write_bytes(eq.base + 32, &code.to_le_bytes());
        }
        let mut b = [0u8; 4];
        if aspace.read_bytes(eq.base + 36, &mut b) {
            let c = u32::from_le_bytes(b).wrapping_add(1);
            let _ = aspace.write_bytes(eq.base + 36, &c.to_le_bytes());
        }
        let mut b = [0u8; 4];
        if aspace.read_bytes(eq.base + 12, &mut b) {
            let n = u32::from_le_bytes(b) | NOTIFY_ERROR;
            let _ = aspace.write_bytes(eq.base + 12, &n.to_le_bytes());
        }
    };
    let mut b = [0u8; 8];
    if !aspace.read_bytes(eq.base + 24, &mut b) {
        return false;
    }
    let cons = u64::from_le_bytes(b);
    if prod.seq < cons {
        prod.seq = cons;
    }
    if prod.seq - cons >= eq.nchunks as u64 {
        // no credit: the event is dropped — say so on the error channel
        mark_err(&mut aspace, E_DROP);
        return false;
    }
    // close the open chunk first when the element does not fit
    let mut chunk_idx = (prod.seq % eq.nchunks as u64) as u32;
    let mut chunk = eq.base + EQ_HDR + chunk_idx as u64 * eq.chunk_size as u64;
    let data_cap = eq.chunk_size - 8;
    if prod.woff > 0 && prod.woff as usize + elem_len > data_cap as usize {
        let next = ((chunk_idx as u64 + 1) % eq.nchunks as u64) as u32;
        if !aspace.write_bytes(chunk, &next.to_le_bytes()) {
            return false;
        }
        if !aspace.write_bytes(chunk + 4, &(prod.woff | EQ_FULL).to_le_bytes()) {
            return false;
        }
        prod.seq += 1;
        prod.woff = 0;
        chunk_idx = (prod.seq % eq.nchunks as u64) as u32;
        chunk = eq.base + EQ_HDR + chunk_idx as u64 * eq.chunk_size as u64;
    }
    if prod.woff == 0 && !aspace.write_bytes(chunk + 4, &0u32.to_le_bytes()) {
        return false;
    }
    if !aspace.write_bytes(chunk + 8 + prod.woff as u64, &elem[..elem_len]) {
        return false;
    }
    prod.woff += elem_len as u32;
    if !aspace.write_bytes(chunk + 4, &prod.woff.to_le_bytes()) {
        return false;
    }
    // doorbell goes out under the same lock section
    let mut d = [0u8; 4];
    if aspace.read_bytes(eq.base + 12, &mut d) {
        let cur = u32::from_le_bytes(d) | 1;
        let _ = aspace.write_bytes(eq.base + 12, &cur.to_le_bytes());
    }
    drop(aspace);
    drop(prod);
    // never touch scheduler state with interrupts enabled: the timer could
    // spin on SCHED forever while we hold it from a syscall context
    interrupts::without_interrupts(|| wake_drivers(&eq));
    true
}

// block the current thread until the doorbell has bits; returns 1 =
// events, 2 = alerted (a parked wait was canceled), -1 = closed or bad
pub fn wait(handle: u64) -> i64 {
    let cur = match crate::sched::current() {
        Some(c) => c,
        None => return -1,
    };
    let qid = match resolve(handle, cur.proc.id, caps::QWAIT, "wait") {
        Some(q) => q,
        None => return -1,
    };
    let eq = match find(qid) {
        Some(e) => e,
        None => return -1,
    };
    interrupts::without_interrupts(|| {
        let bits = rd32(&eq.owner, eq.base, 12).unwrap_or(0);
        if bits & (NOTIFY_EVENT | NOTIFY_ERROR) != 0 {
            return 1i64;
        }
        if bits & NOTIFY_ALERT != 0 {
            // consume the alert: one wait, one cancellation
            let _ = wr32(&eq.owner, eq.base, 12, bits & !NOTIFY_ALERT);
            return 2;
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

// cancel waiters: any SIGNAL holder may ring the alert bit; every parked
// eq_wait wakes with 2 instead of blocking until real events show up
pub fn alert(handle: u64) -> i64 {
    let cur = match crate::sched::current() {
        Some(c) => c,
        None => return -1,
    };
    let qid = match resolve(handle, cur.proc.id, caps::QSIGNAL, "alert") {
        Some(q) => q,
        None => return -1,
    };
    let eq = match find(qid) {
        Some(e) => e,
        None => return -1,
    };
    interrupts::without_interrupts(|| {
        let n = rd32(&eq.owner, eq.base, 12).unwrap_or(0);
        let _ = wr32(&eq.owner, eq.base, 12, n | NOTIFY_ALERT);
        for t in core::mem::take(&mut *eq.drivers.lock()) {
            crate::sched::wake_parked(&t, 2);
        }
        0
    })
}

// called by the consumer after cons_seq++: release parked posters
pub fn kick(handle: u64) -> i64 {
    let cur = match crate::sched::current() {
        Some(c) => c,
        None => return -1,
    };
    let qid = match resolve(handle, cur.proc.id, caps::QKICK, "kick") {
        Some(q) => q,
        None => return -1,
    };
    let eq = match find(qid) {
        Some(e) => e,
        None => return -1,
    };
    interrupts::without_interrupts(|| {
        wake_producers(&eq);
        0
    })
}

// make the caller's queue the kernel event sink (keyboard events); the
// caller's capability must carry LISTEN, and a boot ping proves the pipe
pub fn listen(handle: u64) -> i64 {
    let cur = match crate::sched::current() {
        Some(c) => c,
        None => return -1,
    };
    let qid = match resolve(handle, cur.proc.id, caps::QLISTEN, "listen") {
        Some(q) => q,
        None => return -1,
    };
    let eq = match find(qid) {
        Some(e) => e,
        None => return -1,
    };
    *SINK_QID.lock() = qid;
    serial_println!(
        "[eq] kernel event sink -> queue {} owned by pid {} (listener {})",
        qid,
        eq.owner.id,
        cur.proc.id
    );
    let payload = b"boot";
    post_nb(qid, OP_BOOT, payload);
    0
}

// hand out rights: derives a child capability from the caller's instance
// (subset of its bits; delegation needs QGRANT) bound to target_pid and
// returns the child's token — the granter passes those 8 bytes over ipc.
// would_cycle refuses grants that would hand the queue back to a holder
// of an ancestor (Veda-style cycle in the derivation forest)
pub fn grant(handle: u64, target_pid: u64, bits: u32) -> i64 {
    let cur = match crate::sched::current() {
        Some(c) => c,
        None => return -1,
    };
    let Some(qid) = caps::lookup(handle, cur.proc.id, 0) else {
        serial_println!(
            "[caps] grant denied: pid {} holds no capability {:#x}",
            cur.proc.id,
            handle
        );
        return -1;
    };
    match caps::grant(handle, cur.proc.id, target_pid, bits) {
        Ok(child) => {
            serial_println!(
                "[caps] granted {:#x} on queue {} to pid {} (handle {:#x})",
                bits,
                qid,
                target_pid,
                child
            );
            child as i64
        }
        Err(caps::GrantErr::Cycle) => {
            serial_println!(
                "[caps] would_cycle refused: grant on queue {} to pid {} (chain reaches the holder)",
                qid,
                target_pid
            );
            -1
        }
        Err(caps::GrantErr::NoRight) => {
            serial_println!(
                "[caps] grant denied: pid {} lacks QGRANT or the bits on queue {}",
                cur.proc.id,
                qid
            );
            -1
        }
        Err(caps::GrantErr::Bad) => -1,
    }
}

// take a derivation subtree back: the caller's instance stays, everything
// derived from it (transitively) is removed; returns the count
pub fn revoke(handle: u64) -> i64 {
    let cur = match crate::sched::current() {
        Some(c) => c,
        None => return -1,
    };
    match caps::revoke(handle, cur.proc.id) {
        Ok(n) => {
            serial_println!(
                "[caps] revoked handle {:#x}: {} derived handles died",
                handle,
                n
            );
            n as i64
        }
        Err(_) => -1,
    }
}

// ------------------------------------------------------- zero-copy gifts
//
// big events travel as PAGES, not bytes: eq_zpost detaches the frames
// behind a page-aligned user buffer from the poster's address space and
// posts a tiny OP_ZMSG event carrying a handle; the queue owner maps the
// gift into its own space with eq_zmap (plain loads, zero copies), reads
// it and hands the frames back to the pool with eq_zdone. a gift dies
// with either side of the queue — the frames always come home.

const ZWIN_BASE: u64 = 0x4000_0000; // per-slot mapping window (owner va)
const ZSLOT_STRIDE: u64 = 4 * 1024 * 1024;
const ZSLOTS: u64 = 16;
const ZMAX_PAGES: usize = 64; // one gift <= 256 KiB

struct ZGift {
    handle: u64,
    owner: Arc<Process>, // queue owner = the receiver
    from_pid: u64,       // the poster who gave the pages away
    frames: Vec<PhysFrame<Size4KiB>>,
    bytes: u64,
    mapped_va: u64, // 0 = not mapped yet
}

static GIFTS: Mutex<Vec<ZGift>> = Mutex::new(Vec::new());
static NEXT_GIFT: AtomicU64 = AtomicU64::new(1);

// poster side: validate, detach, register, announce. `handle` is the
// caller's queue capability and must carry POST.
pub fn zpost(handle: u64, uaddr: u64, len: u64) -> i64 {
    if len == 0
        || len > (ZMAX_PAGES as u64) * 0x1000
        || uaddr & 0xFFF != 0
        || len & 0xFFF != 0
        || uaddr.checked_add(len).is_none_or(|end| end > USER_LIMIT)
    {
        return -1;
    }
    let cur = match crate::sched::current() {
        Some(c) => c,
        None => return -1,
    };
    let qid = match resolve(handle, cur.proc.id, caps::QPOST, "zpost") {
        Some(q) => q,
        None => return -1,
    };
    let eq = match find(qid) {
        Some(e) => e,
        None => return -1,
    };
    let npages = (len / 0x1000) as usize;
    // pass 1: every page must be a user-writable 4 KiB leaf and never a
    // kernel-owned frame (BIT_9) — the gift must be poster-exclusive
    let mut frames: Vec<PhysFrame<Size4KiB>> = Vec::new();
    {
        let mut aspace = cur.proc.aspace.lock();
        for i in 0..npages {
            let va = VirtAddr::new(uaddr + i as u64 * 0x1000);
            let Some((frame, flags)) = aspace.walk_leaf(va) else {
                serial_println!("[eq] zpost: unmapped page {} of pid {}", i, cur.proc.id);
                return -1;
            };
            if !flags.contains(PageTableFlags::USER_ACCESSIBLE)
                || !flags.contains(PageTableFlags::WRITABLE)
                || flags.contains(PageTableFlags::BIT_9)
            {
                serial_println!(
                    "[eq] zpost: page {} not givable (flags {:#x})",
                    i,
                    flags.bits()
                );
                return -1;
            }
            frames.push(frame);
        }
    }
    // pass 2: detach from the poster (from here on the pages are the queue's)
    {
        let mut aspace = cur.proc.aspace.lock();
        for i in 0..npages {
            let page =
                Page::<Size4KiB>::containing_address(VirtAddr::new(uaddr + i as u64 * 0x1000));
            aspace.unmap_user_page(page);
        }
    }
    let handle = NEXT_GIFT.fetch_add(1, Ordering::AcqRel);
    GIFTS.lock().push(ZGift {
        handle,
        owner: eq.owner.clone(),
        from_pid: cur.proc.id,
        frames,
        bytes: len,
        mapped_va: 0,
    });
    // announce: 16-byte payload — handle + byte count
    let mut payload = [0u8; 16];
    payload[0..8].copy_from_slice(&handle.to_le_bytes());
    payload[8..16].copy_from_slice(&len.to_le_bytes());
    if post_bytes(qid, OP_ZMSG, &payload) != 0 {
        // queue died mid-post: the gift is void, the pages go back to the
        // pool (the poster's buffer is gone — that is the transfer contract)
        zdrop_handle(handle);
        return -1;
    }
    serial_println!(
        "[eq] zcopy: pid {} gave handle {} ({} pages) to queue {}",
        cur.proc.id,
        handle,
        npages,
        qid
    );
    handle as i64
}

// receiver side: map the gift into the owner's address space, one slot
// each; the mapping is plain RW user pages — reads are direct loads
pub fn zmap(handle: u64) -> i64 {
    let cur = match crate::sched::current() {
        Some(c) => c,
        None => return -1,
    };
    let mut gifts = GIFTS.lock();
    // immutable passes first: existence, ownership, idempotency, slot usage
    let state = gifts
        .iter()
        .find(|g| g.handle == handle)
        .map(|g| (g.owner.id == cur.proc.id, g.mapped_va));
    let Some((is_owner, mapped)) = state else {
        return -1;
    };
    if !is_owner {
        return -1;
    }
    if mapped != 0 {
        return mapped as i64; // idempotent
    }
    let used: Vec<u64> = gifts
        .iter()
        .filter(|x| x.mapped_va != 0)
        .map(|x| x.mapped_va)
        .collect();
    let mut va = 0u64;
    for s in 0..ZSLOTS {
        let cand = ZWIN_BASE + s * ZSLOT_STRIDE;
        if !used.contains(&cand) {
            va = cand;
            break;
        }
    }
    if va == 0 {
        serial_println!("[eq] zmap: no free slot for handle {}", handle);
        return -1;
    }
    let Some(g) = gifts.iter_mut().find(|g| g.handle == handle) else {
        return -1;
    };

    let flags = PageTableFlags::PRESENT
        | PageTableFlags::WRITABLE
        | PageTableFlags::USER_ACCESSIBLE
        | PageTableFlags::NO_EXECUTE;
    let mut installed = 0usize;
    for (i, f) in g.frames.iter().enumerate() {
        let page = Page::<Size4KiB>::containing_address(VirtAddr::new(va + i as u64 * 0x1000));
        serial_println!(
            "[eq] zmap: page {} va {:#x} <- phys {:#x}",
            i,
            page.start_address().as_u64(),
            f.start_address().as_u64()
        );
        if g.owner
            .aspace
            .lock()
            .map_user_page(page, *f, flags)
            .is_err()
        {
            break;
        }
        installed += 1;
    }
    if installed < g.frames.len() {
        let owner = g.owner.clone();
        for i in 0..installed {
            let page = Page::<Size4KiB>::containing_address(VirtAddr::new(va + i as u64 * 0x1000));
            owner.aspace.lock().unmap_user_page(page);
        }
        return -1;
    }
    g.mapped_va = va;
    serial_println!(
        "[eq] zcopy: handle {} -> {:#x} for pid {} ({} bytes)",
        handle,
        va,
        cur.proc.id,
        g.bytes
    );
    va as i64
}

// receiver side: consumed — unmap and free the frames
pub fn zdone(handle: u64) -> i64 {
    let cur = match crate::sched::current() {
        Some(c) => c,
        None => return -1,
    };
    let mut gifts = GIFTS.lock();
    let Some(pos) = gifts.iter().position(|g| g.handle == handle) else {
        return -1;
    };
    if gifts[pos].owner.id != cur.proc.id {
        return -1;
    }
    let g = gifts.remove(pos);
    drop(gifts);
    zfree_gift(&g, "consumed");
    0
}

// unmap (if mapped) and return the frames to the pool
fn zfree_gift(g: &ZGift, why: &str) {
    if g.mapped_va != 0 {
        let mut aspace = g.owner.aspace.lock();
        for i in 0..g.frames.len() {
            let page = Page::<Size4KiB>::containing_address(VirtAddr::new(
                g.mapped_va + i as u64 * 0x1000,
            ));
            aspace.unmap_user_page(page);
        }
    }
    let n = g.frames.len();
    for f in g.frames.iter() {
        memory::dealloc_frame(*f);
    }
    serial_println!("[eq] zcopy: gift {} {} ({} pages freed)", g.handle, why, n);
}

// abort a gift that never got announced (post failure)
fn zdrop_handle(handle: u64) {
    let mut gifts = GIFTS.lock();
    if let Some(pos) = gifts.iter().position(|g| g.handle == handle) {
        let g = gifts.remove(pos);
        drop(gifts);
        zfree_gift(&g, "aborted");
    }
}

// a process died: gifts it posted are lost, gifts it owned are freed —
// either way every frame goes back to the pool, nothing leaks
fn zdrop_process(pid: u64) {
    let dead: Vec<ZGift> = {
        let mut gifts = GIFTS.lock();
        let (dead, rest): (Vec<_>, Vec<_>) = gifts
            .drain(..)
            .partition(|g| g.owner.id == pid || g.from_pid == pid);
        *gifts = rest;
        dead
    };
    for g in dead {
        zfree_gift(&g, "dropped");
    }
}

// keyboard irq hook: forward scancode+ascii to the listening console queue
pub fn kbd_event(scancode: u8, ascii: u8) {
    let qid = *SINK_QID.lock();
    if qid == 0 {
        return;
    }
    post_nb(qid, OP_KERNEL_EVENT, &[scancode, ascii]);
}

// a process died: drop every queue it owns, wake outside parkers with an
// error, and clear grants pointing at the dying queues
pub fn close_process(pid: u64) {
    let dead: Vec<(u64, Arc<Eq>)> = {
        let mut reg = EQS.lock();
        let (mine, rest): (Vec<_>, Vec<_>) = reg.drain(..).partition(|(_, e)| e.owner.id == pid);
        *reg = rest;
        mine
    };
    for (qid, eq) in dead {
        caps::drop_queue(qid);
        if qid == *SINK_QID.lock() {
            *SINK_QID.lock() = 0;
        }
        // the head is in the dying owner's memory: best-effort write so a
        // consumer polling the channel sees why events stopped
        raise_error(&eq, E_CLOSED);
        interrupts::without_interrupts(|| {
            for t in core::mem::take(&mut *eq.drivers.lock()) {
                crate::sched::wake_parked(&t, (-1i64) as u64);
            }
            for t in core::mem::take(&mut *eq.producers.lock()) {
                crate::sched::wake_parked(&t, (-1i64) as u64);
            }
        });
        serial_println!("[eq] queue {} closed (owner {} gone)", qid, pid);
    }
    // zero-copy gifts die with the process too: gifts it posted are lost,
    // gifts it owned are freed — every frame goes back to the pool
    zdrop_process(pid);
}

// remove every parked task of `pid` from queue lists; returns them so the
// killer can mark the tasks Finished
pub fn detach_process(pid: u64) -> Vec<Arc<Task>> {
    let mut out = Vec::new();
    for (_, eq) in EQS.lock().iter() {
        let mut drivers = eq.drivers.lock();
        let (mine, rest): (Vec<_>, Vec<_>) = drivers.drain(..).partition(|t| t.proc.id == pid);
        drivers.extend(rest);
        out.extend(mine);
        let mut producers = eq.producers.lock();
        let (mine, rest): (Vec<_>, Vec<_>) = producers.drain(..).partition(|t| t.proc.id == pid);
        producers.extend(rest);
        out.extend(mine);
    }
    out
}
