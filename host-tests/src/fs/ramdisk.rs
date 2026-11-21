use super::ext2::{BlockDevice, Ext2Error};

/// A RAM-backed block device for ext2 tests.
pub struct RamDisk {
    data: Vec<u8>,
    reads: usize,
    writes: usize,
    flushes: usize,
    fail_writes: bool,
}

impl RamDisk {
    pub fn new(data: Vec<u8>) -> Self {
        Self {
            data,
            reads: 0,
            writes: 0,
            flushes: 0,
            fail_writes: false,
        }
    }

    pub fn data(&self) -> &[u8] {
        &self.data
    }

    pub fn into_data(self) -> Vec<u8> {
        self.data
    }

    pub fn reads(&self) -> usize {
        self.reads
    }

    pub fn writes(&self) -> usize {
        self.writes
    }

    pub fn flushes(&self) -> usize {
        self.flushes
    }

    pub fn set_fail_writes(&mut self, fail: bool) {
        self.fail_writes = fail;
    }
}

impl BlockDevice for RamDisk {
    fn read_at(&mut self, offset: u64, buffer: &mut [u8]) -> Result<(), Ext2Error> {
        let start = offset as usize;
        let end = start.checked_add(buffer.len()).ok_or(Ext2Error::Device)?;
        if end > self.data.len() {
            return Err(Ext2Error::Device);
        }
        buffer.copy_from_slice(&self.data[start..end]);
        self.reads += 1;
        Ok(())
    }

    fn write_at(&mut self, offset: u64, buffer: &[u8]) -> Result<(), Ext2Error> {
        if self.fail_writes {
            return Err(Ext2Error::Device);
        }
        let start = offset as usize;
        let end = start.checked_add(buffer.len()).ok_or(Ext2Error::Device)?;
        if end > self.data.len() {
            return Err(Ext2Error::Device);
        }
        self.data[start..end].copy_from_slice(buffer);
        self.writes += 1;
        Ok(())
    }

    fn flush(&mut self) -> Result<(), Ext2Error> {
        self.flushes += 1;
        Ok(())
    }
}
