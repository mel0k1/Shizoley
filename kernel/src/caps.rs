use alloc::vec::Vec;
use core::arch::asm;
use core::sync::atomic::{AtomicU64, Ordering};
use spin::Mutex;

// universe handles: one capability table for every kernel object kind
// (Veda-style). a handle is an unguessable 64-bit token; a process can
// only use the tokens that sit in ITS row of the table — a token copied
// over ipc by a bystander is refused even though its bits guess right.
// every object syscall resolves the token before it touches the object.
//
// an Instance names a typed object (kind + kernel-internal id): event
// queues, vm guests, zero-copy gifts — new kinds join by declaring their
// right mask. rights travel through a derivation forest: grant derives a
// child instance from an existing one (subset of its bits, a new random
// token, bound to the target pid), and the target learns the token when
// the granter sends the 8 bytes over ipc — the kernel never needs to be
// in that loop. revoke takes a whole subtree back.
//
// before a new edge is created, would_cycle walks the parent chain of the
// granting instance: handing the capability back to a process that already
// holds one of its ancestors would let the object reach itself through
// its own chain — revocation loops, objects keeping themselves alive.
// that grant is refused; a chain walk past the search limit is refused
// too (Veda's "the safe answer").

pub const K_EQ: u32 = 1; // event queue
pub const K_GUEST: u32 = 2; // vm guest (vmcs + ept + ram window)
pub const K_GIFT: u32 = 3; // zero-copy page gift
pub const K_MEM: u32 = 4; // memory object (kernel-owned shareable frames)

// event queue rights
pub const QPOST: u32 = 1; // eq_post into the queue
pub const QWAIT: u32 = 2; // eq_wait on the doorbell
pub const QKICK: u32 = 4; // eq_kick to release chunk credit
pub const QLISTEN: u32 = 8; // eq_listen: make it the kernel event sink
pub const QSIGNAL: u32 = 16; // eq_alert: cancel parked waiters
pub const QGRANT: u32 = 32; // may re-grant subsets of its own rights

// vm guest rights
pub const GRUN: u32 = 1; // vcpu_run
pub const GMAP: u32 = 2; // guest_map
pub const GMODE: u32 = 4; // guest_mode
pub const GPI: u32 = 8; // guest_pi
pub const GDESTROY: u32 = 16; // guest_destroy
pub const GGRANT: u32 = 32; // may re-grant guest rights
pub const GDEV: u32 = 64; // guest_device_attach/detach (iommu domain)

// zero-copy gift rights
pub const ZMAP: u32 = 1; // eq_zmap
pub const ZDONE: u32 = 2; // eq_zdone

// memory object rights
pub const MMAP_R: u32 = 1; // shm_map read-only
pub const MMAP_W: u32 = 2; // shm_map read-write
pub const MGRANT: u32 = 4; // may re-grant subsets of its own rights

// valid right bits per kind: grant refuses anything outside the mask,
// so a child can never carry nonsense bits that outlive a kind change
fn rights_mask(kind: u32) -> u32 {
    match kind {
        K_EQ => QPOST | QWAIT | QKICK | QLISTEN | QSIGNAL | QGRANT,
        K_GUEST => GRUN | GMAP | GMODE | GPI | GDESTROY | GGRANT | GDEV,
        K_GIFT => ZMAP | ZDONE,
        K_MEM => MMAP_R | MMAP_W | MGRANT,
        _ => 0,
    }
}

// how many ancestor hops would_cycle looks at before it gives up and
// refuses the transfer (mirrors Veda's CYCLE_SEARCH_LIMIT)
const CYCLE_SEARCH_LIMIT: usize = 4096;

#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Obj {
    pub kind: u32,
    pub id: u64,
}

impl Obj {
    pub fn eq(id: u64) -> Obj {
        Obj { kind: K_EQ, id }
    }
    pub fn guest(id: u64) -> Obj {
        Obj { kind: K_GUEST, id }
    }
    pub fn gift(id: u64) -> Obj {
        Obj { kind: K_GIFT, id }
    }
    pub fn mem(id: u64) -> Obj {
        Obj { kind: K_MEM, id }
    }
}

#[derive(Clone, Copy)]
struct Instance {
    token: u64,  // the unguessable handle value
    obj: Obj,    // kind + kernel-internal object id
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

// the object's owner registers its root handle; it carries every right
// of its kind and is the only instance with parent == 0
pub fn install_root(obj: Obj, owner_pid: u64) -> u64 {
    let token = rnd();
    INSTANCES.lock().push(Instance {
        token,
        obj,
        pid: owner_pid,
        bits: rights_mask(obj.kind),
        parent: 0,
    });
    token
}

// resolve a token for `pid`, demanding every bit in `need` on a matching
// kind; returns the object it points at. need == 0 just checks existence.
pub fn lookup(token: u64, pid: u64, kind: u32, need: u32) -> Option<Obj> {
    let inst = INSTANCES.lock();
    inst.iter()
        .find(|i| i.token == token && i.pid == pid && i.obj.kind == kind)
        .filter(|i| i.bits & need == need)
        .map(|i| i.obj)
}

// the object an existing instance names, for callers that only need the
// identity (logging, generic wrappers)
pub fn obj_of(token: u64, pid: u64) -> Option<Obj> {
    let inst = INSTANCES.lock();
    inst.iter()
        .find(|i| i.token == token && i.pid == pid)
        .map(|i| i.obj)
}

pub enum GrantErr {
    Bad,     // malformed request (bad bits, pid 0, unknown token)
    NoRight, // the granting instance lacks GRANT or the bits exceed it
    Cycle,   // would_cycle: the chain reaches the target already
}

// derive a child instance from `token` (held by caller_pid) and bind it
// to target_pid; returns the child's token for the granter to hand over
pub fn grant(token: u64, caller_pid: u64, target_pid: u64, bits: u32) -> Result<u64, GrantErr> {
    if target_pid == 0 || bits == 0 {
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
    if bits & !rights_mask(parent.obj.kind) != 0 {
        return Err(GrantErr::Bad);
    }
    // delegation needs the right to delegate and a subset of the parent's
    // rights — nobody grants what they do not hold
    let grant_bit = match parent.obj.kind {
        K_EQ => QGRANT,
        K_GUEST => GGRANT,
        K_MEM => MGRANT,
        _ => 0,
    };
    if grant_bit == 0 || parent.bits & grant_bit == 0 || bits & !parent.bits != 0 {
        return Err(GrantErr::NoRight);
    }
    // would_cycle: walk the parent chain; if the target already holds any
    // ancestor of this instance (the root included), the object would
    // reach itself through its own capability chain
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
            .find(|i| i.token == hop.parent && i.obj == parent.obj)
        {
            Some(next) => hop = *next,
            None => break, // chain cut by a revoke/death: no cycle
        }
    }
    let child = rnd();
    inst.push(Instance {
        token: child,
        obj: parent.obj,
        pid: target_pid,
        bits,
        parent: parent.token,
    });
    Ok(child)
}

// take a derivation subtree back: every instance whose parent chain runs
// through `token` is removed. authority: the caller must hold the token
// itself or one of its ancestors (the owner revokes any subtree it
// spawned, a delegate revokes what it handed out further). a holder keeps
// its own instance; the root handle cannot be revoked — the object dies
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
            .find(|i| i.token == hop.parent && i.obj == target.obj)
        {
            Some(next) => hop = *next,
            None => break,
        }
    }
    if !authorized {
        return Err(GrantErr::Bad);
    }
    if target.parent == 0 && target.pid == caller_pid {
        return Err(GrantErr::Bad); // the root is the object itself
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
// onward survive — the object owner can still revoke them explicitly)
pub fn drop_process(pid: u64) {
    INSTANCES.lock().retain(|i| i.pid != pid);
}

// an object died: every capability pointing at it is void
pub fn drop_obj(obj: Obj) {
    INSTANCES.lock().retain(|i| i.obj != obj);
}
