use alloc::boxed::Box;
use alloc::collections::VecDeque;
use alloc::sync::Arc;
use alloc::vec::Vec;
use spin::{Mutex, Once};
use x86_64::instructions::interrupts;
use x86_64::VirtAddr;

use crate::serial_println;
use crate::task::{AddressSpace, Task, TaskState};

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
static KERNEL_ASPACE: Once<Arc<AddressSpace>> = Once::new();

pub fn kernel_aspace() -> Arc<AddressSpace> {
    KERNEL_ASPACE.get().cloned().unwrap_or_else(|| {
        let aspace = Arc::new(AddressSpace::kernel());
        KERNEL_ASPACE.call_once(|| aspace.clone());
        aspace
    })
}

// all scheduler state is touched with interrupts off
pub fn init_current(task: Task) {
    let name = task.name;
    interrupts::without_interrupts(|| {
        *CURRENT.lock() = Some(Arc::new(task));
    });
    serial_println!("[sched] task running: {} (0)", name);
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
        let mut task = Task::kernel_thread(name, kernel_aspace(), Box::new(f));
        let id = alloc_id();
        task.id = id;
        let task = Arc::new(task);
        SCHED.lock().ready.push_back(task.clone());
        serial_println!("[sched] task ready: {} ({})", name, id);
        id
    })
}

pub fn spawn_user(
    name: &'static str,
    aspace: Arc<AddressSpace>,
    entry: VirtAddr,
    stack_top: VirtAddr,
) -> u64 {
    interrupts::without_interrupts(|| {
        let mut task = Task::user_process(name, aspace, entry, stack_top);
        let id = alloc_id();
        task.id = id;
        let task = Arc::new(task);
        SCHED.lock().ready.push_back(task.clone());
        serial_println!("[sched] task ready: {} ({})", name, id);
        id
    })
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
    next.aspace.switch();
    prev.switch_from(&next);
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

// current task never returns; the reaper drops it from the main loop
pub fn exit_current(code: i64) -> ! {
    interrupts::disable();
    let cur = CURRENT.lock().take().expect("exit without current");
    {
        let t = cur.inner();
        t.state = TaskState::Finished;
        t.exit_code = code;
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
        next.aspace.switch();
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
            "[sched] {} ({}) exited with code {}",
            task.name,
            task.id,
            task.inner().exit_code
        );
        // drops the kernel stack; user frames stay leaked until frame dealloc
    }
}
