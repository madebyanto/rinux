pub const SECTOR_SIZE: usize = 512;
pub const MAX_LBA28: u32 = 1 << 28;
pub const MAX_SECTORS_PER_COMMAND: usize = 256;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AtaInfo {
    pub sectors: u32,
    pub lba48: bool,
    model: [u8; 40],
}

impl AtaInfo {
    pub fn model(&self) -> &str {
        let mut start = 0;
        let mut end = self.model.len();
        while start < end && (self.model[start] == b' ' || self.model[start] == 0) {
            start += 1;
        }
        while end > start && (self.model[end - 1] == b' ' || self.model[end - 1] == 0) {
            end -= 1;
        }
        core::str::from_utf8(&self.model[start..end]).unwrap_or("unknown")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AtaError {
    NotInitialized,
    AlreadyInitialized,
    Timeout,
    DeviceError { lba: u32, status: u8, error: u8 },
    NoDevice,
    IdentifyFailed,
    LbaOutOfRange,
    BufferTooSmall { needed: usize, provided: usize },
    InvalidCount,
}

pub fn chunk_len(remaining: usize) -> usize {
    remaining.min(MAX_SECTORS_PER_COMMAND)
}

pub fn validate_request(lba: u32, count: usize, buffer_len: usize) -> Result<usize, AtaError> {
    if count == 0 {
        return Err(AtaError::InvalidCount);
    }
    if lba >= MAX_LBA28 {
        return Err(AtaError::LbaOutOfRange);
    }
    let available = (MAX_LBA28 - lba) as u64;
    if count as u64 > available {
        return Err(AtaError::LbaOutOfRange);
    }
    let needed = count
        .checked_mul(SECTOR_SIZE)
        .ok_or(AtaError::InvalidCount)?;
    if buffer_len < needed {
        return Err(AtaError::BufferTooSmall {
            needed,
            provided: buffer_len,
        });
    }
    Ok(needed)
}

fn identify_word(block: &[u8; 512], index: usize) -> u16 {
    u16::from_le_bytes([block[index * 2], block[index * 2 + 1]])
}

pub fn parse_identify(block: &[u8; 512]) -> Result<AtaInfo, AtaError> {
    if identify_word(block, 0) & 0x8000 != 0 {
        return Err(AtaError::IdentifyFailed);
    }
    let sectors = identify_word(block, 60) as u32 | ((identify_word(block, 61) as u32) << 16);
    let lba48 = identify_word(block, 83) & (1 << 10) != 0;
    let mut model = [b' '; 40];
    for index in 0..20 {
        let word = identify_word(block, 27 + index);
        model[index * 2] = (word >> 8) as u8;
        model[index * 2 + 1] = word as u8;
    }
    Ok(AtaInfo {
        sectors,
        lba48,
        model,
    })
}

#[cfg(target_os = "none")]
mod backend {
    use super::{chunk_len, parse_identify, validate_request, AtaError, AtaInfo, SECTOR_SIZE};
    use crate::arch::x86_64::port::{inb, inw, outb, outw};
    use crate::sync::SpinLock;

    const DATA: u16 = 0x1F0;
    const ERROR: u16 = 0x1F1;
    const SECTOR_COUNT: u16 = 0x1F2;
    const LBA_LOW: u16 = 0x1F3;
    const LBA_MID: u16 = 0x1F4;
    const LBA_HIGH: u16 = 0x1F5;
    const DRIVE_HEAD: u16 = 0x1F6;
    const STATUS: u16 = 0x1F7;
    const COMMAND: u16 = 0x1F7;
    const ALT_STATUS: u16 = 0x3F6;

    const IDENTIFY: u8 = 0xEC;
    const READ_SECTORS: u8 = 0x20;
    const WRITE_SECTORS: u8 = 0x30;
    const FLUSH_CACHE: u8 = 0xE7;

    const BSY: u8 = 0x80;
    const DRDY: u8 = 0x40;
    const DF: u8 = 0x20;
    const DRQ: u8 = 0x08;
    const ERR: u8 = 0x01;

    const POLL_LIMIT: u32 = 1_000_000;

    static ATA: SpinLock<Option<AtaInfo>> = SpinLock::new(None);

    pub fn init() -> Result<AtaInfo, AtaError> {
        let mut guard = ATA.lock();
        if guard.is_some() {
            return Err(AtaError::AlreadyInitialized);
        }
        let info = identify()?;
        *guard = Some(info);
        Ok(info)
    }

    pub fn read_sectors(lba: u32, count: usize, buffer: &mut [u8]) -> Result<(), AtaError> {
        let guard = ATA.lock();
        if guard.is_none() {
            return Err(AtaError::NotInitialized);
        }
        validate_request(lba, count, buffer.len())?;
        let mut current = lba;
        let mut offset = 0;
        let mut remaining = count;
        while remaining > 0 {
            let chunk = chunk_len(remaining);
            let end = offset + chunk * SECTOR_SIZE;
            read_chunk(current, chunk, &mut buffer[offset..end])?;
            current += chunk as u32;
            offset = end;
            remaining -= chunk;
        }
        Ok(())
    }

    #[allow(dead_code)]
    pub fn write_sectors(lba: u32, count: usize, buffer: &[u8]) -> Result<(), AtaError> {
        let guard = ATA.lock();
        if guard.is_none() {
            return Err(AtaError::NotInitialized);
        }
        validate_request(lba, count, buffer.len())?;
        let mut current = lba;
        let mut offset = 0;
        let mut remaining = count;
        while remaining > 0 {
            let chunk = chunk_len(remaining);
            let end = offset + chunk * SECTOR_SIZE;
            write_chunk(current, chunk, &buffer[offset..end])?;
            current += chunk as u32;
            offset = end;
            remaining -= chunk;
        }
        flush()
    }

    fn identify() -> Result<AtaInfo, AtaError> {
        unsafe {
            outb(DRIVE_HEAD, 0xA0);
        }
        delay_400ns();
        unsafe {
            outb(SECTOR_COUNT, 0);
            outb(LBA_LOW, 0);
            outb(LBA_MID, 0);
            outb(LBA_HIGH, 0);
            outb(COMMAND, IDENTIFY);
        }
        delay_400ns();
        let status = wait_not_busy(0)?;
        if status & (ERR | DF) != 0 {
            return Err(AtaError::IdentifyFailed);
        }
        if unsafe { inb(LBA_MID) } != 0 || unsafe { inb(LBA_HIGH) } != 0 {
            return Err(AtaError::IdentifyFailed);
        }
        wait_drq(0)?;
        let mut block = [0u8; 512];
        for index in 0..256 {
            let word = unsafe { inw(DATA) };
            block[index * 2..index * 2 + 2].copy_from_slice(&word.to_le_bytes());
        }
        check_status(0)?;
        parse_identify(&block)
    }

    fn read_chunk(lba: u32, count: usize, buffer: &mut [u8]) -> Result<(), AtaError> {
        select(lba)?;
        set_transfer(lba, count);
        unsafe {
            outb(COMMAND, READ_SECTORS);
        }
        delay_400ns();
        for sector in 0..count {
            let sector_lba = lba + sector as u32;
            wait_drq(sector_lba)?;
            let offset = sector * SECTOR_SIZE;
            for index in 0..256 {
                let word = unsafe { inw(DATA) };
                let target = offset + index * 2;
                buffer[target..target + 2].copy_from_slice(&word.to_le_bytes());
            }
            check_status(sector_lba)?;
        }
        wait_idle(lba + count as u32 - 1)
    }

    fn write_chunk(lba: u32, count: usize, buffer: &[u8]) -> Result<(), AtaError> {
        select(lba)?;
        set_transfer(lba, count);
        unsafe {
            outb(COMMAND, WRITE_SECTORS);
        }
        delay_400ns();
        for sector in 0..count {
            let sector_lba = lba + sector as u32;
            wait_drq(sector_lba)?;
            let offset = sector * SECTOR_SIZE;
            for index in 0..256 {
                let source = offset + index * 2;
                let word = u16::from_le_bytes([buffer[source], buffer[source + 1]]);
                unsafe {
                    outw(DATA, word);
                }
            }
            check_status(sector_lba)?;
        }
        wait_idle(lba + count as u32 - 1)
    }

    fn flush() -> Result<(), AtaError> {
        unsafe {
            outb(DRIVE_HEAD, 0xA0);
        }
        delay_400ns();
        wait_ready(0)?;
        unsafe {
            outb(COMMAND, FLUSH_CACHE);
        }
        delay_400ns();
        wait_idle(0)
    }

    fn select(lba: u32) -> Result<(), AtaError> {
        unsafe {
            outb(DRIVE_HEAD, 0xE0 | ((lba >> 24) & 0x0F) as u8);
        }
        delay_400ns();
        wait_ready(lba)
    }

    fn set_transfer(lba: u32, count: usize) {
        let count_register = if count == 256 { 0 } else { count as u8 };
        unsafe {
            outb(SECTOR_COUNT, count_register);
            outb(LBA_LOW, lba as u8);
            outb(LBA_MID, (lba >> 8) as u8);
            outb(LBA_HIGH, (lba >> 16) as u8);
        }
    }

    fn status() -> u8 {
        unsafe { inb(STATUS) }
    }

    fn error() -> u8 {
        unsafe { inb(ERROR) }
    }

    fn delay_400ns() {
        for _ in 0..4 {
            let _ = unsafe { inb(ALT_STATUS) };
        }
    }

    fn wait_not_busy(_lba: u32) -> Result<u8, AtaError> {
        for _ in 0..POLL_LIMIT {
            let status = status();
            if status == 0xFF {
                return Err(AtaError::NoDevice);
            }
            if status & BSY == 0 {
                return Ok(status);
            }
        }
        Err(AtaError::Timeout)
    }

    fn wait_ready(lba: u32) -> Result<(), AtaError> {
        for _ in 0..POLL_LIMIT {
            let status = status();
            if status == 0xFF {
                return Err(AtaError::NoDevice);
            }
            if status & (ERR | DF) != 0 {
                return Err(AtaError::DeviceError {
                    lba,
                    status,
                    error: error(),
                });
            }
            if status & (BSY | DRDY) == DRDY {
                return Ok(());
            }
        }
        Err(AtaError::Timeout)
    }

    fn wait_drq(lba: u32) -> Result<(), AtaError> {
        for _ in 0..POLL_LIMIT {
            let status = status();
            if status == 0xFF {
                return Err(AtaError::NoDevice);
            }
            if status & (ERR | DF) != 0 {
                return Err(AtaError::DeviceError {
                    lba,
                    status,
                    error: error(),
                });
            }
            if status & (BSY | DRQ) == DRQ {
                return Ok(());
            }
        }
        Err(AtaError::Timeout)
    }

    fn wait_idle(lba: u32) -> Result<(), AtaError> {
        for _ in 0..POLL_LIMIT {
            let status = status();
            if status == 0xFF {
                return Err(AtaError::NoDevice);
            }
            if status & (ERR | DF) != 0 {
                return Err(AtaError::DeviceError {
                    lba,
                    status,
                    error: error(),
                });
            }
            if status & BSY == 0 {
                return Ok(());
            }
        }
        Err(AtaError::Timeout)
    }

    fn check_status(lba: u32) -> Result<(), AtaError> {
        let status = status();
        if status == 0xFF {
            return Err(AtaError::NoDevice);
        }
        if status & (ERR | DF) != 0 {
            return Err(AtaError::DeviceError {
                lba,
                status,
                error: error(),
            });
        }
        Ok(())
    }
}

#[cfg(target_os = "none")]
#[allow(unused_imports)]
pub use backend::{init, read_sectors, write_sectors};
