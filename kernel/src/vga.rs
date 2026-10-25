use core::fmt;
use core::sync::atomic::{AtomicU64, Ordering};
use spin::{Lazy, Mutex};
use volatile::Volatile;

#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Color {
    Black = 0,
    Blue,
    Green,
    Cyan,
    Red,
    Magenta,
    Brown,
    LightGray,
    DarkGray,
    LightBlue,
    LightGreen,
    LightCyan,
    LightRed,
    Pink,
    Yellow,
    White,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(transparent)]
struct ColorCode(u8);

impl ColorCode {
    const fn new(foreground: Color, background: Color) -> Self {
        ColorCode((background as u8) << 4 | foreground as u8)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(C)]
struct ScreenChar {
    ascii: u8,
    color_code: ColorCode,
}

const BUFFER_HEIGHT: usize = 25;
const BUFFER_WIDTH: usize = 80;

#[repr(transparent)]
struct Buffer {
    chars: [[Volatile<ScreenChar>; BUFFER_WIDTH]; BUFFER_HEIGHT],
}

pub struct Writer {
    column: usize,
    color_code: ColorCode,
}

// the text buffer lives at the physical vga address: identity-mapped while
// the boot pml4 is active, remapped to a higher-half page once memory is
// up — user cr3s carry only the kernel's upper half, so an identity vga
// pointer would fault on the first print after a process switch
static VGA_ADDR: AtomicU64 = AtomicU64::new(0xb8000);

pub fn remap_high() {
    let va = crate::memory::map_mmio(0xB8000, 1);
    VGA_ADDR.store(va.as_u64(), Ordering::Relaxed);
}

fn buffer() -> &'static mut Buffer {
    unsafe { &mut *(VGA_ADDR.load(Ordering::Relaxed) as *mut Buffer) }
}

impl Writer {
    pub fn write_byte(&mut self, byte: u8) {
        let buffer = buffer();
        match byte {
            b'\n' => self.new_line(),
            byte => {
                if self.column >= BUFFER_WIDTH {
                    self.new_line();
                }
                let row = BUFFER_HEIGHT - 1;
                let col = self.column;
                buffer.chars[row][col].write(ScreenChar {
                    ascii: byte,
                    color_code: self.color_code,
                });
                self.column += 1;
            }
        }
    }

    pub fn write_string(&mut self, s: &str) {
        for byte in s.bytes() {
            self.write_byte(byte);
        }
    }

    fn new_line(&mut self) {
        for row in 1..BUFFER_HEIGHT {
            let (head, tail) = buffer().chars.split_at_mut(row);
            let src = &tail[0];
            let dst = &mut head[row - 1];
            for (dst_cell, src_cell) in dst.iter_mut().zip(src.iter()) {
                dst_cell.write(src_cell.read());
            }
        }
        self.clear_row(BUFFER_HEIGHT - 1);
        self.column = 0;
    }

    fn clear_row(&mut self, row: usize) {
        let blank = ScreenChar {
            ascii: b' ',
            color_code: self.color_code,
        };
        for cell in buffer().chars[row].iter_mut() {
            cell.write(blank);
        }
    }
}

impl fmt::Write for Writer {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        self.write_string(s);
        Ok(())
    }
}

pub static WRITER: Lazy<Mutex<Writer>> = Lazy::new(|| {
    Mutex::new(Writer {
        column: 0,
        color_code: ColorCode::new(Color::LightGreen, Color::Black),
    })
});

pub fn _print(args: fmt::Arguments) {
    use core::fmt::Write;
    x86_64::instructions::interrupts::without_interrupts(|| {
        WRITER.lock().write_fmt(args).unwrap();
    });
}

pub fn put_byte(b: u8) {
    x86_64::instructions::interrupts::without_interrupts(|| WRITER.lock().write_byte(b));
}

#[macro_export]
macro_rules! print {
    ($($arg:tt)*) => {
        $crate::vga::_print(format_args!($($arg)*))
    };
}

#[macro_export]
macro_rules! println {
    () => {
        $crate::print!("\n")
    };
    ($($arg:tt)*) => {
        $crate::print!("{}\n", format_args!($($arg)*))
    };
}
