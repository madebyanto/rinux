use core::arch::asm;

pub fn halt_forever() -> ! {
    loop {
        unsafe {
            asm!("hlt", options(nomem, nostack, preserves_flags));
        }
    }
}

pub fn read_cr3() -> u64 {
    let value: u64;
    unsafe {
        asm!("mov {}, cr3", out(reg) value, options(nomem, nostack, preserves_flags));
    }
    value
}

pub fn invlpg(address: usize) {
    unsafe {
        asm!("invlpg [{}]", in(reg) address, options(nostack, preserves_flags));
    }
}
