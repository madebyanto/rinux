#![no_std]
#![no_main]

#[macro_use]
mod console;
mod arch;
mod drivers;
mod mm;
mod sync;

extern crate alloc;

use arch::x86_64::cpu;
use arch::x86_64::multiboot2::BootInfo;
use arch::x86_64::{idt, pic, pit, serial, vga};
use core::panic::PanicInfo;
use drivers::ps2;

const REPORT_INTERVAL_MS: u64 = 1000;

#[no_mangle]
pub extern "C" fn rust_main(multiboot_info: usize) -> ! {
    serial::init();
    vga::init();
    kprintln!("Rinux {}", env!("CARGO_PKG_VERSION"));
    kprintln!("multiboot2 info at {:#x}", multiboot_info);
    kprintln!("64-bit long mode active, identity paging 4 KiB-1 GiB, null page reserved");
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

    pic::remap();
    idt::init();
    if let Err(error) = idt::register_irq(0, pit::handle_tick) {
        fatal("PIT IRQ registration failed", error);
    }
    if let Err(error) = idt::register_irq(1, ps2::handle_interrupt) {
        fatal("keyboard IRQ registration failed", error);
    }
    pit::init();
    if let Err(error) = ps2::init() {
        fatal("PS/2 initialization failed", error);
    }
    if let Err(error) = pic::unmask(0) {
        fatal("PIT IRQ unmask failed", error);
    }
    if let Err(error) = pic::unmask(1) {
        fatal("keyboard IRQ unmask failed", error);
    }
    kprintln!(
        "PIC remapped to vectors {}-{}",
        pic::VECTOR_BASE,
        pic::VECTOR_BASE + 15
    );
    kprintln!("IDT loaded with 256 entries");
    kprintln!("PIT configured at {} Hz", pit::FREQUENCY_HZ);
    kprintln!("PS/2 keyboard ready on IRQ1 (scancode set 1)");
    kprintln!("type to echo keys, press F12 to fault on a null pointer");
    cpu::enable_interrupts();
    kprintln!("interrupts enabled");

    let mut next_report = (pit::uptime_ms() / REPORT_INTERVAL_MS + 1) * REPORT_INTERVAL_MS;
    let mut line_open = false;
    loop {
        cpu::halt();
        while let Some(key) = ps2::read_key() {
            match key {
                b'\n' => {
                    kprintln!();
                    line_open = false;
                }
                b'\t' => {
                    kprint!("    ");
                    line_open = true;
                }
                0x20..=0x7E => {
                    kprint!("{}", key as char);
                    line_open = true;
                }
                _ => {}
            }
        }
        if ps2::take_magic() {
            unsafe { core::ptr::write_volatile(core::ptr::null_mut::<u64>(), 0) };
        }
        let now = pit::uptime_ms();
        if now >= next_report {
            if line_open {
                kprintln!();
                line_open = false;
            }
            kprintln!("uptime: {} ms", now);
            next_report = (now / REPORT_INTERVAL_MS + 1) * REPORT_INTERVAL_MS;
        }
    }
}

fn fatal(context: &str, error: impl core::fmt::Debug) -> ! {
    kprintln!("{}: {:?}", context, error);
    cpu::halt_forever()
}

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    kprintln!("KERNEL PANIC: {}", info);
    cpu::halt_forever()
}
