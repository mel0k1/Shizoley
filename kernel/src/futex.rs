use alloc::sync::Arc;
use alloc::vec::Vec;
use spin::Mutex;
use x86_64::instructions::interrupts;

use crate::task::{Task, TaskState, USER_LIMIT};

// futex over the doorbell: threads of one process sleep on a u32 word in
// their own memory and wake each other without a server in the middle.
// the park/wake machinery is exactly the eq doorbell's (Blocked task in a
// wait bucket, switch_away_parked, wake_parked) — the bucket key is
// (pid, uaddr) since the lower half of the address space is private to
// the process and its threads share it.
//
// wait is check-and-park under irq-off: the value read and the queue
// push are one atomic section against wakers, so a wake that lands after
// the value matched cannot be lost. a mismatched value returns 1 without
// sleeping (linux -EAGAIN shape); a wake returns how many threads it
// released.

struct Bucket {
    pid: u64,
    uaddr: u64,
    waiters: Vec<Arc<Task>>,
}

static BUCKETS: Mutex<Vec<Bucket>> = Mutex::new(Vec::new());

fn key_ok(uaddr: u64) -> bool {
    uaddr >= 0x1000 && uaddr & 3 == 0 && uaddr + 4 <= USER_LIMIT
}

// futex_wait(uaddr, expected): 0 = slept and woke, 1 = value mismatch
// (no sleep), -1 = bad address or nowhere to switch to
pub fn wait(uaddr: u64, expected: u32) -> i64 {
    if !key_ok(uaddr) {
        return -1;
    }
    let cur = match crate::sched::current() {
        Some(c) => c,
        None => return -1,
    };
    interrupts::without_interrupts(|| {
        let Some(word) = crate::uar::read_u32(uaddr) else {
            return -1;
        };
        if word != expected {
            return 1;
        }
        {
            let t = cur.inner();
            t.user_rsp_saved = crate::syscall::saved_user_rsp();
            t.state = TaskState::Blocked;
        }
        let mut buckets = BUCKETS.lock();
        match buckets
            .iter_mut()
            .find(|b| b.pid == cur.proc.id && b.uaddr == uaddr)
        {
            Some(b) => b.waiters.push(cur.clone()),
            None => buckets.push(Bucket {
                pid: cur.proc.id,
                uaddr,
                waiters: alloc::vec![cur.clone()],
            }),
        }
        drop(buckets);
        if crate::sched::switch_away_parked() {
            0
        } else {
            // nothing else to run: stay runnable, the word may still match
            cur.inner().state = TaskState::Running;
            let mut buckets = BUCKETS.lock();
            if let Some(pos) = buckets
                .iter()
                .position(|b| b.pid == cur.proc.id && b.uaddr == uaddr)
            {
                buckets[pos].waiters.retain(|t| !Arc::ptr_eq(t, &cur));
                if buckets[pos].waiters.is_empty() {
                    buckets.remove(pos);
                }
            }
            -1
        }
    })
}

// futex_wake(uaddr, n): release up to n waiters of THIS process on the
// word; returns how many woke
pub fn wake(uaddr: u64, n: u64) -> i64 {
    if !key_ok(uaddr) {
        return -1;
    }
    let cur = match crate::sched::current() {
        Some(c) => c,
        None => return -1,
    };
    interrupts::without_interrupts(|| {
        let mut woke = 0i64;
        let mut buckets = BUCKETS.lock();
        if let Some(pos) = buckets
            .iter()
            .position(|b| b.pid == cur.proc.id && b.uaddr == uaddr)
        {
            while woke < n as i64 && !buckets[pos].waiters.is_empty() {
                let t = buckets[pos].waiters.remove(0);
                crate::sched::wake_parked(&t, 0);
                woke += 1;
            }
            if buckets[pos].waiters.is_empty() {
                buckets.remove(pos);
            }
        }
        woke
    })
}

// kill(14): hand the parked threads of the victim to the killer
pub fn detach_process(pid: u64) -> Vec<Arc<Task>> {
    let mut out = Vec::new();
    let mut buckets = BUCKETS.lock();
    let (mine, rest): (Vec<_>, Vec<_>) = buckets.drain(..).partition(|b| b.pid == pid);
    *buckets = rest;
    for b in mine {
        out.extend(b.waiters);
    }
    out
}

// teardown: the address space is going away, drop whatever is left
pub fn drop_process(pid: u64) {
    BUCKETS.lock().retain(|b| b.pid != pid);
}
