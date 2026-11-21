#![allow(dead_code)]

//! Pure block/inode group arithmetic.
//!
//! The device-backed allocator in [`super::Ext2`] uses these helpers so the
//! tricky parts — the `first_data_block` offset and last-group clamping — are
//! testable without a disk.

use super::structs::Superblock;

/// Number of real blocks in `group`.
pub fn blocks_in_group(superblock: &Superblock, group: u32) -> u32 {
    let first =
        group as u64 * superblock.blocks_per_group() as u64 + superblock.first_data_block() as u64;
    if first >= superblock.blocks_count() as u64 {
        return 0;
    }
    let remaining = superblock.blocks_count() as u64 - first;
    remaining.min(superblock.blocks_per_group() as u64) as u32
}

/// Number of real inodes in `group`.
pub fn inodes_in_group(superblock: &Superblock, group: u32) -> u32 {
    let first = group as u64 * superblock.inodes_per_group() as u64;
    if first >= superblock.inodes_count() as u64 {
        return 0;
    }
    let remaining = superblock.inodes_count() as u64 - first;
    remaining.min(superblock.inodes_per_group() as u64) as u32
}

/// Convert an absolute block number to `(group, bit)`.
pub fn block_location(superblock: &Superblock, block: u32) -> Option<(u32, u32)> {
    if block < superblock.first_data_block() || block >= superblock.blocks_count() {
        return None;
    }
    let index = block - superblock.first_data_block();
    let group = index / superblock.blocks_per_group();
    if group >= superblock.groups() {
        return None;
    }
    Some((group, index % superblock.blocks_per_group()))
}

/// Convert `(group, bit)` to an absolute block number.
pub fn block_number(superblock: &Superblock, group: u32, bit: u32) -> Option<u32> {
    if bit >= blocks_in_group(superblock, group) {
        return None;
    }
    let block = group as u64 * superblock.blocks_per_group() as u64
        + superblock.first_data_block() as u64
        + bit as u64;
    if block >= superblock.blocks_count() as u64 {
        return None;
    }
    Some(block as u32)
}

/// Convert an absolute inode number to `(group, bit)`.
pub fn inode_location(superblock: &Superblock, inode: u32) -> Option<(u32, u32)> {
    if inode == 0 || inode > superblock.inodes_count() {
        return None;
    }
    let index = inode - 1;
    let group = index / superblock.inodes_per_group();
    if group >= superblock.groups() {
        return None;
    }
    Some((group, index % superblock.inodes_per_group()))
}

/// Convert `(group, bit)` to an absolute inode number.
pub fn inode_number(superblock: &Superblock, group: u32, bit: u32) -> Option<u32> {
    if bit >= inodes_in_group(superblock, group) {
        return None;
    }
    let inode = group as u64 * superblock.inodes_per_group() as u64 + bit as u64 + 1;
    if inode > superblock.inodes_count() as u64 {
        return None;
    }
    Some(inode as u32)
}

/// Lowest inode bit the allocator may use in `group`.
///
/// Group 0 contains the reserved inodes before `s_first_ino`.
pub fn first_inode_bit(superblock: &Superblock, group: u32) -> u32 {
    if group == 0 {
        superblock.first_ino().saturating_sub(1)
    } else {
        0
    }
}
