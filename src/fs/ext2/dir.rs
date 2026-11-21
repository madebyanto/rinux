#![allow(dead_code)]

use super::structs::{u16_at, u32_at};
use super::Ext2Error;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RawDirEntry {
    pub inode: u32,
    pub name_start: usize,
    pub name_len: usize,
    pub file_type: u8,
}

pub fn parse_dir_entry(
    block: &[u8],
    offset: usize,
    has_filetype: bool,
) -> Result<(RawDirEntry, usize), Ext2Error> {
    let block_size = block.len();
    if offset >= block_size {
        return Err(Ext2Error::CorruptDirectory);
    }
    if offset + 8 > block_size {
        return Err(Ext2Error::CorruptDirectory);
    }
    let inode = u32_at(block, offset);
    let rec_len = u16_at(block, offset + 4) as usize;
    if rec_len < 8 || rec_len % 4 != 0 || offset + rec_len > block_size {
        return Err(Ext2Error::CorruptDirectory);
    }
    let (name_len, file_type) = if has_filetype {
        (block[offset + 6] as usize, block[offset + 7])
    } else {
        (u16_at(block, offset + 6) as usize, 0)
    };
    if name_len > 255 || name_len > rec_len - 8 {
        return Err(Ext2Error::CorruptDirectory);
    }
    if inode != 0 && name_len == 0 {
        return Err(Ext2Error::CorruptDirectory);
    }
    Ok((
        RawDirEntry {
            inode,
            name_start: offset + 8,
            name_len,
            file_type,
        },
        offset + rec_len,
    ))
}
