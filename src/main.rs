#![no_std]
#![no_main]

#[macro_use]
mod console;
mod arch;
mod mm;
mod sync;

extern crate alloc;

use arch::x86_64::cpu;
use arch::x86_64::multiboot2::BootInfo;
use arch::x86_64::{serial, vga};
use core::panic::PanicInfo;

#[no_mangle]
pub extern "C" fn rust_main(multiboot_info: usize) -> ! {
    serial::init();
    vga::init();
    kprintln!("Rinux {}", env!("CARGO_PKG_VERSION"));
    kprintln!("multiboot2 info at {:#x}", multiboot_info);
    kprintln!("64-bit long mode active, identity paging 0-1 GiB");
    let boot_info = match unsafe { BootInfo::parse(multiboot_info) } {
        Ok(info) => info,
        Err(error) => panic!("multiboot2 parse failed: {:?}", error),
    };
    kprintln!(
        "multiboot2: {} usable memory regions",
        boot_info.usable_regions().len()
    );
    mm::init(&boot_info);
    mm::self_test();
    kprintln!("memory subsystem ready");
    cpu::halt_forever()
}

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    kprintln!("KERNEL PANIC: {}", info);
    cpu::halt_forever()
}
