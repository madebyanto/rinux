#![no_std]
#![no_main]

#[macro_use]
mod console;
mod arch;
mod drivers;
mod fs;
mod mm;
mod proc;
mod sync;
mod syscall;
mod user;

extern crate alloc;

use arch::x86_64::cpu;
use arch::x86_64::multiboot2::BootInfo;
use arch::x86_64::{gdt, idt, pic, pit, serial, vga};
use core::panic::PanicInfo;
use drivers::{ata, ps2};
use fs::vfs::{DirCursor, FileType};
use proc::scheduler;

const TASK_REPORT_INTERVAL_MS: u64 = 300;

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
    if let Err(error) = scheduler::spawn(task_one) {
        fatal("task 1 spawn failed", error);
    }
    if let Err(error) = scheduler::spawn(task_two) {
        fatal("task 2 spawn failed", error);
    }
    if let Err(error) = scheduler::spawn(task_three) {
        fatal("task 3 spawn failed", error);
    }
    let image = match user::exec::load("/bin/init.elf", &["/bin/init.elf"], &[]) {
        Ok(image) => image,
        Err(error) => fatal_display("init ELF load failed", error),
    };
    let image_entry = image.entry;
    let image_stack = image.stack_top;
    if let Err(error) = scheduler::spawn_user(image_entry, image_stack, image.address_space) {
        fatal("user task spawn failed", error);
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
        "elf: loaded /bin/init.elf, entry {:#x}, stack {:#x}",
        image_entry,
        image_stack
    );
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
        while let Some(key) = ps2::read_key() {
            match key {
                b'\n' => kprintln!(),
                b'\t' => kprint!("    "),
                0x20..=0x7E => kprint!("{}", key as char),
                _ => {}
            }
        }
        if ps2::take_magic() {
            unsafe { core::ptr::write_volatile(core::ptr::null_mut::<u64>(), 0) };
        }
    }
}

extern "C" fn task_one() {
    task_body(1)
}

extern "C" fn task_two() {
    task_body(2)
}

extern "C" fn task_three() {
    task_body(3)
}

fn task_body(id: u32) -> ! {
    let mut next_report = 0u64;
    loop {
        let now = pit::uptime_ms();
        if now >= next_report {
            cpu::without_interrupts(|| {
                kprintln!("task {} alive, uptime {} ms", id, now);
            });
            next_report = now.saturating_add(TASK_REPORT_INTERVAL_MS);
        }
        cpu::halt();
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
    kprintln!("ext2: cat /hello.txt");
    let mut file = match fs::open("/hello.txt") {
        Ok(file) => file,
        Err(error) => fatal_display("ext2 open failed", error),
    };
    let mut buffer = [0u8; 128];
    loop {
        match fs::read(&mut file, &mut buffer) {
            Ok(0) => break,
            Ok(count) => {
                for byte in &buffer[..count] {
                    console::write_byte(*byte);
                }
            }
            Err(error) => fatal_display("ext2 read failed", error),
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
