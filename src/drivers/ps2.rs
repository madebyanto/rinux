#[cfg(target_os = "none")]
use crate::arch::x86_64::cpu;
#[cfg(target_os = "none")]
use crate::arch::x86_64::port::{inb, outb};
#[cfg(target_os = "none")]
use crate::sync::SpinLock;

pub const BUFFER_CAPACITY: usize = 64;
pub const F12: u8 = 0x58;

const SCANCODE_RELEASE: u8 = 0x80;
const LEFT_SHIFT: u8 = 0x2A;
const LEFT_SHIFT_RELEASE: u8 = LEFT_SHIFT | SCANCODE_RELEASE;
const RIGHT_SHIFT: u8 = 0x36;
const RIGHT_SHIFT_RELEASE: u8 = RIGHT_SHIFT | SCANCODE_RELEASE;

#[cfg(target_os = "none")]
const DATA_PORT: u16 = 0x60;
#[cfg(target_os = "none")]
const STATUS_PORT: u16 = 0x64;
#[cfg(target_os = "none")]
const COMMAND_PORT: u16 = 0x64;
#[cfg(target_os = "none")]
const OUTPUT_FULL: u8 = 0x01;
#[cfg(target_os = "none")]
const INPUT_FULL: u8 = 0x02;
#[cfg(target_os = "none")]
const COMMAND_READ_CONFIG: u8 = 0x20;
#[cfg(target_os = "none")]
const COMMAND_WRITE_CONFIG: u8 = 0x60;
#[cfg(target_os = "none")]
const COMMAND_ENABLE_KEYBOARD: u8 = 0xAE;
#[cfg(target_os = "none")]
const COMMAND_DISABLE_KEYBOARD: u8 = 0xAD;
#[cfg(target_os = "none")]
const CONFIG_KEYBOARD_IRQ: u8 = 0x01;
#[cfg(target_os = "none")]
const FLUSH_LIMIT: usize = 32;
#[cfg(target_os = "none")]
const WAIT_LIMIT: usize = 100_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyBufferError {
    Full,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ps2Error {
    InputTimeout,
    OutputTimeout,
}

pub struct KeyBuffer {
    data: [u8; BUFFER_CAPACITY],
    head: usize,
    tail: usize,
    len: usize,
}

impl KeyBuffer {
    pub const fn new() -> Self {
        Self {
            data: [0; BUFFER_CAPACITY],
            head: 0,
            tail: 0,
            len: 0,
        }
    }

    pub fn push(&mut self, key: u8) -> Result<(), KeyBufferError> {
        if self.len == BUFFER_CAPACITY {
            return Err(KeyBufferError::Full);
        }
        self.data[self.tail] = key;
        self.tail = (self.tail + 1) % BUFFER_CAPACITY;
        self.len += 1;
        Ok(())
    }

    pub fn pop(&mut self) -> Option<u8> {
        if self.len == 0 {
            return None;
        }
        let key = self.data[self.head];
        self.head = (self.head + 1) % BUFFER_CAPACITY;
        self.len -= 1;
        Some(key)
    }

    #[allow(dead_code)]
    pub fn len(&self) -> usize {
        self.len
    }

    #[allow(dead_code)]
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }
}

pub struct Keyboard {
    shift: bool,
    magic: bool,
    buffer: KeyBuffer,
}

impl Keyboard {
    pub const fn new() -> Self {
        Self {
            shift: false,
            magic: false,
            buffer: KeyBuffer::new(),
        }
    }

    pub fn handle_scancode(&mut self, scancode: u8) -> Option<u8> {
        match scancode {
            LEFT_SHIFT | RIGHT_SHIFT => {
                self.shift = true;
                None
            }
            LEFT_SHIFT_RELEASE | RIGHT_SHIFT_RELEASE => {
                self.shift = false;
                None
            }
            F12 => {
                self.magic = true;
                None
            }
            _ => {
                if scancode & SCANCODE_RELEASE != 0 {
                    return None;
                }
                let key = translate(scancode, self.shift)?;
                let _ = self.buffer.push(key);
                Some(key)
            }
        }
    }

    pub fn read_key(&mut self) -> Option<u8> {
        self.buffer.pop()
    }

    pub fn take_magic(&mut self) -> bool {
        let magic = self.magic;
        self.magic = false;
        magic
    }

    #[allow(dead_code)]
    pub fn shift_pressed(&self) -> bool {
        self.shift
    }

    #[allow(dead_code)]
    pub fn pending(&self) -> usize {
        self.buffer.len()
    }
}

pub fn translate(scancode: u8, shift: bool) -> Option<u8> {
    if scancode & SCANCODE_RELEASE != 0 {
        return None;
    }
    Some(match scancode {
        0x01 => 0x1B,
        0x02 => pick(shift, b'1', b'!'),
        0x03 => pick(shift, b'2', b'@'),
        0x04 => pick(shift, b'3', b'#'),
        0x05 => pick(shift, b'4', b'$'),
        0x06 => pick(shift, b'5', b'%'),
        0x07 => pick(shift, b'6', b'^'),
        0x08 => pick(shift, b'7', b'&'),
        0x09 => pick(shift, b'8', b'*'),
        0x0A => pick(shift, b'9', b'('),
        0x0B => pick(shift, b'0', b')'),
        0x0C => pick(shift, b'-', b'_'),
        0x0D => pick(shift, b'=', b'+'),
        0x0E => 0x08,
        0x0F => b'\t',
        0x10 => pick(shift, b'q', b'Q'),
        0x11 => pick(shift, b'w', b'W'),
        0x12 => pick(shift, b'e', b'E'),
        0x13 => pick(shift, b'r', b'R'),
        0x14 => pick(shift, b't', b'T'),
        0x15 => pick(shift, b'y', b'Y'),
        0x16 => pick(shift, b'u', b'U'),
        0x17 => pick(shift, b'i', b'I'),
        0x18 => pick(shift, b'o', b'O'),
        0x19 => pick(shift, b'p', b'P'),
        0x1A => pick(shift, b'[', b'{'),
        0x1B => pick(shift, b']', b'}'),
        0x1C => b'\n',
        0x1E => pick(shift, b'a', b'A'),
        0x1F => pick(shift, b's', b'S'),
        0x20 => pick(shift, b'd', b'D'),
        0x21 => pick(shift, b'f', b'F'),
        0x22 => pick(shift, b'g', b'G'),
        0x23 => pick(shift, b'h', b'H'),
        0x24 => pick(shift, b'j', b'J'),
        0x25 => pick(shift, b'k', b'K'),
        0x26 => pick(shift, b'l', b'L'),
        0x27 => pick(shift, b';', b':'),
        0x28 => pick(shift, b'\'', b'"'),
        0x29 => pick(shift, b'`', b'~'),
        0x2B => pick(shift, b'\\', b'|'),
        0x2C => pick(shift, b'z', b'Z'),
        0x2D => pick(shift, b'x', b'X'),
        0x2E => pick(shift, b'c', b'C'),
        0x2F => pick(shift, b'v', b'V'),
        0x30 => pick(shift, b'b', b'B'),
        0x31 => pick(shift, b'n', b'N'),
        0x32 => pick(shift, b'm', b'M'),
        0x33 => pick(shift, b',', b'<'),
        0x34 => pick(shift, b'.', b'>'),
        0x35 => pick(shift, b'/', b'?'),
        0x39 => b' ',
        _ => return None,
    })
}

fn pick(shift: bool, normal: u8, shifted: u8) -> u8 {
    if shift {
        shifted
    } else {
        normal
    }
}

#[cfg(target_os = "none")]
static KEYBOARD: SpinLock<Keyboard> = SpinLock::new(Keyboard::new());

#[cfg(target_os = "none")]
pub fn init() -> Result<(), Ps2Error> {
    unsafe {
        flush_output();
        wait_input_clear()?;
        outb(COMMAND_PORT, COMMAND_DISABLE_KEYBOARD);
        flush_output();
        wait_input_clear()?;
        outb(COMMAND_PORT, COMMAND_READ_CONFIG);
        let config = read_data()?;
        wait_input_clear()?;
        outb(COMMAND_PORT, COMMAND_WRITE_CONFIG);
        wait_input_clear()?;
        outb(DATA_PORT, config | CONFIG_KEYBOARD_IRQ);
        wait_input_clear()?;
        outb(COMMAND_PORT, COMMAND_ENABLE_KEYBOARD);
        flush_output();
    }
    Ok(())
}

#[cfg(target_os = "none")]
pub fn handle_interrupt() {
    let status = unsafe { inb(STATUS_PORT) };
    if status & OUTPUT_FULL == 0 {
        return;
    }
    let scancode = unsafe { inb(DATA_PORT) };
    KEYBOARD.lock().handle_scancode(scancode);
}

#[cfg(target_os = "none")]
pub fn read_key() -> Option<u8> {
    cpu::without_interrupts(|| KEYBOARD.lock().read_key())
}

#[cfg(target_os = "none")]
pub fn take_magic() -> bool {
    cpu::without_interrupts(|| KEYBOARD.lock().take_magic())
}

#[cfg(target_os = "none")]
fn flush_output() {
    for _ in 0..FLUSH_LIMIT {
        if unsafe { inb(STATUS_PORT) } & OUTPUT_FULL == 0 {
            return;
        }
        let _ = unsafe { inb(DATA_PORT) };
    }
}

#[cfg(target_os = "none")]
fn wait_input_clear() -> Result<(), Ps2Error> {
    for _ in 0..WAIT_LIMIT {
        if unsafe { inb(STATUS_PORT) } & INPUT_FULL == 0 {
            return Ok(());
        }
    }
    Err(Ps2Error::InputTimeout)
}

#[cfg(target_os = "none")]
fn read_data() -> Result<u8, Ps2Error> {
    for _ in 0..WAIT_LIMIT {
        if unsafe { inb(STATUS_PORT) } & OUTPUT_FULL != 0 {
            return Ok(unsafe { inb(DATA_PORT) });
        }
    }
    Err(Ps2Error::OutputTimeout)
}
