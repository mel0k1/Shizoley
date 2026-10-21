use alloc::boxed::Box;
use alloc::collections::VecDeque;
use alloc::sync::Arc;
use alloc::vec::Vec;
use spin::{Mutex, Once};
use x86_64::instructions::interrupts;
use x86_64::VirtAddr;

use crate::serial_println;
use crate::task::{AddressSpace, BlockedRecv, Process, Task, TaskState};

struct Scheduler {
    ready: VecDeque<Arc<Task>>,
    next_id: u64,
}

static SCHED: Mutex<Scheduler> = Mutex::new(Scheduler {
    ready: VecDeque::new(),
    next_id: 1,
});
static CURRENT: Mutex<Option<Arc<Task>>> = Mutex::new(None);
static REAPED: Mutex<Vec<Arc<Task>>> = Mutex::new(Vec::new());
static KERNEL_PROC: Once<Arc<Process>> = Once::new();

// the shared "process" every kernel thread belongs to (pid 0)
pub fn kernel_process() -> Arc<Process> {
    KERNEL_PROC.get().cloned().unwrap_or_else(|| {
        let proc = Arc::new(Process::new(0, "kernel", AddressSpace::kernel()));
        KERNEL_PROC.call_once(|| proc.clone());
        proc
    })
}

// all scheduler state is touched with interrupts off
pub fn init_current(task: Task) {
    interrupts::without_interrupts(|| {
        *CURRENT.lock() = Some(Arc::new(task));
    });
    serial_println!("[sched] task running: main (0)");
}

pub fn current() -> Option<Arc<Task>> {
    CURRENT.lock().clone()
}

fn alloc_id() -> u64 {
    let mut s = SCHED.lock();
    let id = s.next_id;
    s.next_id += 1;
    id
}

pub fn spawn_kernel(name: &'static str, f: impl FnOnce() + Send + 'static) -> u64 {
    interrupts::without_interrupts(|| {
        let mut task = Task::kernel_thread(name, kernel_process(), Box::new(f));
        let id = alloc_id();
        task.id = id;
        let task = Arc::new(task);
        SCHED.lock().ready.push_back(task.clone());
        serial_println!("[sched] task ready: {} ({})", name, id);
        id
    })
}

// queue a new thread of `proc`; the process lives while its threads hold Arcs
pub fn spawn_user(
    name: &'static str,
    proc: Arc<Process>,
    entry: VirtAddr,
    stack_top: VirtAddr,
    arg: u64,
) -> u64 {
    interrupts::without_interrupts(|| {
        let mut task = Task::user_process(name, proc.clone(), entry, stack_top, arg);
        let id = alloc_id();
        task.id = id;
        proc.alive
            .fetch_add(1, core::sync::atomic::Ordering::AcqRel);
        let task = Arc::new(task);
        SCHED.lock().ready.push_back(task.clone());
        serial_println!("[sched] thread ready: {} pid={} tid={}", name, proc.id, id);
        id
    })
}

// push a woken task back into rotation; caller holds irq-off context
pub fn push_ready(task: Arc<Task>) {
    SCHED.lock().ready.push_back(task);
}

// prev must stay alive across the switch: its context is written by the asm
fn switch_to(next: Arc<Task>) {
    let prev = CURRENT
        .lock()
        .replace(next.clone())
        .expect("switch without current");
    {
        let p = prev.inner();
        p.user_rsp_saved = crate::syscall::saved_user_rsp();
        p.state = TaskState::Ready;
    }
    next.inner().state = TaskState::Running;
    crate::syscall::enter_task(next.kstack_top().as_u64(), next.inner().user_rsp_saved);
    crate::gdt::set_rsp0(next.kstack_top());
    next.proc.switch();
    prev.switch_from(&next);
}

// switch away while current stays parked (Blocked); the waker will push it
// back and enter_task restores its syscall frame state on the way in
// returns false when there is nobody else to run
fn switch_away_parked() -> bool {
    let next = SCHED.lock().ready.pop_front();
    let Some(next) = next else {
        return false;
    };
    let prev = CURRENT
        .lock()
        .replace(next.clone())
        .expect("park without current");
    next.inner().state = TaskState::Running;
    crate::syscall::enter_task(next.kstack_top().as_u64(), next.inner().user_rsp_saved);
    crate::gdt::set_rsp0(next.kstack_top());
    next.proc.switch();
    prev.switch_from(&next);
    true
}

// preemption point from the timer handler (interrupts already off)
pub fn on_tick() {
    let cur = match CURRENT.lock().clone() {
        Some(c) => c,
        None => return,
    };
    let next = {
        let mut s = SCHED.lock();
        s.ready.push_back(cur.clone());
        s.ready.pop_front()
    };
    if let Some(next) = next {
        if !Arc::ptr_eq(&next, &cur) {
            switch_to(next);
        }
    }
}

pub fn yield_now() {
    interrupts::disable();
    on_tick();
    interrupts::enable();
}

// block the current thread until a matching ipc message arrives; the sender
// copies the payload into (buf, buf+max) and resumes us with ret = sender pid
pub fn recv_or_block(src: u64, buf: u64, max: u64) -> u64 {
    interrupts::without_interrupts(|| {
        let cur = current().expect("recv without current");
        if let Some(from) = crate::ipc::try_recv(&cur, src, buf, max) {
            return from;
        }
        {
            let t = cur.inner();
            t.user_rsp_saved = crate::syscall::saved_user_rsp();
            t.blocked = Some(BlockedRecv { src, buf, max });
            t.state = TaskState::Blocked;
        }
        cur.proc.waiters.lock().push(cur.clone());
        if switch_away_parked() {
            // resumed: the waker filled wake_ret and copied the message
            cur.inner().wake_ret
        } else {
            // nothing else to run: stay runnable rather than park forever
            cur.inner().state = TaskState::Running;
            cur.proc.waiters.lock().retain(|t| !Arc::ptr_eq(t, &cur));
            (-1i64) as u64
        }
    })
}

// current task never returns; the reaper drops it from the main loop
pub fn exit_current(code: i64) -> ! {
    interrupts::disable();
    let cur = CURRENT.lock().take().expect("exit without current");
    {
        let t = cur.inner();
        t.state = TaskState::Finished;
        t.exit_code = code;
    }
    // never keep an exited task in the ipc park list
    cur.proc.waiters.lock().retain(|t| !Arc::ptr_eq(t, &cur));
    if cur.proc.id != 0 {
        let was = cur
            .proc
            .alive
            .fetch_sub(1, core::sync::atomic::Ordering::AcqRel);
        if was == 1 {
            serial_println!(
                "[sched] process '{}' ({}) finished",
                cur.proc.name,
                cur.proc.id
            );
        }
    }
    REAPED.lock().push(cur);
    // guard must be dropped before the switch: this task never returns
    let next = SCHED.lock().ready.pop_front();
    if let Some(next) = next {
        // no prev to save: the exiting task already left CURRENT
        *CURRENT.lock() = Some(next.clone());
        next.inner().state = TaskState::Running;
        crate::syscall::enter_task(next.kstack_top().as_u64(), next.inner().user_rsp_saved);
        crate::gdt::set_rsp0(next.kstack_top());
        next.proc.switch();
        next.switch_into();
    }
    loop {
        x86_64::instructions::hlt();
    }
}

pub fn reap() {
    let done: Vec<Arc<Task>> = core::mem::take(&mut *REAPED.lock());
    for task in done {
        serial_println!(
            "[sched] {} (pid {} tid {}) exited with code {}",
            task.name,
            task.proc.id,
            task.id,
            task.inner().exit_code
        );
        // drops the kernel stack; user frames stay leaked until frame dealloc
    }
}
