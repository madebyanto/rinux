#![allow(dead_code)]

use crate::fs::vfs::FileType;
use alloc::vec;
use alloc::vec::Vec;

use super::Ext2Error;

pub const SUPERBLOCK_OFFSET: u64 = 1024;
pub const SUPERBLOCK_SIZE: usize = 1024;
pub const GROUP_DESCRIPTOR_SIZE: usize = 32;
pub const MAX_GROUPS: u32 = 1024;
pub const MIN_INODE_SIZE: usize = 128;

/// `s_state` value for a filesystem that was cleanly unmounted.
pub const VALID_FS: u16 = 0x0001;

const COMPAT_DIR_PREALLOC: u32 = 0x1;
const COMPAT_IMAGIC_INODES: u32 = 0x2;
const COMPAT_EXT_ATTR: u32 = 0x8;
const COMPAT_RESIZE_INODE: u32 = 0x10;
const COMPAT_DIR_INDEX: u32 = 0x20;
const COMPAT_SUPPORTED: u32 = COMPAT_DIR_PREALLOC
    | COMPAT_IMAGIC_INODES
    | COMPAT_EXT_ATTR
    | COMPAT_RESIZE_INODE
    | COMPAT_DIR_INDEX;

const INCOMPAT_FILETYPE: u32 = 0x2;
const INCOMPAT_SUPPORTED: u32 = INCOMPAT_FILETYPE;

const RO_SPARSE_SUPER: u32 = 0x1;
const RO_LARGE_FILE: u32 = 0x2;
const RO_SUPPORTED: u32 = RO_SPARSE_SUPER | RO_LARGE_FILE;

pub const EXT2_INDEX_FL: u32 = 0x1000;
const EXTENTS_FL: u32 = 0x80000;
const MAGIC: u16 = 0xEF53;

pub(crate) fn u16_at(data: &[u8], offset: usize) -> u16 {
    u16::from_le_bytes([data[offset], data[offset + 1]])
}

pub(crate) fn u32_at(data: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes([
        data[offset],
        data[offset + 1],
        data[offset + 2],
        data[offset + 3],
    ])
}

fn put_u16(data: &mut [u8], offset: usize, value: u16) {
    data[offset..offset + 2].copy_from_slice(&value.to_le_bytes());
}

fn put_u32(data: &mut [u8], offset: usize, value: u32) {
    data[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
}

/// The ext2 superblock, keeping the original on-disk bytes so writeback
/// preserves fields this kernel does not interpret.
pub struct Superblock {
    raw: [u8; SUPERBLOCK_SIZE],
    inodes_count: u32,
    blocks_count: u32,
    first_data_block: u32,
    block_size: usize,
    blocks_per_group: u32,
    inodes_per_group: u32,
    inode_size: u16,
    first_ino: u32,
    free_blocks_count: u32,
    free_inodes_count: u32,
    state: u16,
    wtime: u32,
    mtime: u32,
    mnt_count: u16,
    feature_incompat: u32,
    feature_ro_compat: u32,
    volume_name: [u8; 16],
}

impl Superblock {
    pub fn parse(raw: &[u8]) -> Result<Self, Ext2Error> {
        if raw.len() < SUPERBLOCK_SIZE {
            return Err(Ext2Error::BadSuperblock);
        }
        let mut raw_block = [0u8; SUPERBLOCK_SIZE];
        raw_block.copy_from_slice(&raw[..SUPERBLOCK_SIZE]);

        let magic = u16_at(&raw_block, 56);
        if magic != MAGIC {
            return Err(Ext2Error::BadMagic(magic));
        }
        let log_block_size = u32_at(&raw_block, 24);
        if log_block_size > 2 {
            return Err(Ext2Error::UnsupportedBlockSize(log_block_size));
        }
        let block_size = 1024usize << log_block_size;
        let first_data_block = u32_at(&raw_block, 20);
        let expected_first = if block_size == 1024 { 1 } else { 0 };
        if first_data_block != expected_first {
            return Err(Ext2Error::BadSuperblock);
        }
        let inodes_count = u32_at(&raw_block, 0);
        let blocks_count = u32_at(&raw_block, 4);
        let blocks_per_group = u32_at(&raw_block, 32);
        let inodes_per_group = u32_at(&raw_block, 40);
        if inodes_count == 0 || blocks_count == 0 || blocks_per_group == 0 || inodes_per_group == 0 {
            return Err(Ext2Error::BadSuperblock);
        }
        let revision = u32_at(&raw_block, 76);
        let inode_size = if revision == 0 { 128 } else { u16_at(&raw_block, 88) };
        if inode_size < MIN_INODE_SIZE as u16
            || inode_size as usize > block_size
            || block_size % inode_size as usize != 0
        {
            return Err(Ext2Error::UnsupportedInodeSize(inode_size));
        }
        let feature_compat = u32_at(&raw_block, 92);
        let feature_incompat = u32_at(&raw_block, 96);
        let feature_ro_compat = u32_at(&raw_block, 100);
        let unsupported_compat = feature_compat & !COMPAT_SUPPORTED;
        let unsupported_incompat = feature_incompat & !INCOMPAT_SUPPORTED;
        let unsupported_ro = feature_ro_compat & !RO_SUPPORTED;
        if unsupported_compat != 0 || unsupported_incompat != 0 || unsupported_ro != 0 {
            return Err(Ext2Error::UnsupportedFeature {
                compat: unsupported_compat,
                incompat: unsupported_incompat,
                ro_compat: unsupported_ro,
            });
        }
        let groups = ((blocks_count as u64 + blocks_per_group as u64 - 1)
            / blocks_per_group as u64) as u32;
        if groups == 0 || groups > MAX_GROUPS {
            return Err(Ext2Error::BadSuperblock);
        }
        let first_ino = u32_at(&raw_block, 84);
        let first_ino = if first_ino == 0 { 11 } else { first_ino };
        if first_ino > inodes_count {
            return Err(Ext2Error::BadSuperblock);
        }
        let free_blocks_count = u32_at(&raw_block, 12);
        let free_inodes_count = u32_at(&raw_block, 16);
        if free_blocks_count > blocks_count || free_inodes_count > inodes_count {
            return Err(Ext2Error::BadSuperblock);
        }
        let mut volume_name = [0u8; 16];
        volume_name.copy_from_slice(&raw_block[120..136]);
        Ok(Self {
            raw: raw_block,
            inodes_count,
            blocks_count,
            first_data_block,
            block_size,
            blocks_per_group,
            inodes_per_group,
            inode_size,
            first_ino,
            free_blocks_count,
            free_inodes_count,
            state: u16_at(&raw_block, 58),
            wtime: u32_at(&raw_block, 48),
            mtime: u32_at(&raw_block, 44),
            mnt_count: u16_at(&raw_block, 52),
            feature_incompat,
            feature_ro_compat,
            volume_name,
        })
    }

    pub fn inodes_count(&self) -> u32 {
        self.inodes_count
    }

    pub fn blocks_count(&self) -> u32 {
        self.blocks_count
    }

    pub fn block_size(&self) -> usize {
        self.block_size
    }

    pub fn inode_size(&self) -> u16 {
        self.inode_size
    }

    pub fn blocks_per_group(&self) -> u32 {
        self.blocks_per_group
    }

    pub fn inodes_per_group(&self) -> u32 {
        self.inodes_per_group
    }

    pub fn first_data_block(&self) -> u32 {
        self.first_data_block
    }

    pub fn first_ino(&self) -> u32 {
        self.first_ino
    }

    pub fn free_blocks_count(&self) -> u32 {
        self.free_blocks_count
    }

    pub fn free_inodes_count(&self) -> u32 {
        self.free_inodes_count
    }

    pub fn state(&self) -> u16 {
        self.state
    }

    pub fn wtime(&self) -> u32 {
        self.wtime
    }

    pub fn mtime(&self) -> u32 {
        self.mtime
    }

    pub fn mnt_count(&self) -> u16 {
        self.mnt_count
    }

    pub fn groups(&self) -> u32 {
        ((self.blocks_count as u64 + self.blocks_per_group as u64 - 1)
            / self.blocks_per_group as u64) as u32
    }

    pub fn has_filetype(&self) -> bool {
        self.feature_incompat & INCOMPAT_FILETYPE != 0
    }

    pub fn has_large_file(&self) -> bool {
        self.feature_ro_compat & RO_LARGE_FILE != 0
    }

    pub fn volume_name(&self) -> &str {
        let mut end = self.volume_name.len();
        while end > 0 && (self.volume_name[end - 1] == 0 || self.volume_name[end - 1] == b' ') {
            end -= 1;
        }
        core::str::from_utf8(&self.volume_name[..end]).unwrap_or("unknown")
    }

    pub fn raw(&self) -> &[u8; SUPERBLOCK_SIZE] {
        &self.raw
    }

    pub fn set_free_blocks_count(&mut self, value: u32) {
        self.free_blocks_count = value;
        put_u32(&mut self.raw, 12, value);
    }

    pub fn set_free_inodes_count(&mut self, value: u32) {
        self.free_inodes_count = value;
        put_u32(&mut self.raw, 16, value);
    }

    pub fn set_state(&mut self, value: u16) {
        self.state = value;
        put_u16(&mut self.raw, 58, value);
    }

    pub fn set_wtime(&mut self, value: u32) {
        self.wtime = value;
        put_u32(&mut self.raw, 48, value);
    }

    pub fn set_mtime(&mut self, value: u32) {
        self.mtime = value;
        put_u32(&mut self.raw, 44, value);
    }

    pub fn set_mnt_count(&mut self, value: u16) {
        self.mnt_count = value;
        put_u16(&mut self.raw, 52, value);
    }
}

/// A group descriptor, keeping the reserved bytes intact for writeback.
#[derive(Debug, Clone, Copy)]
pub struct GroupDescriptor {
    raw: [u8; GROUP_DESCRIPTOR_SIZE],
    pub block_bitmap: u32,
    pub inode_bitmap: u32,
    pub inode_table: u32,
    pub free_blocks_count: u16,
    pub free_inodes_count: u16,
    pub used_dirs_count: u16,
    pub dirty: bool,
}

impl GroupDescriptor {
    pub fn parse(raw: &[u8]) -> Result<Self, Ext2Error> {
        if raw.len() < GROUP_DESCRIPTOR_SIZE {
            return Err(Ext2Error::BadSuperblock);
        }
        let mut block = [0u8; GROUP_DESCRIPTOR_SIZE];
        block.copy_from_slice(&raw[..GROUP_DESCRIPTOR_SIZE]);
        Ok(Self {
            raw: block,
            block_bitmap: u32_at(&block, 0),
            inode_bitmap: u32_at(&block, 4),
            inode_table: u32_at(&block, 8),
            free_blocks_count: u16_at(&block, 12),
            free_inodes_count: u16_at(&block, 14),
            used_dirs_count: u16_at(&block, 16),
            dirty: false,
        })
    }

    pub fn write_into(&self, target: &mut [u8]) {
        target[..GROUP_DESCRIPTOR_SIZE].copy_from_slice(&self.raw);
    }

    pub fn set_free_blocks_count(&mut self, value: u16) {
        self.free_blocks_count = value;
        put_u16(&mut self.raw, 12, value);
        self.dirty = true;
    }

    pub fn set_free_inodes_count(&mut self, value: u16) {
        self.free_inodes_count = value;
        put_u16(&mut self.raw, 14, value);
        self.dirty = true;
    }

    pub fn set_used_dirs_count(&mut self, value: u16) {
        self.used_dirs_count = value;
        put_u16(&mut self.raw, 16, value);
        self.dirty = true;
    }
}

#[derive(Debug, Clone)]
pub(crate) struct Inode {
    pub(crate) number: u32,
    pub(crate) mode: u16,
    pub(crate) size: u64,
    pub(crate) blocks: [u32; 15],
    raw: Vec<u8>,
}

impl Inode {
    /// A zeroed inode slot, ready to be initialised by the caller.
    pub(crate) fn empty(number: u32, inode_size: usize) -> Result<Self, Ext2Error> {
        if inode_size < MIN_INODE_SIZE {
            return Err(Ext2Error::CorruptInode);
        }
        let raw = vec![0u8; inode_size];
        parse_inode(number, &raw, false)
    }

    pub(crate) fn kind(&self) -> FileType {
        match self.mode & 0xF000 {
            0x8000 => FileType::Regular,
            0x4000 => FileType::Directory,
            _ => FileType::Other,
        }
    }

    pub(crate) fn is_dir(&self) -> bool {
        self.kind() == FileType::Directory
    }

    /// The untouched on-disk bytes, including fields this kernel does not
    /// interpret. `write_inode` copies them back verbatim.
    pub(crate) fn raw(&self) -> &[u8] {
        &self.raw
    }

    /// `i_flags`, including `EXT2_INDEX_FL`.
    pub(crate) fn flags(&self) -> u32 {
        u32_at(&self.raw, 32)
    }

    pub(crate) fn block(&self, index: usize) -> u32 {
        self.blocks[index]
    }

    pub(crate) fn set_block(&mut self, index: usize, block: u32) {
        self.blocks[index] = block;
        put_u32(&mut self.raw, 40 + index * 4, block);
    }

    pub(crate) fn set_mode(&mut self, mode: u16) {
        self.mode = mode;
        put_u16(&mut self.raw, 0, mode);
    }

    pub(crate) fn set_links_count(&mut self, links: u16) {
        put_u16(&mut self.raw, 26, links);
    }

    /// Re-apply the decoded fields to the preserved raw bytes before writeback.
    pub(crate) fn sync_raw(&mut self, large_file: bool) -> Result<(), Ext2Error> {
        if !large_file && self.size > u32::MAX as u64 {
            return Err(Ext2Error::FileTooLarge);
        }
        put_u16(&mut self.raw, 0, self.mode);
        put_u32(&mut self.raw, 4, self.size as u32);
        if self.kind() == FileType::Regular && large_file {
            put_u32(&mut self.raw, 108, (self.size >> 32) as u32);
        }
        for (index, block) in self.blocks.iter().enumerate() {
            put_u32(&mut self.raw, 40 + index * 4, *block);
        }
        Ok(())
    }

    /// `i_blocks` counts 512-byte sectors, including indirect blocks.
    pub(crate) fn add_sectors(&mut self, sectors: u32) {
        let total = u32_at(&self.raw, 28).saturating_add(sectors);
        put_u32(&mut self.raw, 28, total);
    }

    pub(crate) fn sub_sectors(&mut self, sectors: u32) {
        let total = u32_at(&self.raw, 28).saturating_sub(sectors);
        put_u32(&mut self.raw, 28, total);
    }

    pub(crate) fn set_size(&mut self, size: u64, large_file: bool) -> Result<(), Ext2Error> {
        if !large_file && size > u32::MAX as u64 {
            return Err(Ext2Error::FileTooLarge);
        }
        self.size = size;
        put_u32(&mut self.raw, 4, size as u32);
        if self.kind() == FileType::Regular && large_file {
            put_u32(&mut self.raw, 108, (size >> 32) as u32);
        }
        Ok(())
    }

    pub(crate) fn set_atime(&mut self, time: u32) {
        put_u32(&mut self.raw, 8, time);
    }

    pub(crate) fn set_mtime(&mut self, time: u32) {
        put_u32(&mut self.raw, 16, time);
    }

    pub(crate) fn set_ctime(&mut self, time: u32) {
        put_u32(&mut self.raw, 12, time);
    }

    pub(crate) fn set_dtime(&mut self, time: u32) {
        put_u32(&mut self.raw, 20, time);
    }
}

pub(crate) fn parse_inode(
    number: u32,
    raw: &[u8],
    large_file: bool,
) -> Result<Inode, Ext2Error> {
    if raw.len() < MIN_INODE_SIZE {
        return Err(Ext2Error::CorruptInode);
    }
    let mode = u16_at(raw, 0);
    let size_low = u32_at(raw, 4) as u64;
    let flags = u32_at(raw, 32);
    if flags & EXTENTS_FL != 0 {
        return Err(Ext2Error::UnsupportedInode);
    }
    let mut blocks = [0u32; 15];
    for (index, block) in blocks.iter_mut().enumerate() {
        *block = u32_at(raw, 40 + index * 4);
    }
    let size = if mode & 0xF000 == 0x8000 && large_file {
        size_low | ((u32_at(raw, 108) as u64) << 32)
    } else {
        size_low
    };
    Ok(Inode {
        number,
        mode,
        size,
        blocks,
        raw: raw.to_vec(),
    })
}
