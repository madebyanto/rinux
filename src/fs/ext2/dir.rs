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

/// Write one directory entry, clearing the unused name area.
fn write_entry(
    block: &mut [u8],
    offset: usize,
    rec_len: usize,
    inode: u32,
    name: &[u8],
    file_type: u8,
    has_filetype: bool,
) {
    block[offset..offset + 4].copy_from_slice(&inode.to_le_bytes());
    block[offset + 4..offset + 6].copy_from_slice(&(rec_len as u16).to_le_bytes());
    if has_filetype {
        block[offset + 6] = name.len() as u8;
        block[offset + 7] = file_type;
    } else {
        block[offset + 6..offset + 8].copy_from_slice(&(name.len() as u16).to_le_bytes());
    }
    block[offset + 8..offset + rec_len].fill(0);
    block[offset + 8..offset + 8 + name.len()].copy_from_slice(name);
}

fn write_free_entry(block: &mut [u8], offset: usize, rec_len: usize) {
    block[offset..offset + 4].fill(0);
    block[offset + 4..offset + 6].copy_from_slice(&(rec_len as u16).to_le_bytes());
    block[offset + 6..offset + 8].fill(0);
    block[offset + 8..offset + rec_len].fill(0);
}

/// Insert `name` into a directory block, reusing a free hole or splitting an
/// active entry. Returns `false` when the block has no room.
pub fn insert_dir_entry(
    block: &mut [u8],
    name: &[u8],
    inode: u32,
    file_type: u8,
    has_filetype: bool,
) -> bool {
    let block_size = block.len();
    if name.is_empty() || name.len() > 255 || inode == 0 {
        return false;
    }
    let needed = (8 + name.len() + 3) & !3;
    if needed > block_size {
        return false;
    }
    let mut offset = 0;
    while offset < block_size {
        if offset + 8 > block_size {
            return false;
        }
        let entry_inode = u32_at(block, offset);
        let rec_len = u16_at(block, offset + 4) as usize;
        if rec_len < 8 || rec_len % 4 != 0 || offset + rec_len > block_size {
            return false;
        }
        if entry_inode == 0 {
            if rec_len >= needed {
                if rec_len - needed >= 8 {
                    write_entry(
                        block,
                        offset,
                        needed,
                        inode,
                        name,
                        file_type,
                        has_filetype,
                    );
                    write_free_entry(block, offset + needed, rec_len - needed);
                } else {
                    write_entry(
                        block,
                        offset,
                        rec_len,
                        inode,
                        name,
                        file_type,
                        has_filetype,
                    );
                }
                return true;
            }
        } else {
            let name_len = if has_filetype {
                block[offset + 6] as usize
            } else {
                u16_at(block, offset + 6) as usize
            };
            if name_len > 255 || name_len > rec_len - 8 {
                return false;
            }
            let actual_used = (8 + name_len + 3) & !3;
            if rec_len >= actual_used && rec_len - actual_used >= needed {
                let remaining = rec_len - actual_used;
                block[offset + 4..offset + 6]
                    .copy_from_slice(&(actual_used as u16).to_le_bytes());
                if remaining - needed >= 8 {
                    write_entry(
                        block,
                        offset + actual_used,
                        needed,
                        inode,
                        name,
                        file_type,
                        has_filetype,
                    );
                    write_free_entry(
                        block,
                        offset + actual_used + needed,
                        remaining - needed,
                    );
                } else {
                    write_entry(
                        block,
                        offset + actual_used,
                        remaining,
                        inode,
                        name,
                        file_type,
                        has_filetype,
                    );
                }
                return true;
            }
        }
        offset += rec_len;
    }
    false
}

/// Remove the entry at `target_offset`, merging its record with the preceding
/// entry. The first entry in a block becomes a free hole instead.
pub fn remove_dir_entry(
    block: &mut [u8],
    target_offset: usize,
    has_filetype: bool,
) -> Result<(), Ext2Error> {
    let block_size = block.len();
    if target_offset >= block_size || target_offset + 8 > block_size {
        return Err(Ext2Error::CorruptDirectory);
    }
    let (_, next) = parse_dir_entry(block, target_offset, has_filetype)?;
    let target_rec_len = next - target_offset;
    if target_offset == 0 {
        block[0..4].fill(0);
        return Ok(());
    }

    let mut offset = 0;
    let mut previous = None;
    while offset < target_offset {
        if offset + 8 > block_size {
            return Err(Ext2Error::CorruptDirectory);
        }
        let rec_len = u16_at(block, offset + 4) as usize;
        if rec_len < 8 || rec_len % 4 != 0 || offset + rec_len > block_size {
            return Err(Ext2Error::CorruptDirectory);
        }
        let next = offset + rec_len;
        if next == target_offset {
            previous = Some(offset);
            break;
        }
        if next > target_offset {
            break;
        }
        offset = next;
    }
    let previous = previous.ok_or(Ext2Error::CorruptDirectory)?;
    let previous_rec_len = u16_at(block, previous + 4) as usize;
    let merged = previous_rec_len + target_rec_len;
    if merged > u16::MAX as usize {
        return Err(Ext2Error::CorruptDirectory);
    }
    block[previous + 4..previous + 6].copy_from_slice(&(merged as u16).to_le_bytes());
    Ok(())
}

/// Rename the entry at `offset` in place when the new name still fits in the
/// record's `rec_len`. The inode number and record length are preserved.
pub fn rename_dir_entry_in_place(
    block: &mut [u8],
    offset: usize,
    name: &[u8],
    has_filetype: bool,
) -> bool {
    if name.is_empty() || name.len() > 255 {
        return false;
    }
    let Ok((entry, next)) = parse_dir_entry(block, offset, has_filetype) else {
        return false;
    };
    if entry.inode == 0 {
        return false;
    }
    let rec_len = next - offset;
    let needed = (8 + name.len() + 3) & !3;
    if needed > rec_len {
        return false;
    }
    block[offset + 8..offset + rec_len].fill(0);
    block[offset + 8..offset + 8 + name.len()].copy_from_slice(name);
    if has_filetype {
        block[offset + 6] = name.len() as u8;
    } else {
        block[offset + 6..offset + 8].copy_from_slice(&(name.len() as u16).to_le_bytes());
    }
    true
}

/// Replace the inode number of the entry at `offset`, keeping its name and
/// record length. Used to repoint a moved directory's `..` entry.
pub fn set_entry_inode(block: &mut [u8], offset: usize, inode: u32) -> Result<(), Ext2Error> {
    if inode == 0 || offset + 4 > block.len() {
        return Err(Ext2Error::CorruptDirectory);
    }
    block[offset..offset + 4].copy_from_slice(&inode.to_le_bytes());
    Ok(())
}

/// Initialise a directory block containing one free entry covering the block.
pub fn init_empty_dir_block(block: &mut [u8]) {
    let block_size = block.len();
    block.fill(0);
    if block_size >= 8 {
        block[4..6].copy_from_slice(&(block_size as u16).to_le_bytes());
    }
}

/// Initialise a new directory block with `.` and `..` entries.
pub fn init_dot_dotdot_block(
    block: &mut [u8],
    self_inode: u32,
    parent_inode: u32,
    has_filetype: bool,
) {
    let block_size = block.len();
    block.fill(0);
    if block_size < 20 {
        return;
    }
    write_entry(block, 0, 12, self_inode, b".", 2, has_filetype);
    write_entry(
        block,
        12,
        block_size - 12,
        parent_inode,
        b"..",
        2,
        has_filetype,
    );
}
