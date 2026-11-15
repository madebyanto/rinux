#![no_std]
#![no_main]

#[macro_use]
mod console;
mod arch;
mod drivers;
mod fs;
mod init;
mod mm;
mod proc;
mod sync;
mod syscall;

extern crate alloc;

use arch::x86_64::cpu;
use arch::x86_64::multiboot2::BootInfo;
use arch::x86_64::{gdt, idt, pic, pit, serial, vga};
use core::panic::PanicInfo;
use drivers::{ata, ps2};
use fs::vfs::{DirCursor, FileType};
use proc::scheduler;

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
    let command_line = init::cmdline::init(boot_info.command_line());
    let boot_params = init::cmdline::params();
    if command_line.truncated {
        kprintln!(
            "cmdline: truncated to {} bytes",
            command_line.stored_len
        );
    }
    if command_line.unknown > 0 {
        kprintln!(
            "cmdline: {} unknown parameter(s) ignored",
            command_line.unknown
        );
    }
    if command_line.invalid > 0 {
        kprintln!(
            "cmdline: {} invalid parameter value(s) ignored",
            command_line.invalid
        );
    }
    kprintln!(
        "cmdline: console={:?}, loglevel={}, selftest={}",
        boot_params.console, boot_params.loglevel, boot_params.selftest
    );
    if let Some(root) = boot_params.root {
        kprintln!("cmdline: root={}", root);
    }
    if let Some(init) = boot_params.init {
        kprintln!("cmdline: init={}", init);
    }
    kprintln!(
        "multiboot2: {} usable memory regions",
        boot_info.usable_regions().len()
    );
    mm::init(&boot_info);
    if boot_params.selftest {
        mm::self_test();
    }
    kprintln!("memory subsystem ready");

    gdt::init();
    arch::x86_64::syscall::init();

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
    let ps2_flushed = ps2::flush_stale();
    let ata_info = match ata::init() {
        Ok(info) => info,
        Err(error) => fatal("ATA initialization failed", error),
    };
    let mut mbr_sector = [0u8; 512];
    if let Err(error) = ata::read_sectors(0, 1, &mut mbr_sector) {
        fatal("ATA MBR sector read failed", error);
    }
    let mbr_found = mbr_sector[510] == 0x55 && mbr_sector[511] == 0xAA;
    if !mbr_found {
        fatal_display("ext2 mount failed", "no MBR signature 0x55AA");
    }
    let mount_info = match fs::mount(ata_info.sectors) {
        Ok(info) => info,
        Err(error) => fatal_display("ext2 mount failed", error),
    };
    if let Err(error) = scheduler::init() {
        fatal("scheduler initialization failed", error);
    }
    if let Err(error) = scheduler::spawn_init(init::init_task::kernel_init) {
        fatal("kernel_init spawn failed", error);
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
    kprintln!("ps2: flushed {} stale keys", ps2_flushed);
    kprintln!(
        "ata: {}, {} sectors (LBA28, LBA48 {})",
        ata_info.model(),
        ata_info.sectors,
        if ata_info.lba48 {
            "supported"
        } else {
            "not supported"
        }
    );
    kprintln!(
        "ata: MBR signature 0x55AA {}",
        if mbr_found { "found" } else { "not found" }
    );
    print_ext2(&mount_info);
    kprintln!(
        "gdt: kernel CS/SS {:#x}/{:#x}, user CS/SS {:#x}/{:#x}, TSS {:#x}",
        gdt::KERNEL_CODE,
        gdt::KERNEL_DATA,
        gdt::USER_CODE_RPL3,
        gdt::USER_DATA_RPL3,
        gdt::TSS_SELECTOR
    );
    kprintln!(
        "syscall: SCE=1 LSTAR={:#x} STAR={:#x} FMASK={:#x}",
        arch::x86_64::syscall::lstar(),
        arch::x86_64::syscall::star_value(),
        arch::x86_64::syscall::fmask_value()
    );
    kprintln!(
        "scheduler: round-robin, {} tasks ({} ready), timeslice {} ms ({} ticks)",
        scheduler::task_count(),
        scheduler::ready_count(),
        scheduler::TIMESLICE_MS,
        scheduler::TIMESLICE_TICKS
    );
    cpu::enable_interrupts();
    cpu::without_interrupts(|| kprintln!("interrupts enabled"));
    scheduler::yield_now();
    loop {
        cpu::halt();
        if ps2::take_magic() {
            unsafe { core::ptr::write_volatile(core::ptr::null_mut::<u64>(), 0) };
        }
    }
}

fn print_ext2(info: &fs::MountInfo) {
    kprintln!(
        "ext2: partition start LBA {}, {} sectors",
        info.partition.start_lba,
        info.partition.sectors
    );
    kprintln!(
        "ext2: volume \"{}\", block size {}, inode size {}, inodes {}, blocks {}",
        info.volume,
        info.block_size,
        info.inode_size,
        info.inodes,
        info.blocks
    );
    kprintln!("ext2: /");
    let root = match fs::root() {
        Ok(root) => root,
        Err(error) => fatal_display("ext2 root failed", error),
    };
    let mut cursor = DirCursor::new();
    loop {
        match fs::readdir(root, &mut cursor) {
            Ok(Some(entry)) => {
                if entry.name == "." || entry.name == ".." {
                    continue;
                }
                let slash = if entry.kind == FileType::Directory {
                    "/"
                } else {
                    ""
                };
                kprintln!("ext2:   {}{}", entry.name, slash);
            }
            Ok(None) => break,
            Err(error) => fatal_display("ext2 readdir failed", error),
        }
    }
}

fn fatal_display(context: &str, error: impl core::fmt::Display) -> ! {
    kprintln!("{}: {}", context, error);
    cpu::halt_forever()
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
