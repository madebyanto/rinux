use super::port::{inb, outb};

const COM1: u16 = 0x3F8;
const LINE_STATUS: u16 = COM1 + 5;
const TRANSMIT_HOLDING_EMPTY: u8 = 0x20;

pub fn init() {
    unsafe {
        outb(COM1 + 1, 0x00);
        outb(COM1 + 3, 0x80);
        outb(COM1, 0x03);
        outb(COM1 + 1, 0x00);
        outb(COM1 + 3, 0x03);
        outb(COM1 + 2, 0xC7);
        outb(COM1 + 4, 0x0B);
    }
}

pub fn write_byte(byte: u8) {
    if byte == b'\n' {
        write_raw(b'\r');
    }
    write_raw(byte);
}

fn write_raw(byte: u8) {
    unsafe {
        while (inb(LINE_STATUS) & TRANSMIT_HOLDING_EMPTY) == 0 {}
        outb(COM1, byte);
    }
}
