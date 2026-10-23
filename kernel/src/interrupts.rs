use crate::gdt;
use crate::sched;
use crate::{println, serial_print, serial_println};
use core::sync::atomic::{AtomicU64, Ordering};
use pic8259::ChainedPics;
use spin::{Lazy, Mutex};
use x86_64::instructions::port::Port;
use x86_64::structures::idt::{InterruptDescriptorTable, InterruptStackFrame, PageFaultErrorCode};
use x86_64::PrivilegeLevel;

pub const PIC_1_OFFSET: u8 = 32;
pub const PIC_2_OFFSET: u8 = PIC_1_OFFSET + 8;
// the timer's line, visible to vmx exit handling for scheduler fairness
pub const TIMER_VECTOR: u8 = PIC_1_OFFSET;

// scheduler tick counter (also the vmx preemption-timer calibration clock)
pub fn tick_count() -> u64 {
    TICKS.load(Ordering::Relaxed)
}

static PICS: spin::Mutex<ChainedPics> =
    spin::Mutex::new(unsafe { ChainedPics::new(PIC_1_OFFSET, PIC_2_OFFSET) });

static TICKS: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone, Copy)]
#[repr(u8)]
enum InterruptIndex {
    Timer = PIC_1_OFFSET,
    Keyboard,
}

impl InterruptIndex {
    fn as_u8(self) -> u8 {
        self as u8
    }
}

static IDT: Lazy<InterruptDescriptorTable> = Lazy::new(|| {
    let mut idt = InterruptDescriptorTable::new();
    idt.breakpoint.set_handler_fn(breakpoint_handler);
    idt.divide_error.set_handler_fn(divide_error_handler);
    idt.invalid_opcode.set_handler_fn(invalid_opcode_handler);
    idt.general_protection_fault
        .set_handler_fn(general_protection_handler);
    unsafe {
        idt.double_fault
            .set_handler_fn(double_fault_handler)
            .set_stack_index(gdt::DOUBLE_FAULT_IST_INDEX);
    }
    idt.page_fault.set_handler_fn(page_fault_handler);
    idt[InterruptIndex::Timer.as_u8()].set_handler_fn(timer_handler);
    idt[InterruptIndex::Keyboard.as_u8()].set_handler_fn(keyboard_handler);
    // vmx posted-interrupt notification vector: the cpu consumes this
    // vector itself while a guest runs (it never reaches the host idt in
    // that case); in vmx-root it is a silent stub so a stray notification
    // can never land on an unhandled idt slot and shut the machine down
    idt[0xF0].set_handler_fn(pi_notify_handler);
    idt
});

fn pit_init() {
    // channel 0, lo/hi, square wave: ~100 Hz
    const DIVISOR: u16 = 11_932;
    unsafe {
        Port::new(0x43).write(0x36u8);
        Port::new(0x40).write((DIVISOR & 0xFF) as u8);
        Port::new(0x40).write((DIVISOR >> 8) as u8);
    }
}

pub fn init() {
    IDT.load();
    unsafe { PICS.lock().initialize() };
    pit_init();
    x86_64::instructions::interrupts::enable();
}

extern "x86-interrupt" fn breakpoint_handler(stack_frame: InterruptStackFrame) {
    serial_println!("[paparaci] breakpoint: {:#?}", stack_frame);
}

extern "x86-interrupt" fn divide_error_handler(stack_frame: InterruptStackFrame) {
    fault_kill("divide error", stack_frame);
}

extern "x86-interrupt" fn invalid_opcode_handler(stack_frame: InterruptStackFrame) {
    fault_kill("invalid opcode", stack_frame);
}

extern "x86-interrupt" fn general_protection_handler(
    stack_frame: InterruptStackFrame,
    error_code: u64,
) {
    serial_println!("[paparaci] gpf (code {:#x})", error_code);
    fault_kill("general protection fault", stack_frame);
}

// ring-3 faults terminate the buggy process; the same fault in the kernel
// is a kernel bug and halts (the serial log tells the story)
fn fault_kill(what: &str, stack_frame: InterruptStackFrame) {
    let from_user = stack_frame.code_segment.rpl() == PrivilegeLevel::Ring3;
    serial_println!(
        "[paparaci] {} from ring {} at {:#x}",
        what,
        stack_frame.code_segment.rpl() as u8,
        stack_frame.instruction_pointer.as_u64()
    );
    if from_user {
        sched::exit_current(-2);
    }
    loop {
        x86_64::instructions::hlt();
    }
}

extern "x86-interrupt" fn double_fault_handler(
    stack_frame: InterruptStackFrame,
    _error_code: u64,
) -> ! {
    panic!("[paparaci] double fault: {:#?}", stack_frame);
}

extern "x86-interrupt" fn page_fault_handler(
    stack_frame: InterruptStackFrame,
    error_code: PageFaultErrorCode,
) {
    use x86_64::registers::control::Cr2;
    let fault_addr = x86_64::VirtAddr::new(Cr2::read_raw());
    let from_user = stack_frame.code_segment.rpl() == PrivilegeLevel::Ring3;
    println!(
        "[paparaci] page fault at {:?} ({:?})",
        fault_addr, error_code
    );
    serial_println!(
        "[paparaci] page fault at {:#x} from ring {} ({:?})",
        fault_addr,
        stack_frame.code_segment.rpl() as u8,
        error_code
    );
    if from_user {
        // buggy userspace dies, the kernel lives on
        sched::exit_current(-2);
    }
    loop {
        x86_64::instructions::hlt();
    }
}

// the vmx posted-interrupt notification vector (0xf0): nothing to ack (it
// cannot come from the pic) and the kernel entry-sync processes the
// descriptor anyway — just return
extern "x86-interrupt" fn pi_notify_handler(_stack_frame: InterruptStackFrame) {}

extern "x86-interrupt" fn timer_handler(_stack_frame: InterruptStackFrame) {
    let ticks = TICKS.fetch_add(1, Ordering::Relaxed);
    if ticks % 512 == 0 {
        serial_print!(".");
    }
    // EOI before switching: the next task runs with the PIC ready
    unsafe {
        PICS.lock()
            .notify_end_of_interrupt(InterruptIndex::Timer.as_u8())
    };
    sched::on_tick();
}

extern "x86-interrupt" fn keyboard_handler(_stack_frame: InterruptStackFrame) {
    use pc_keyboard::{layouts, DecodedKey, HandleControl, Keyboard, ScancodeSet1};

    static KEYBOARD: Lazy<Mutex<Keyboard<layouts::Us104Key, ScancodeSet1>>> = Lazy::new(|| {
        Mutex::new(Keyboard::new(
            ScancodeSet1::new(),
            layouts::Us104Key,
            HandleControl::Ignore,
        ))
    });

    let mut keyboard = KEYBOARD.lock();
    let mut port = Port::new(0x60);
    let scancode: u8 = unsafe { port.read() };

    if let Ok(Some(key_event)) = keyboard.add_byte(scancode) {
        if let Some(key) = keyboard.process_keyevent(key_event) {
            // the console is a ring-3 process now: keys travel as events on
            // the listening queue instead of being echoed by the kernel
            match key {
                DecodedKey::Unicode(character) => {
                    crate::eq::kbd_event(scancode, character as u8);
                }
                DecodedKey::RawKey(_) => {
                    crate::eq::kbd_event(scancode, 0);
                }
            }
        }
    }

    unsafe {
        PICS.lock()
            .notify_end_of_interrupt(InterruptIndex::Keyboard.as_u8())
    };
}

// irq taken while the cpu runs a guest (vmx root operation): the vmx exit
// handler forwards the vector here. we acknowledge the pic and keep the
// accounting alive; scheduling decisions wait until the guest exits.
// returns true when the interrupt was one of ours.
pub fn dispatch_in_root(vector: u8) -> bool {
    match vector {
        v if v == InterruptIndex::Timer.as_u8() => {
            TICKS.fetch_add(1, Ordering::Relaxed);
            unsafe {
                PICS.lock()
                    .notify_end_of_interrupt(InterruptIndex::Timer.as_u8())
            };
            true
        }
        v if v == InterruptIndex::Keyboard.as_u8() => {
            // read the scancode and hand it to the console queue
            use pc_keyboard::{layouts, HandleControl, Keyboard, ScancodeSet1};
            static KEYBOARD_ROOT: Lazy<Mutex<Keyboard<layouts::Us104Key, ScancodeSet1>>> =
                Lazy::new(|| {
                    Mutex::new(Keyboard::new(
                        ScancodeSet1::new(),
                        layouts::Us104Key,
                        HandleControl::Ignore,
                    ))
                });
            let scancode: u8 = unsafe { Port::new(0x60).read() };
            let mut keyboard = KEYBOARD_ROOT.lock();
            if let Ok(Some(key_event)) = keyboard.add_byte(scancode) {
                if let Some(key) = keyboard.process_keyevent(key_event) {
                    match key {
                        pc_keyboard::DecodedKey::Unicode(c) => {
                            crate::eq::kbd_event(scancode, c as u8)
                        }
                        pc_keyboard::DecodedKey::RawKey(_) => crate::eq::kbd_event(scancode, 0),
                    }
                }
            }
            unsafe {
                PICS.lock()
                    .notify_end_of_interrupt(InterruptIndex::Keyboard.as_u8())
            };
            true
        }
        _ => false,
    }
}
