#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Partition {
    pub start_lba: u32,
    pub sectors: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MbrError {
    Signature,
    NoPartition,
    OutOfRange,
    Misaligned,
}

pub fn partition(block: &[u8; 512]) -> Result<Partition, MbrError> {
    if block[510] != 0x55 || block[511] != 0xAA {
        return Err(MbrError::Signature);
    }
    for index in 0..4 {
        let offset = 446 + index * 16;
        let kind = block[offset + 4];
        let start = u32::from_le_bytes([
            block[offset + 8],
            block[offset + 9],
            block[offset + 10],
            block[offset + 11],
        ]);
        let sectors = u32::from_le_bytes([
            block[offset + 12],
            block[offset + 13],
            block[offset + 14],
            block[offset + 15],
        ]);
        if kind == 0x83 && start != 0 && sectors != 0 {
            return Ok(Partition {
                start_lba: start,
                sectors,
            });
        }
    }
    Err(MbrError::NoPartition)
}

pub fn absolute_lba(start_lba: u32, offset: u64) -> Result<u32, MbrError> {
    if offset % 512 != 0 {
        return Err(MbrError::Misaligned);
    }
    let delta = offset / 512;
    if delta > u32::MAX as u64 {
        return Err(MbrError::OutOfRange);
    }
    start_lba
        .checked_add(delta as u32)
        .ok_or(MbrError::OutOfRange)
}
