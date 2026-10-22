use alloc::vec::Vec;
use spin::Mutex;

// capability rights on event-queue ids: the creator owns a queue and holds
// every right implicitly; other processes hold only what an owner granted
// them via eq_grant(19). every eq syscall checks before it touches anything.

pub const QPOST: u32 = 1; // eq_post into the queue
pub const QWAIT: u32 = 2; // eq_wait on the doorbell
pub const QKICK: u32 = 4; // eq_kick to release chunk credit
pub const QLISTEN: u32 = 8; // eq_listen: make it the kernel event sink
pub const QSIGNAL: u32 = 16; // eq_alert: cancel parked waiters

#[derive(Clone, Copy)]
struct Grant {
    pid: u64,
    qid: u64,
    bits: u32,
}

static GRANTS: Mutex<Vec<Grant>> = Mutex::new(Vec::new());

// does `pid` hold `bit` on `qid`? the owner always does
pub fn check(qid: u64, pid: u64, owner_pid: u64, bit: u32) -> bool {
    if pid == owner_pid {
        return true;
    }
    let grants = GRANTS.lock();
    grants
        .iter()
        .any(|g| g.qid == qid && g.pid == pid && g.bits & bit != 0)
}

// grant rights; returns false if the pid already has a grant entry
pub fn grant(qid: u64, pid: u64, bits: u32) -> bool {
    if bits & !(QPOST | QWAIT | QKICK | QLISTEN | QSIGNAL) != 0 || bits == 0 {
        return false;
    }
    let mut grants = GRANTS.lock();
    if grants.iter().any(|g| g.qid == qid && g.pid == pid) {
        return false;
    }
    grants.push(Grant { pid, qid, bits });
    true
}

pub fn revoke(qid: u64, pid: u64) -> bool {
    let mut grants = GRANTS.lock();
    let before = grants.len();
    grants.retain(|g| !(g.qid == qid && g.pid == pid));
    grants.len() != before
}

// a process died: its grants mean nothing anymore
pub fn drop_process(pid: u64) {
    GRANTS.lock().retain(|g| g.pid != pid);
}

// a queue died: grants pointing at it are void
pub fn drop_queue(qid: u64) {
    GRANTS.lock().retain(|g| g.qid != qid);
}
