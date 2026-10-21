use core::fmt;
use spin::{Lazy, Mutex};
use uart_16550::{backend::PioBackend, Config, Uart16550Tty};
use x86_64::instructions::interrupts;

pub static SERIAL1: Lazy<Mutex<Uart16550Tty<PioBackend>>> = Lazy::new(|| {
    Mutex::new(unsafe {
        Uart16550Tty::new_port(0x3F8, Config::default()).expect("uart init failed")
    })
});

pub fn _print(args: fmt::Arguments) {
    use core::fmt::Write;
    interrupts::without_interrupts(|| {
        SERIAL1.lock().write_fmt(args).expect("serial print failed");
    });
}

#[macro_export]
macro_rules! serial_print {
    ($($arg:tt)*) => {
        $crate::serial::_print(format_args!($($arg)*))
    };
}

#[macro_export]
macro_rules! serial_println {
    () => {
        $crate::serial_print!("\n")
    };
    ($($arg:tt)*) => {
        $crate::serial_print!("{}\n", format_args!($($arg)*))
    };
}
