use alloc::sync::{Arc, Weak};
use alloc::vec::Vec;
use spin::Mutex;
use x86_64::instructions::interrupts;

use crate::sched;
use crate::task::{Process, Task, TaskState};

// simple mailbox ipc: send enqueues or hands off directly to a parked
// receiver, recv dequeues or parks the thread until a sender wakes it
// (a managarm-style async queue into user memory is planned on top of this)

pub const MAX_MSG: usize = 4096;
pub const MAILBOX_CAP: usize = 32;

pub struct Message {
    pub from: u64,
    pub data: Vec<u8>,
}

static PROCESSES: Mutex<Vec<Weak<Process>>> = Mutex::new(Vec::new());

pub fn register(proc: &Arc<Process>) {
    let mut list = PROCESSES.lock();
    list.retain(|w| w.upgrade().is_some());
    list.push(Arc::downgrade(proc));
}

// drop a dead process from the pid registry so sends to it fail cleanly
pub fn unregister(pid: u64) {
    let mut list = PROCESSES.lock();
    list.retain(|w| match w.upgrade() {
        Some(p) => p.id != pid,
        None => false,
    });
}

pub fn find(pid: u64) -> Option<Arc<Process>> {
    let mut list = PROCESSES.lock();
    list.retain(|w| w.upgrade().is_some());
    list.iter().find_map(|w| {
        let p = w.upgrade()?;
        (p.id == pid).then_some(p)
    })
}

// kernel-side send: copy the payload out of the sender, then either hand it
// straight to a parked receiver or drop it into the target mailbox
pub fn send(sender: &Arc<Task>, dest_pid: u64, buf: u64, len: u64) -> i64 {
    // edge cases: empty/oversized payloads, kernel pid, self-send is allowed
    if len == 0 || len as usize > MAX_MSG {
        return -1;
    }
    if dest_pid == 0 {
        return -1; // the kernel process does not take mailbox messages
    }
    let mut data = alloc::vec![0u8; len as usize];
    {
        let mut aspace = sender.proc.aspace.lock();
        if !aspace.read_bytes(buf, &mut data) {
            return -1;
        }
    }
    let from = sender.proc.id;
    let Some(dest) = find(dest_pid) else {
        return -1; // unknown or dead pid
    };

    interrupts::without_interrupts(|| {
        // direct handoff: take the first waiter matching the source filter
        let waiter = {
            let mut waiters = dest.waiters.lock();
            let pos = waiters.iter().position(|t| {
                matches!(
                    t.inner().blocked,
                    Some(b) if b.src == 0 || b.src == from
                )
            });
            pos.map(|i| waiters.remove(i))
        };
        if let Some(waiter) = waiter {
            let (wbuf, wmax) = {
                let t = waiter.inner();
                let b = t.blocked.expect("parked waiter without recv record");
                (b.buf, b.max as usize)
            };
            if data.len() <= wmax && waiter.proc.aspace.lock().write_bytes(wbuf, &data) {
                // resume the receiver: its recv syscall returns the sender pid
                let t = waiter.inner();
                t.wake_ret = from;
                t.state = TaskState::Ready;
                t.blocked = None;
                sched::push_ready(waiter.clone());
                return 0;
            }
            // buffer too small or unmapped: put the waiter back, fail the send
            dest.waiters.lock().push(waiter);
            return -1;
        }

        let mut mb = dest.mailbox.lock();
        if mb.len() >= MAILBOX_CAP {
            return -1;
        }
        mb.push_back(Message { from, data });
        0
    })
}

// result of a non-blocking receive attempt
pub enum Recv {
    // no matching message: the caller may park
    Empty,
    // message copied out, value = sender pid
    Delivered(u64),
    // a matching message exists but cannot be delivered (buffer too small
    // or not writable): the caller must fail its syscall, not park forever
    Bad,
}

// non-blocking half of recv; called with interrupts off from recv_or_block
pub fn try_recv(receiver: &Arc<Task>, src: u64, buf: u64, max: u64) -> Recv {
    let proc = &receiver.proc;
    let mut mb = proc.mailbox.lock();
    let Some(idx) = mb
        .iter()
        .position(|m| (src == 0 || m.from == src) && m.data.len() <= max as usize)
    else {
        // a matching message that cannot fit the buffer is an error: the
        // caller retries with a bigger one instead of parking on it forever
        let stuck = mb.iter().any(|m| src == 0 || m.from == src);
        return if stuck { Recv::Bad } else { Recv::Empty };
    };
    let msg = mb.remove(idx).expect("position vanished");
    if !proc.aspace.lock().write_bytes(buf, &msg.data) {
        mb.push_front(msg);
        return Recv::Bad;
    }
    Recv::Delivered(msg.from)
}
