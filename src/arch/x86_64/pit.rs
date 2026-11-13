use super::port::outb;
use crate::arch::x86_64::cpu;
use crate::sync::SpinLock;

pub const FREQUENCY_HZ: u32 = 100;

const BASE_FREQUENCY_HZ: u32 = 1_193_182;
const CHANNEL0: u16 = 0x40;
const COMMAND: u16 = 0x43;
const MODE_SQUARE_WAVE: u8 = 0x36;
const TICK_MS: u64 = 1000 / FREQUENCY_HZ as u64;

static UPTIME_MS: SpinLock<u64> = SpinLock::new(0);

pub fn init() {
    let divisor = (BASE_FREQUENCY_HZ / FREQUENCY_HZ) as u16;
    unsafe {
        outb(COMMAND, MODE_SQUARE_WAVE);
        outb(CHANNEL0, divisor as u8);
        outb(CHANNEL0, (divisor >> 8) as u8);
    }
}

pub fn handle_tick() {
    *UPTIME_MS.lock() += TICK_MS;
}

pub fn uptime_ms() -> u64 {
    cpu::without_interrupts(|| *UPTIME_MS.lock())
}
