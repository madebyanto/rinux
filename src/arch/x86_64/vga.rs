use core::sync::atomic::{AtomicUsize, Ordering};

const BUFFER_ADDRESS: usize = 0xB8000;
const COLUMNS: usize = 80;
const ROWS: usize = 25;
const CELLS: usize = COLUMNS * ROWS;
const ATTRIBUTE: u16 = 0x0F00;

static CURSOR: AtomicUsize = AtomicUsize::new(0);

fn blank() -> u16 {
    ATTRIBUTE | b' ' as u16
}

fn put(index: usize, value: u16) {
    unsafe {
        core::ptr::write_volatile((BUFFER_ADDRESS as *mut u16).add(index), value);
    }
}

fn get(index: usize) -> u16 {
    unsafe { core::ptr::read_volatile((BUFFER_ADDRESS as *const u16).add(index)) }
}

pub fn init() {
    for index in 0..CELLS {
        put(index, blank());
    }
    CURSOR.store(0, Ordering::Relaxed);
}

pub fn write_byte(byte: u8) {
    if byte == b'\n' {
        newline();
        return;
    }
    let mut position = CURSOR.load(Ordering::Relaxed);
    if position >= CELLS {
        scroll();
        position = CURSOR.load(Ordering::Relaxed);
    }
    put(position, ATTRIBUTE | byte as u16);
    CURSOR.store(position + 1, Ordering::Relaxed);
}

fn newline() {
    let position = CURSOR.load(Ordering::Relaxed);
    let next = (position / COLUMNS + 1) * COLUMNS;
    CURSOR.store(next, Ordering::Relaxed);
    if next >= CELLS {
        scroll();
    }
}

fn scroll() {
    for index in COLUMNS..CELLS {
        put(index - COLUMNS, get(index));
    }
    for index in (ROWS - 1) * COLUMNS..CELLS {
        put(index, blank());
    }
    CURSOR.store((ROWS - 1) * COLUMNS, Ordering::Relaxed);
}
