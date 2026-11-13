use core::ptr::read_unaligned;

const TAG_END: u32 = 0;
const TAG_MEMORY_MAP: u32 = 6;
const MEMORY_AVAILABLE: u32 = 1;
const INFO_HEADER_SIZE: usize = 8;
const TAG_HEADER_SIZE: usize = 8;
const MAP_HEADER_SIZE: usize = 16;
const MAP_ENTRY_MIN_SIZE: usize = 24;
pub const MAX_REGIONS: usize = 32;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Region {
    pub start: usize,
    pub end: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BootInfoError {
    NullAddress,
    Misaligned,
    TruncatedTag,
    MissingMemoryMap,
    TooManyRegions,
}

pub struct BootInfo {
    info_start: usize,
    info_end: usize,
    regions: [Region; MAX_REGIONS],
    region_count: usize,
}

impl BootInfo {
    pub unsafe fn parse(address: usize) -> Result<Self, BootInfoError> {
        if address == 0 {
            return Err(BootInfoError::NullAddress);
        }
        if address % 8 != 0 {
            return Err(BootInfoError::Misaligned);
        }
        let total = read_u32(address) as usize;
        if total < INFO_HEADER_SIZE {
            return Err(BootInfoError::TruncatedTag);
        }
        let mut info = Self {
            info_start: address,
            info_end: address + total,
            regions: [Region { start: 0, end: 0 }; MAX_REGIONS],
            region_count: 0,
        };
        let mut memory_map_found = false;
        let mut offset = INFO_HEADER_SIZE;
        while offset + TAG_HEADER_SIZE <= total {
            let tag = address + offset;
            let tag_type = read_u32(tag);
            let tag_size = read_u32(tag + 4) as usize;
            if tag_type == TAG_END {
                break;
            }
            if tag_size < TAG_HEADER_SIZE || offset + tag_size > total {
                return Err(BootInfoError::TruncatedTag);
            }
            if tag_type == TAG_MEMORY_MAP {
                info.parse_memory_map(tag, tag_size)?;
                memory_map_found = true;
            }
            offset += align8(tag_size);
        }
        if !memory_map_found {
            return Err(BootInfoError::MissingMemoryMap);
        }
        Ok(info)
    }

    pub fn usable_regions(&self) -> &[Region] {
        &self.regions[..self.region_count]
    }

    pub fn info_range(&self) -> (usize, usize) {
        (self.info_start, self.info_end)
    }

    unsafe fn parse_memory_map(&mut self, tag: usize, size: usize) -> Result<(), BootInfoError> {
        if size < MAP_HEADER_SIZE {
            return Err(BootInfoError::TruncatedTag);
        }
        let entry_size = read_u32(tag + 8) as usize;
        if entry_size < MAP_ENTRY_MIN_SIZE {
            return Err(BootInfoError::TruncatedTag);
        }
        let end = tag + size;
        let mut entry = tag + MAP_HEADER_SIZE;
        while entry + MAP_ENTRY_MIN_SIZE <= end {
            let base = read_u64(entry) as usize;
            let length = read_u64(entry + 8) as usize;
            let kind = read_u32(entry + 16);
            if kind == MEMORY_AVAILABLE && length > 0 {
                self.push_region(Region {
                    start: base,
                    end: base.saturating_add(length),
                })?;
            }
            entry += entry_size;
        }
        Ok(())
    }

    fn push_region(&mut self, region: Region) -> Result<(), BootInfoError> {
        if self.region_count == MAX_REGIONS {
            return Err(BootInfoError::TooManyRegions);
        }
        self.regions[self.region_count] = region;
        self.region_count += 1;
        Ok(())
    }
}

unsafe fn read_u32(address: usize) -> u32 {
    read_unaligned(address as *const u32)
}

unsafe fn read_u64(address: usize) -> u64 {
    read_unaligned(address as *const u64)
}

fn align8(value: usize) -> usize {
    (value + 7) & !7
}
