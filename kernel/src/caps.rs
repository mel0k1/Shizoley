use alloc::vec::Vec;
use core::arch::asm;
use core::sync::atomic::{AtomicU64, Ordering};
use spin::Mutex;

// full capability handles on event queues (Veda-style). a handle is an
// unguessable 64-bit token; a process can only use the tokens that sit in
// ITS row of the table — a token copied over ipc by a bystander is refused
// even though its bits guess right. every eq syscall resolves the token
// before it touches the queue.
//
// rights travel through a derivation forest: eq_grant derives a child
// instance from an existing one (subset of its bits, a new random token,
// bound to the target pid), and the target learns the token when the
// granter sends the 8 bytes over ipc — the kernel never needs to be in
// that loop. revoke takes a whole subtree back.
//
// before a new edge is created, would_cycle walks the parent chain of the
// granting instance: handing the capability back to a process that already
// holds one of its ancestors would let the queue reach itself through its
// own chain — revocation loops, objects keeping themselves alive. that
// grant is refused; a chain walk past the search limit is refused too
// (Veda's "the safe answer").

pub const QPOST: u32 = 1; // eq_post into the queue
pub const QWAIT: u32 = 2; // eq_wait on the doorbell
pub const QKICK: u32 = 4; // eq_kick to release chunk credit
pub const QLISTEN: u32 = 8; // eq_listen: make it the kernel event sink
pub const QSIGNAL: u32 = 16; // eq_alert: cancel parked waiters
pub const QGRANT: u32 = 32; // may re-grant subsets of its own rights
pub const QALL: u32 = QPOST | QWAIT | QKICK | QLISTEN | QSIGNAL | QGRANT;

// how many ancestor hops would_cycle looks at before it gives up and
// refuses the transfer (mirrors Veda's CYCLE_SEARCH_LIMIT)
const CYCLE_SEARCH_LIMIT: usize = 4096;

#[derive(Clone, Copy)]
struct Instance {
    token: u64, // the unguessable handle value
    qid: u64,
    pid: u64,    // holder: syscalls only work from this process
    bits: u32,   // rights carried by this instance
    parent: u64, // token it was derived from; 0 = the owner's root handle
}

static INSTANCES: Mutex<Vec<Instance>> = Mutex::new(Vec::new());
static RNG: AtomicU64 = AtomicU64::new(0);

// xorshift64* seeded from the tsc on first use: enough to keep sibling
// processes from guessing handle values. bit 63 stays clear so a token
// can never look like the syscall error (-1) on the 64-bit boundary
fn rnd() -> u64 {
    let mut x = RNG.load(Ordering::Relaxed);
    if x == 0 {
        let (lo, hi): (u32, u32);
        unsafe {
            asm!("rdtsc", out("eax") lo, out("edx") hi, options(nostack, nomem));
        }
        x = ((hi as u64) << 32) | lo as u64 | 1;
    }
    x ^= x << 13;
    x ^= x >> 7;
    x ^= x << 17;
    x &= !(1u64 << 63);
    RNG.store(x, Ordering::Relaxed);
    x
}

// the queue's owner registers its root handle; it carries every right and
// is the only instance with parent == 0
pub fn install_root(qid: u64, owner_pid: u64) -> u64 {
    let token = rnd();
    INSTANCES.lock().push(Instance {
        token,
        qid,
        pid: owner_pid,
        bits: QALL,
        parent: 0,
    });
    token
}

// resolve a token for `pid`, demanding every bit in `need`; returns the
// qid the capability points at. need == 0 just checks existence.
pub fn lookup(token: u64, pid: u64, need: u32) -> Option<u64> {
    let inst = INSTANCES.lock();
    inst.iter()
        .find(|i| i.token == token && i.pid == pid)
        .filter(|i| i.bits & need == need)
        .map(|i| i.qid)
}

pub enum GrantErr {
    Bad,     // malformed request (bad bits, pid 0, unknown token)
    NoRight, // the granting instance lacks QGRANT or the bits exceed it
    Cycle,   // would_cycle: the chain reaches the target already
}

// derive a child instance from `token` (held by caller_pid) and bind it to
// target_pid; returns the child's token for the granter to hand over
pub fn grant(token: u64, caller_pid: u64, target_pid: u64, bits: u32) -> Result<u64, GrantErr> {
    if target_pid == 0 || bits == 0 || bits & !QALL != 0 {
        return Err(GrantErr::Bad);
    }
    let mut inst = INSTANCES.lock();
    let parent = match inst
        .iter()
        .find(|i| i.token == token && i.pid == caller_pid)
    {
        Some(p) => *p,
        None => return Err(GrantErr::Bad),
    };
    // delegation needs the right to delegate and a subset of the parent's
    // rights — nobody grants what they do not hold
    if parent.bits & QGRANT == 0 || bits & !parent.bits != 0 {
        return Err(GrantErr::NoRight);
    }
    // would_cycle: walk the parent chain; if the target already holds any
    // ancestor of this instance (the root included), the queue would reach
    // itself through its own capability chain
    let mut hop = parent;
    let mut looked = 0usize;
    loop {
        if hop.pid == target_pid {
            return Err(GrantErr::Cycle);
        }
        if hop.parent == 0 {
            break;
        }
        looked += 1;
        if looked > CYCLE_SEARCH_LIMIT {
            return Err(GrantErr::Cycle); // the safe answer
        }
        match inst
            .iter()
            .find(|i| i.token == hop.parent && i.qid == parent.qid)
        {
            Some(next) => hop = *next,
            None => break, // chain cut by a revoke/death: no cycle
        }
    }
    let child = rnd();
    inst.push(Instance {
        token: child,
        qid: parent.qid,
        pid: target_pid,
        bits,
        parent: parent.token,
    });
    Ok(child)
}

// take a derivation subtree back: every instance whose parent chain runs
// through `token` is removed. authority: the caller must hold the token
// itself or one of its ancestors (the queue owner revokes any subtree it
// spawned, a delegate revokes what it handed out further). a holder keeps
// its own instance; the root handle cannot be revoked — the queue dies
// with its owner.
pub fn revoke(token: u64, caller_pid: u64) -> Result<usize, GrantErr> {
    let mut inst = INSTANCES.lock();
    let target = match inst.iter().find(|i| i.token == token) {
        Some(t) => *t,
        None => return Err(GrantErr::Bad),
    };
    // authority: walk the parent chain looking for the caller
    let mut hop = target;
    let mut authorized = false;
    loop {
        if hop.pid == caller_pid {
            authorized = true;
            break;
        }
        if hop.parent == 0 {
            break;
        }
        match inst
            .iter()
            .find(|i| i.token == hop.parent && i.qid == target.qid)
        {
            Some(next) => hop = *next,
            None => break,
        }
    }
    if !authorized {
        return Err(GrantErr::Bad);
    }
    if target.parent == 0 && target.pid == caller_pid {
        return Err(GrantErr::Bad); // the root is the queue itself
    }
    // BFS over children: kill tokens accumulate, every instance whose
    // parent is a kill dies too (cascade through the whole subtree)
    let mut killed: Vec<u64> = alloc::vec![token];
    let mut n = 0usize;
    loop {
        let mut grew = false;
        inst.retain(|i| {
            if i.token == token && i.pid == caller_pid {
                return true; // the holder keeps its own instance
            }
            if i.token == token || killed.contains(&i.parent) {
                killed.push(i.token);
                n += 1;
                grew = true;
                return false;
            }
            true
        });
        if !grew {
            break;
        }
    }
    Ok(n)
}

// a process died: its instances mean nothing anymore (instances it granted
// onward survive — the queue owner can still revoke them explicitly)
pub fn drop_process(pid: u64) {
    INSTANCES.lock().retain(|i| i.pid != pid);
}

// a queue died: every capability pointing at it is void
pub fn drop_queue(qid: u64) {
    INSTANCES.lock().retain(|i| i.qid != qid);
}
