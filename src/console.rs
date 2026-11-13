use crate::arch::x86_64::{serial, vga};
use core::fmt::{self, Write};

struct Console;

impl Write for Console {
    fn write_str(&mut self, text: &str) -> fmt::Result {
        for byte in text.bytes() {
            serial::write_byte(byte);
            vga::write_byte(byte);
        }
        Ok(())
    }
}

pub fn print(args: fmt::Arguments) {
    let mut console = Console;
    let _ = console.write_fmt(args);
}

#[macro_export]
macro_rules! kprint {
    ($($arg:tt)*) => {
        $crate::console::print(format_args!($($arg)*))
    };
}

#[macro_export]
macro_rules! kprintln {
    () => {
        $crate::kprint!("\n")
    };
    ($($arg:tt)*) => {
        $crate::console::print(format_args!("{}\n", format_args!($($arg)*)))
    };
}
