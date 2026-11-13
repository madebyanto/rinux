use super::port::{inb, outb};

pub const IRQ_COUNT: u8 = 16;
pub const VECTOR_BASE: u8 = 32;

const MASTER_COMMAND: u16 = 0x20;
const MASTER_DATA: u16 = 0x21;
const SLAVE_COMMAND: u16 = 0xA0;
const SLAVE_DATA: u16 = 0xA1;
const IO_WAIT_PORT: u16 = 0x80;

const ICW1_INIT: u8 = 0x11;
const ICW4_8086: u8 = 0x01;
const CASCADE_IRQ: u8 = 2;
const EOI: u8 = 0x20;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PicError {
    InvalidIrq(u8),
}

pub fn remap() {
    unsafe {
        outb(MASTER_COMMAND, ICW1_INIT);
        io_wait();
        outb(SLAVE_COMMAND, ICW1_INIT);
        io_wait();

        outb(MASTER_DATA, VECTOR_BASE);
        io_wait();
        outb(SLAVE_DATA, VECTOR_BASE + 8);
        io_wait();

        outb(MASTER_DATA, 1 << CASCADE_IRQ);
        io_wait();
        outb(SLAVE_DATA, CASCADE_IRQ);
        io_wait();

        outb(MASTER_DATA, ICW4_8086);
        io_wait();
        outb(SLAVE_DATA, ICW4_8086);
        io_wait();
    }
    mask_all();
}

pub fn mask_all() {
    for irq in 0..IRQ_COUNT {
        let _ = mask(irq);
    }
}

pub fn mask(irq: u8) -> Result<(), PicError> {
    let port = data_port(irq)?;
    let line = irq & 7;
    unsafe {
        outb(port, inb(port) | (1 << line));
    }
    Ok(())
}

pub fn unmask(irq: u8) -> Result<(), PicError> {
    let port = data_port(irq)?;
    let line = irq & 7;
    unsafe {
        outb(port, inb(port) & !(1 << line));
        if irq >= 8 {
            outb(MASTER_DATA, inb(MASTER_DATA) & !(1 << CASCADE_IRQ));
        }
    }
    Ok(())
}

pub fn eoi(irq: u8) -> Result<(), PicError> {
    if irq >= IRQ_COUNT {
        return Err(PicError::InvalidIrq(irq));
    }
    unsafe {
        if irq >= 8 {
            outb(SLAVE_COMMAND, EOI);
        }
        outb(MASTER_COMMAND, EOI);
    }
    Ok(())
}

fn data_port(irq: u8) -> Result<u16, PicError> {
    match irq {
        0..=7 => Ok(MASTER_DATA),
        8..=15 => Ok(SLAVE_DATA),
        _ => Err(PicError::InvalidIrq(irq)),
    }
}

fn io_wait() {
    unsafe {
        outb(IO_WAIT_PORT, 0);
    }
}
