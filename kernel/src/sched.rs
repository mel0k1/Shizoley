use alloc::boxed::Box;
use alloc::collections::VecDeque;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::sync::atomic::Ordering;
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
    // irq-off: the timer handler locks CURRENT from on_tick; a syscall
    // context holding it with interrupts on would deadlock the handler
    interrupts::without_interrupts(|| CURRENT.lock().clone())
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

// resume a parked task (ipc/eq waiters) with ret as its blocked-syscall result
pub fn wake_parked(task: &Arc<Task>, ret: u64) {
    {
        let t = task.inner();
        t.wake_ret = ret;
        t.blocked = None;
        t.state = TaskState::Ready;
    }
    SCHED.lock().ready.push_back(task.clone());
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
pub(crate) fn switch_away_parked() -> bool {
    let next = pop_ready();
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
    {
        let mut s = SCHED.lock();
        s.ready.push_back(cur.clone());
    }
    if let Some(next) = pop_ready() {
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
        match crate::ipc::try_recv(&cur, src, buf, max) {
            crate::ipc::Recv::Delivered(from) => return from,
            // a matching message exists but cannot be delivered into the
            // given buffer: report the error instead of parking on it forever
            crate::ipc::Recv::Bad => return (-1i64) as u64,
            crate::ipc::Recv::Empty => {}
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
    if cur.proc.id != 0 && cur.proc.alive.load(Ordering::Acquire) > 0 {
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
    let next = pop_ready();
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
        // drops the kernel stack; the process half is freed right here
        maybe_destroy_process(&task.proc);
    }
}

// last thread of a user process is gone: release everything it owned —
// address space frames, page tables, ipc registration, event queues, caps
fn maybe_destroy_process(proc: &Arc<Process>) {
    if proc.id == 0 || proc.alive.load(Ordering::Acquire) != 0 {
        return;
    }
    if proc.torn_down.swap(true, Ordering::AcqRel) {
        return;
    }
    crate::ipc::unregister(proc.id);
    crate::vmx::close_process(proc);
    crate::eq::close_process(proc.id);
    crate::memobj::close_process(proc);
    crate::caps::drop_process(proc.id);
    crate::futex::drop_process(proc.id);
    let freed = proc.aspace.lock().teardown();
    serial_println!(
        "[sched] process '{}' ({}) torn down, {} frames freed",
        proc.name,
        proc.id,
        freed
    );
}

// pop the next runnable task, silently reaping Finished ones (kill can mark
// tasks that still sit in the ready queue)
fn pop_ready() -> Option<Arc<Task>> {
    loop {
        let task = SCHED.lock().ready.pop_front()?;
        if task.inner().state == TaskState::Finished {
            REAPED.lock().push(task);
            continue;
        }
        return Some(task);
    }
}

// kill(14): mark every thread of the target process Finished, detach it from
// every park list and let the reaper free the process. killing the caller's
// own process is allowed and behaves like exiting the whole group.
pub fn kill_process(target: u64) -> i64 {
    if target == 0 {
        return -1; // the kernel process is immortal
    }
    // unknown pid: nothing to kill, report it instead of fake success
    if crate::ipc::find(target).is_none() {
        return -1;
    }
    interrupts::without_interrupts(|| {
        let cur = current();
        let is_self = cur.as_ref().is_some_and(|t| t.proc.id == target);
        let mut killed = Vec::new();
        // members sitting in the ready queue
        {
            let mut s = SCHED.lock();
            let (mine, rest): (Vec<_>, Vec<_>) =
                s.ready.drain(..).partition(|t| t.proc.id == target);
            s.ready.extend(rest);
            killed.extend(mine);
        }
        // parked in event-queue wait/producer lists
        killed.extend(crate::eq::detach_process(target));
        // parked on futex words
        killed.extend(crate::futex::detach_process(target));
        // parked in the target's own recv waiters
        if let Some(proc) = crate::ipc::find(target) {
            killed.extend(proc.waiters.lock().drain(..));
        }
        for task in &killed {
            let t = task.inner();
            t.state = TaskState::Finished;
            t.blocked = None;
        }
        if let Some(proc) = crate::ipc::find(target) {
            proc.alive.store(0, Ordering::Release);
        }
        REAPED.lock().extend(killed);
        if is_self {
            // the caller is part of the group: exit via the normal path;
            // exit_current will not double-count alive since it is already 0
            exit_current(-3);
        }
        0
    })
}
