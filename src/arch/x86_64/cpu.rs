use core::arch::asm;

pub fn halt() {
    unsafe {
        asm!("hlt", options(nomem, nostack, preserves_flags));
    }
}

pub fn halt_forever() -> ! {
    loop {
        halt();
    }
}

pub fn disable_interrupts() {
    unsafe {
        asm!("cli", options(nomem, nostack));
    }
}

pub fn enable_interrupts() {
    unsafe {
        asm!("sti", options(nomem, nostack));
    }
}

pub fn read_rflags() -> u64 {
    let flags: u64;
    unsafe {
        asm!("pushfq; pop {}", out(reg) flags, options(preserves_flags));
    }
    flags
}

pub fn restore_rflags(flags: u64) {
    unsafe {
        asm!("push {}; popfq", in(reg) flags);
    }
}

pub fn without_interrupts<F, R>(function: F) -> R
where
    F: FnOnce() -> R,
{
    let flags = read_rflags();
    disable_interrupts();
    let result = function();
    restore_rflags(flags);
    result
}

pub fn read_cr2() -> u64 {
    let value: u64;
    unsafe {
        asm!("mov {}, cr2", out(reg) value, options(nomem, nostack, preserves_flags));
    }
    value
}

pub fn read_cr3() -> u64 {
    let value: u64;
    unsafe {
        asm!("mov {}, cr3", out(reg) value, options(nomem, nostack, preserves_flags));
    }
    value
}

pub fn write_cr3(value: usize) {
    unsafe {
        asm!("mov cr3, {}", in(reg) value, options(nostack, preserves_flags));
    }
}

pub fn invlpg(address: usize) {
    unsafe {
        asm!("invlpg [{}]", in(reg) address, options(nostack, preserves_flags));
    }
}
