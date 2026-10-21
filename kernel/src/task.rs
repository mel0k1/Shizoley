use alloc::boxed::Box;
use alloc::collections::VecDeque;
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::arch::{asm, global_asm};
use core::cell::UnsafeCell;
use core::sync::atomic::AtomicUsize;
use spin::Mutex;
use x86_64::registers::control::{Cr3, Cr3Flags};
use x86_64::structures::paging::page_table::PageTableEntry;
use x86_64::structures::paging::{
    Mapper, OffsetPageTable, Page, PageTable, PageTableFlags, PhysFrame, Size4KiB,
};
use x86_64::VirtAddr;

use crate::ipc::Message;
use crate::memory;

const KSTACK_SIZE: usize = 32 * 1024;

// lower half of the address space belongs to user processes
pub const USER_LIMIT: u64 = 0x0000_8000_0000_0000;

// ---------------------------------------------------------------- address spaces

// one CR3 root per process; kernel (higher half + physmap) is shared, the
// lower half is a private deep copy with shared leaf frames
pub struct AddressSpace {
    pub(crate) pml4: PhysFrame<Size4KiB>,
}

impl AddressSpace {
    // address space of the running kernel (boot CR3), shared by kernel tasks
    pub fn kernel() -> Self {
        AddressSpace {
            pml4: *memory::KERNEL_PML4.get().expect("kernel pml4 not set"),
        }
    }

    // fresh root: zero frame, kernel upper half shared, lower half deep-copied
    pub fn new_user() -> Result<Self, &'static str> {
        let root = memory::alloc_frame().ok_or("no frames for user pml4")?;
        unsafe { zero_frame(root) };

        let kernel_root = unsafe {
            &*(memory::phys_to_virt(Self::kernel().pml4.start_address()).as_ptr::<PageTable>())
        };
        let new_root: &mut PageTable =
            unsafe { &mut *(memory::phys_to_virt(root.start_address()).as_mut_ptr()) };

        for i in 0..512 {
            let entry = kernel_root[i].clone();
            if !entry.flags().contains(PageTableFlags::PRESENT) {
                continue;
            }
            // upper half (physmap etc.) is shared verbatim, lower half is copied
            new_root[i] = if i >= 256 {
                entry
            } else {
                copy_level2(&entry)?
            };
        }
        // ring 3 may walk the lower half path; leaf grants stay supervisor-only
        for i in 0..256 {
            let e = &mut new_root[i];
            if e.flags().contains(PageTableFlags::PRESENT) {
                e.set_flags(e.flags() | PageTableFlags::USER_ACCESSIBLE);
            }
        }
        Ok(AddressSpace { pml4: root })
    }

    // mapper over this root; table frames are reached through the physmap,
    // so this works while another CR3 is active
    fn mapper(&mut self) -> OffsetPageTable<'_> {
        let virt = memory::phys_to_virt(self.pml4.start_address());
        unsafe { OffsetPageTable::new(&mut *(virt.as_mut_ptr()), memory::phys_offset()) }
    }

    // mark every existing ancestor of the page user-walkable
    fn mark_user_ancestors(&mut self, page: Page<Size4KiB>) {
        let root: &mut PageTable =
            unsafe { &mut *(memory::phys_to_virt(self.pml4.start_address()).as_mut_ptr()) };
        let pml4e = &mut root[page.p4_index()];
        pml4e.set_flags(pml4e.flags() | PageTableFlags::USER_ACCESSIBLE);
        if !pml4e.flags().contains(PageTableFlags::PRESENT) {
            return;
        }
        let pdpt: &mut PageTable =
            unsafe { &mut *(memory::phys_to_virt(pml4e.addr()).as_mut_ptr()) };
        let pdpte = &mut pdpt[page.p3_index()];
        pdpte.set_flags(pdpte.flags() | PageTableFlags::USER_ACCESSIBLE);
    }

    // map one user page; returns true when the frame was freshly inserted
    pub fn map_user_page(
        &mut self,
        page: Page<Size4KiB>,
        frame: PhysFrame<Size4KiB>,
        flags: PageTableFlags,
    ) -> Result<bool, x86_64::structures::paging::mapper::MapToError<Size4KiB>> {
        self.mark_user_ancestors(page);
        let mut mapper = self.mapper();
        let parent_flags =
            PageTableFlags::PRESENT | PageTableFlags::WRITABLE | PageTableFlags::USER_ACCESSIBLE;
        match unsafe {
            mapper.map_to_with_table_flags(
                page,
                frame,
                flags,
                parent_flags,
                &mut memory::GlobalFrames,
            )
        } {
            Ok(flush) => {
                flush.flush();
                Ok(true)
            }
            Err(x86_64::structures::paging::mapper::MapToError::PageAlreadyMapped(_)) => Ok(false),
            Err(e) => Err(e),
        }
    }

    pub fn translate_page(&mut self, page: Page<Size4KiB>) -> Option<PhysFrame<Size4KiB>> {
        self.mapper().translate_page(page).ok()
    }

    // byte-wise access into this address space via the physmap; works no
    // matter which CR3 is active, used for ipc copies between processes
    pub fn write_bytes(&mut self, mut addr: u64, mut data: &[u8]) -> bool {
        while !data.is_empty() {
            if addr >= USER_LIMIT {
                return false;
            }
            let page = Page::containing_address(VirtAddr::new(addr));
            let Some(frame) = self.translate_page(page) else {
                return false;
            };
            let off = (addr & 0xFFF) as usize;
            let n = core::cmp::min(4096 - off, data.len());
            let dst = memory::phys_to_virt(frame.start_address()).as_mut_ptr::<u8>();
            unsafe { core::ptr::copy_nonoverlapping(data.as_ptr(), dst.add(off), n) };
            addr += n as u64;
            data = &data[n..];
        }
        true
    }

    pub fn read_bytes(&mut self, mut addr: u64, buf: &mut [u8]) -> bool {
        let mut off = 0usize;
        while off < buf.len() {
            if addr >= USER_LIMIT {
                return false;
            }
            let page = Page::containing_address(VirtAddr::new(addr));
            let Some(frame) = self.translate_page(page) else {
                return false;
            };
            let page_off = (addr & 0xFFF) as usize;
            let n = core::cmp::min(4096 - page_off, buf.len() - off);
            let src = memory::phys_to_virt(frame.start_address()).as_ptr::<u8>();
            unsafe {
                core::ptr::copy_nonoverlapping(src.add(page_off), buf.as_mut_ptr().add(off), n)
            };
            addr += n as u64;
            off += n;
        }
        true
    }
}

// pdpt level: children are page directories, copied one level down
fn copy_level2(entry: &PageTableEntry) -> Result<PageTableEntry, &'static str> {
    if entry.flags().contains(PageTableFlags::HUGE_PAGE) {
        return Ok(entry.clone());
    }
    let src: &PageTable = unsafe { &*(memory::phys_to_virt(entry.addr()).as_ptr()) };
    let frame = memory::alloc_frame().ok_or("no frames for page tables")?;
    unsafe { zero_frame(frame) };
    let dst: &mut PageTable =
        unsafe { &mut *(memory::phys_to_virt(frame.start_address()).as_mut_ptr()) };
    for i in 0..512 {
        let child = src[i].clone();
        if child.flags().contains(PageTableFlags::PRESENT) {
            dst[i] = copy_level1(&child)?;
        }
    }
    let mut new_entry = entry.clone();
    let flags = new_entry.flags();
    new_entry.set_addr(frame.start_address(), flags);
    Ok(new_entry)
}

// pd level: children are page tables whose entries share data frames
fn copy_level1(entry: &PageTableEntry) -> Result<PageTableEntry, &'static str> {
    if entry.flags().contains(PageTableFlags::HUGE_PAGE) {
        return Ok(entry.clone());
    }
    let src: &PageTable = unsafe { &*(memory::phys_to_virt(entry.addr()).as_ptr()) };
    let frame = memory::alloc_frame().ok_or("no frames for page tables")?;
    unsafe { zero_frame(frame) };
    let dst: &mut PageTable =
        unsafe { &mut *(memory::phys_to_virt(frame.start_address()).as_mut_ptr()) };
    for i in 0..512 {
        let child = src[i].clone();
        if child.flags().contains(PageTableFlags::PRESENT) {
            dst[i] = child;
        }
    }
    let mut new_entry = entry.clone();
    let flags = new_entry.flags();
    new_entry.set_addr(frame.start_address(), flags);
    Ok(new_entry)
}

unsafe fn zero_frame(frame: PhysFrame<Size4KiB>) {
    let virt = memory::phys_to_virt(frame.start_address());
    core::ptr::write_bytes(virt.as_mut_ptr::<u8>(), 0, 4096);
}

// ---------------------------------------------------------------- processes

// a process owns one address space, a mailbox and a set of threads; threads
// (tasks) hold an Arc to it, so it lives while at least one thread exists
pub struct Process {
    pub id: u64,
    pub name: String,
    // immutable after creation: the switch path reads it without a lock
    pub pml4: PhysFrame<Size4KiB>,
    // page-table mutations (elf load, ipc copies) go through this lock
    pub aspace: Mutex<AddressSpace>,
    pub mailbox: Mutex<VecDeque<Message>>,
    // threads parked in ipc_recv, woken by ipc_send handoff
    pub waiters: Mutex<Vec<Arc<Task>>>,
    pub alive: AtomicUsize,
}

impl Process {
    pub fn new(id: u64, name: &str, aspace: AddressSpace) -> Self {
        Process {
            id,
            name: String::from(name),
            pml4: aspace.pml4,
            aspace: Mutex::new(aspace),
            mailbox: Mutex::new(VecDeque::new()),
            waiters: Mutex::new(Vec::new()),
            alive: AtomicUsize::new(0),
        }
    }

    pub fn switch(&self) {
        unsafe { Cr3::write(self.pml4, Cr3Flags::empty()) };
    }
}

// ---------------------------------------------------------------- context switch

// fxsave area first so it sits 16-aligned; rsp saved at +512
#[repr(C, align(16))]
pub struct SwitchFrame {
    pub fx: [u8; 512],
    pub rsp: u64,
}

// rdi = &prev frame, rsi = &next frame; callee-saved regs + fx state ride the
// per-task kernel stack, so a task interrupted mid-handler resumes cleanly
global_asm!(
    ".section .text.__paparaci_switch, \"ax\"",
    ".p2align 4",
    ".globl __paparaci_switch",
    "__paparaci_switch:",
    "    fxsave64 [rdi]",
    "    push rbp",
    "    push rbx",
    "    push r12",
    "    push r13",
    "    push r14",
    "    push r15",
    "    mov [rdi + 512], rsp",
    "    mov rsp, [rsi + 512]",
    "    pop r15",
    "    pop r14",
    "    pop r13",
    "    pop r12",
    "    pop rbx",
    "    pop rbp",
    "    fxrstor64 [rsi]",
    "    ret",
);

unsafe extern "C" {
    fn __paparaci_switch(prev: *mut SwitchFrame, next: *const SwitchFrame);
}

unsafe fn fxsave_current(area: &mut [u8; 512]) {
    asm!("fxsave64 [{}]", in(reg) area.as_mut_ptr(), options(nostack));
}

// ---------------------------------------------------------------- tasks

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskState {
    Ready,
    Running,
    Blocked,
    Finished,
}

// what a task parked in ipc_recv waits for; the waker copies the message
// straight into (buf, buf+max) of the parked task's address space
#[derive(Debug, Clone, Copy)]
pub struct BlockedRecv {
    pub src: u64,
    pub buf: u64,
    pub max: u64,
}

type Payload = Box<dyn FnOnce() + Send>;

struct KStack {
    ptr: *mut u8,
}

unsafe impl Send for KStack {}

impl KStack {
    fn new() -> Option<Self> {
        let layout = alloc::alloc::Layout::from_size_align(KSTACK_SIZE, 16).ok()?;
        let ptr = unsafe { alloc::alloc::alloc(layout) };
        if ptr.is_null() {
            return None;
        }
        Some(KStack { ptr })
    }

    fn top(&self) -> VirtAddr {
        VirtAddr::new(self.ptr as u64 + KSTACK_SIZE as u64)
    }
}

impl Drop for KStack {
    fn drop(&mut self) {
        let layout = alloc::alloc::Layout::from_size_align(KSTACK_SIZE, 16).unwrap();
        unsafe { alloc::alloc::dealloc(self.ptr, layout) };
    }
}

// scheduler-mutated part; guarded by irq-off scheduler discipline
pub struct TaskInner {
    pub ctx: SwitchFrame,
    pub state: TaskState,
    pub exit_code: i64,
    pub user_rsp_saved: u64,
    pub wake_ret: u64,
    pub blocked: Option<BlockedRecv>,
    payload: Option<Payload>,
}

pub struct Task {
    pub id: u64,
    pub name: &'static str,
    pub proc: Arc<Process>,
    pub user_entry: u64,
    pub user_rsp0: u64,
    pub user_arg: u64,
    kstack: KStack,
    inner: UnsafeCell<TaskInner>,
}

unsafe impl Send for Task {}
unsafe impl Sync for Task {}

impl Task {
    fn new(name: &'static str, proc: Arc<Process>) -> Option<Self> {
        let kstack = KStack::new()?;
        Some(Task {
            id: 0,
            name,
            proc,
            user_entry: 0,
            user_rsp0: 0,
            user_arg: 0,
            kstack,
            inner: UnsafeCell::new(TaskInner {
                ctx: SwitchFrame {
                    fx: [0; 512],
                    rsp: 0,
                },
                state: TaskState::Ready,
                exit_code: 0,
                user_rsp_saved: 0,
                wake_ret: 0,
                blocked: None,
                payload: None,
            }),
        })
    }

    // scheduler-only view of the mutable half; single core, irq-off paths
    // UnsafeCell-backed: the Sync impl plus scheduler discipline make it sound
    #[allow(clippy::mut_from_ref)]
    pub fn inner(&self) -> &mut TaskInner {
        unsafe { &mut *self.inner.get() }
    }

    pub fn kstack_top(&self) -> VirtAddr {
        self.kstack.top()
    }

    fn arm(&mut self, entry: usize) {
        unsafe { fxsave_current(&mut self.inner.get_mut().ctx.fx) };
        // leave rsp at top-8 on entry so fresh tasks look call-entered
        let base = self.kstack.top().as_u64() - 8 * 8;
        let slots = base as *mut u64;
        unsafe {
            for i in 0..6 {
                slots.add(i).write_volatile(0);
            }
            slots.add(6).write_volatile(entry as u64);
        }
        self.inner.get_mut().ctx.rsp = base;
    }

    pub fn boot_current(name: &'static str, proc: Arc<Process>) -> Self {
        let mut task = Task::new(name, proc).expect("task alloc failed");
        unsafe { fxsave_current(&mut task.inner.get_mut().ctx.fx) };
        task
    }

    pub fn kernel_thread(name: &'static str, proc: Arc<Process>, f: Payload) -> Self {
        let mut task = Task::new(name, proc).expect("task alloc failed");
        task.arm(kernel_task_entry as unsafe extern "C" fn() -> ! as usize);
        task.inner.get_mut().payload = Some(f);
        task
    }

    pub fn user_process(
        name: &'static str,
        proc: Arc<Process>,
        entry: VirtAddr,
        stack_top: VirtAddr,
        arg: u64,
    ) -> Self {
        let mut task = Task::new(name, proc).expect("task alloc failed");
        task.arm(user_task_entry as unsafe extern "C" fn() -> ! as usize);
        task.user_entry = entry.as_u64();
        task.user_rsp0 = stack_top.as_u64();
        task.user_arg = arg;
        task
    }

    pub(crate) fn take_payload(&self) -> Option<Payload> {
        self.inner().payload.take()
    }

    pub(crate) fn switch_from(&self, next: &Task) {
        unsafe { __paparaci_switch(&mut (*self.inner.get()).ctx, &(*next.inner.get()).ctx) };
    }

    // entry for a task whose predecessor is already gone (task exit)
    pub(crate) fn switch_into(&self) {
        static mut DUMMY: SwitchFrame = SwitchFrame {
            fx: [0; 512],
            rsp: 0,
        };
        unsafe { __paparaci_switch(core::ptr::addr_of_mut!(DUMMY), &(*self.inner.get()).ctx) };
    }
}

pub extern "C" fn kernel_task_entry() -> ! {
    x86_64::instructions::interrupts::enable();
    let task = crate::sched::current().expect("kernel entry without current");
    let payload = task.take_payload().expect("kernel task without payload");
    payload();
    crate::sched::exit_current(0);
}

pub extern "C" fn user_task_entry() -> ! {
    let task = crate::sched::current().expect("user entry without current");
    let entry = task.user_entry;
    let rsp = task.user_rsp0;
    let arg = task.user_arg;
    unsafe { crate::user::enter_ring3(VirtAddr::new(entry), VirtAddr::new(rsp), arg) };
}
