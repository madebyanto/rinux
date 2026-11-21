pub mod bitmap;
pub mod dir;
pub mod inode;
pub mod layout;
pub mod structs;

pub use dir::parse_dir_entry;
pub use inode::{block_path, BlockPath};
pub use structs::{GroupDescriptor, Superblock};

use super::vfs::{DirCursor, DirEntry, FileSystem, FileType, NodeId, Stat, VfsError};
use ::alloc::string::String;
use ::alloc::vec;
use ::alloc::vec::Vec;
use bitmap::Bitmap;
use inode::read_pointer;
use structs::{
    parse_inode, Inode, GROUP_DESCRIPTOR_SIZE, SUPERBLOCK_OFFSET, SUPERBLOCK_SIZE, VALID_FS,
};

pub const MAX_PATH: usize = 256;
const CACHE_ENTRIES: usize = 16;

#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ext2Error {
    Device,
    ReadOnly,
    NoSpace,
    BadMagic(u16),
    BadSuperblock,
    UnsupportedFeature {
        compat: u32,
        incompat: u32,
        ro_compat: u32,
    },
    UnsupportedBlockSize(u32),
    UnsupportedInodeSize(u16),
    UnsupportedInode,
    OutOfBounds,
    InodeOutOfRange(u32),
    NotFound,
    NotDirectory,
    NotFile,
    CorruptDirectory,
    CorruptInode,
    InvalidBlock,
    InvalidPath,
}

impl core::fmt::Display for Ext2Error {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Ext2Error::Device => write!(formatter, "block device error"),
            Ext2Error::ReadOnly => write!(formatter, "filesystem is mounted read-only"),
            Ext2Error::NoSpace => write!(formatter, "no space left on device"),
            Ext2Error::BadMagic(magic) => write!(formatter, "bad ext2 magic {:#x}", magic),
            Ext2Error::BadSuperblock => write!(formatter, "invalid ext2 superblock"),
            Ext2Error::UnsupportedFeature {
                compat,
                incompat,
                ro_compat,
            } => write!(
                formatter,
                "unsupported ext2 features compat={:#x} incompat={:#x} ro_compat={:#x}",
                compat, incompat, ro_compat
            ),
            Ext2Error::UnsupportedBlockSize(log) => {
                write!(formatter, "unsupported ext2 block-size log {}", log)
            }
            Ext2Error::UnsupportedInodeSize(size) => {
                write!(formatter, "unsupported ext2 inode size {}", size)
            }
            Ext2Error::UnsupportedInode => write!(formatter, "unsupported ext2 inode feature"),
            Ext2Error::OutOfBounds => write!(formatter, "ext2 block out of bounds"),
            Ext2Error::InodeOutOfRange(node) => {
                write!(formatter, "ext2 inode {} out of range", node)
            }
            Ext2Error::NotFound => write!(formatter, "not found"),
            Ext2Error::NotDirectory => write!(formatter, "not a directory"),
            Ext2Error::NotFile => write!(formatter, "not a regular file"),
            Ext2Error::CorruptDirectory => write!(formatter, "corrupt ext2 directory"),
            Ext2Error::CorruptInode => write!(formatter, "corrupt ext2 inode"),
            Ext2Error::InvalidBlock => write!(formatter, "invalid ext2 file block"),
            Ext2Error::InvalidPath => write!(formatter, "invalid path"),
        }
    }
}

impl From<Ext2Error> for VfsError {
    fn from(error: Ext2Error) -> Self {
        match error {
            Ext2Error::Device => VfsError::Device,
            Ext2Error::ReadOnly | Ext2Error::NoSpace => VfsError::Unsupported,
            Ext2Error::NotFound => VfsError::NotFound,
            Ext2Error::NotDirectory => VfsError::NotDirectory,
            Ext2Error::NotFile => VfsError::NotFile,
            Ext2Error::InvalidPath => VfsError::InvalidPath,
            Ext2Error::UnsupportedFeature { .. }
            | Ext2Error::UnsupportedBlockSize(_)
            | Ext2Error::UnsupportedInodeSize(_)
            | Ext2Error::UnsupportedInode => VfsError::Unsupported,
            _ => VfsError::Corrupt,
        }
    }
}

pub trait BlockDevice {
    fn read_at(&mut self, offset: u64, buffer: &mut [u8]) -> Result<(), Ext2Error>;

    /// Write a block-aligned buffer. Devices that cannot be written default to
    /// `ReadOnly`, which keeps the existing read-only host test device simple.
    fn write_at(&mut self, _offset: u64, _buffer: &[u8]) -> Result<(), Ext2Error> {
        Err(Ext2Error::ReadOnly)
    }

    /// Flush any device-side write cache.
    #[allow(dead_code)]
    fn flush(&mut self) -> Result<(), Ext2Error> {
        Ok(())
    }
}

pub type Clock = fn() -> u32;

#[cfg(target_os = "none")]
fn default_clock() -> u32 {
    (crate::arch::x86_64::pit::uptime_ms() / 1000) as u32
}

#[cfg(not(target_os = "none"))]
fn default_clock() -> u32 {
    0
}

struct CacheEntry {
    block: u64,
    valid: bool,
    data: Vec<u8>,
}

struct BlockCache {
    entries: [CacheEntry; CACHE_ENTRIES],
    next: usize,
    hits: u64,
    misses: u64,
}

impl BlockCache {
    fn new(block_size: usize) -> Self {
        Self {
            entries: core::array::from_fn(|_| CacheEntry {
                block: 0,
                valid: false,
                data: vec![0u8; block_size],
            }),
            next: 0,
            hits: 0,
            misses: 0,
        }
    }

    fn get<D: BlockDevice>(
        &mut self,
        device: &mut D,
        block: u64,
        block_size: usize,
    ) -> Result<&[u8], Ext2Error> {
        let mut found = None;
        for (index, entry) in self.entries.iter().enumerate() {
            if entry.valid && entry.block == block {
                found = Some(index);
                break;
            }
        }
        if let Some(index) = found {
            self.hits += 1;
            return Ok(&self.entries[index].data);
        }
        let index = self.next;
        self.next = (index + 1) % CACHE_ENTRIES;
        self.misses += 1;
        let entry = &mut self.entries[index];
        device.read_at(block * block_size as u64, &mut entry.data)?;
        entry.block = block;
        entry.valid = true;
        Ok(&entry.data)
    }

    fn read_vec<D: BlockDevice>(
        &mut self,
        device: &mut D,
        block: u64,
        block_size: usize,
    ) -> Result<Vec<u8>, Ext2Error> {
        Ok(self.get(device, block, block_size)?.to_vec())
    }

    fn write<D: BlockDevice>(
        &mut self,
        device: &mut D,
        block: u64,
        block_size: usize,
        data: &[u8],
    ) -> Result<(), Ext2Error> {
        if data.len() != block_size {
            return Err(Ext2Error::InvalidBlock);
        }
        device.write_at(block * block_size as u64, data)?;
        for entry in self.entries.iter_mut() {
            if entry.valid && entry.block == block {
                entry.data.copy_from_slice(data);
                return Ok(());
            }
        }
        let index = self.next;
        self.next = (index + 1) % CACHE_ENTRIES;
        let entry = &mut self.entries[index];
        entry.data.copy_from_slice(data);
        entry.block = block;
        entry.valid = true;
        Ok(())
    }

    #[allow(dead_code)]
    fn stats(&self) -> (u64, u64) {
        (self.hits, self.misses)
    }
}

pub struct Ext2<D: BlockDevice> {
    device: D,
    superblock: Superblock,
    groups: Vec<GroupDescriptor>,
    cache: BlockCache,
    has_filetype: bool,
    read_only: bool,
    clock: Clock,
}

#[allow(dead_code)]
impl<D: BlockDevice> Ext2<D> {
    /// Mount read-only. Kept as the default for existing callers.
    pub fn mount(device: D) -> Result<Self, Ext2Error> {
        Self::mount_with(device, true, default_clock)
    }

    /// Mount read-write. The kernel uses this path.
    pub fn mount_rw(device: D) -> Result<Self, Ext2Error> {
        Self::mount_with(device, false, default_clock)
    }

    /// Read-write mount with an injected clock, used by host tests.
    pub fn mount_rw_with_clock(device: D, clock: Clock) -> Result<Self, Ext2Error> {
        Self::mount_with(device, false, clock)
    }

    fn mount_with(mut device: D, read_only: bool, clock: Clock) -> Result<Self, Ext2Error> {
        let mut raw_superblock = [0u8; SUPERBLOCK_SIZE];
        device.read_at(SUPERBLOCK_OFFSET, &mut raw_superblock)?;
        let superblock = Superblock::parse(&raw_superblock)?;
        let mut cache = BlockCache::new(superblock.block_size());
        let groups = read_groups(&mut device, &mut cache, &superblock)?;
        let has_filetype = superblock.has_filetype();
        let mut filesystem = Self {
            device,
            superblock,
            groups,
            cache,
            has_filetype,
            read_only,
            clock,
        };
        let root = filesystem.read_inode(2)?;
        if !root.is_dir() {
            return Err(Ext2Error::CorruptInode);
        }
        if !read_only {
            // Mark the filesystem dirty while it is mounted. `sync` restores
            // VALID_FS once all metadata has been written back.
            filesystem.superblock.set_state(0);
            filesystem.write_superblock()?;
        }
        Ok(filesystem)
    }

    pub fn superblock(&self) -> &Superblock {
        &self.superblock
    }

    pub fn is_read_only(&self) -> bool {
        self.read_only
    }

    pub fn device(&self) -> &D {
        &self.device
    }

    pub fn device_mut(&mut self) -> &mut D {
        &mut self.device
    }

    pub fn into_device(self) -> D {
        self.device
    }

    pub fn groups_count(&self) -> usize {
        self.groups.len()
    }

    pub fn group_descriptor(&self, group: usize) -> Option<&GroupDescriptor> {
        self.groups.get(group)
    }

    #[allow(dead_code)]
    pub fn cache_stats(&self) -> (u64, u64) {
        self.cache.stats()
    }

    /// Allocate one block, preferring `preferred_group` and falling back to a
    /// scan of the other groups. The returned block is not zeroed.
    pub fn allocate_block(&mut self, preferred_group: usize) -> Result<u32, Ext2Error> {
        self.require_writable()?;
        let group_count = self.groups.len();
        if group_count == 0 {
            return Err(Ext2Error::NoSpace);
        }
        let start = preferred_group.min(group_count - 1);
        for offset in 0..group_count {
            let group = (start + offset) % group_count;
            let valid = layout::blocks_in_group(&self.superblock, group as u32);
            if valid == 0 || self.groups[group].free_blocks_count == 0 {
                continue;
            }
            let block_bitmap = self.groups[group].block_bitmap;
            let mut bitmap = Bitmap::from_bytes(self.read_block_vec(block_bitmap as u64)?);
            let Some(bit) = bitmap.first_free(valid, 0) else {
                if self.groups[group].free_blocks_count != 0 {
                    self.groups[group].set_free_blocks_count(0);
                    self.write_group(group)?;
                }
                continue;
            };
            bitmap.set(bit);
            self.write_block(block_bitmap as u64, bitmap.as_bytes())?;
            let free = self.groups[group].free_blocks_count.saturating_sub(1);
            self.groups[group].set_free_blocks_count(free);
            let total = self.superblock.free_blocks_count().saturating_sub(1);
            self.superblock.set_free_blocks_count(total);
            self.write_group(group)?;
            self.write_superblock()?;
            return layout::block_number(&self.superblock, group as u32, bit)
                .ok_or(Ext2Error::CorruptInode);
        }
        Err(Ext2Error::NoSpace)
    }

    /// Free a block previously returned by [`Self::allocate_block`].
    pub fn free_block(&mut self, block: u32) -> Result<(), Ext2Error> {
        self.require_writable()?;
        let (group, bit) =
            layout::block_location(&self.superblock, block).ok_or(Ext2Error::InvalidBlock)?;
        let group = group as usize;
        let block_bitmap = self
            .groups
            .get(group)
            .ok_or(Ext2Error::CorruptInode)?
            .block_bitmap;
        let mut bitmap = Bitmap::from_bytes(self.read_block_vec(block_bitmap as u64)?);
        if !bitmap.is_set(bit) {
            return Err(Ext2Error::InvalidBlock);
        }
        bitmap.clear(bit);
        self.write_block(block_bitmap as u64, bitmap.as_bytes())?;
        let valid = layout::blocks_in_group(&self.superblock, group as u32);
        let free = self.groups[group]
            .free_blocks_count
            .saturating_add(1)
            .min(valid.min(u16::MAX as u32) as u16);
        self.groups[group].set_free_blocks_count(free);
        let total = self
            .superblock
            .free_blocks_count()
            .saturating_add(1)
            .min(self.superblock.blocks_count());
        self.superblock.set_free_blocks_count(total);
        self.write_group(group)?;
        self.write_superblock()?;
        Ok(())
    }

    /// Allocate one inode. The inode is not initialised; callers write its
    /// mode, link count and block pointers before making it reachable.
    pub fn allocate_inode(
        &mut self,
        preferred_group: usize,
        is_dir: bool,
    ) -> Result<u32, Ext2Error> {
        self.require_writable()?;
        let group_count = self.groups.len();
        if group_count == 0 {
            return Err(Ext2Error::NoSpace);
        }
        let start = preferred_group.min(group_count - 1);
        for offset in 0..group_count {
            let group = (start + offset) % group_count;
            let valid = layout::inodes_in_group(&self.superblock, group as u32);
            if valid == 0 || self.groups[group].free_inodes_count == 0 {
                continue;
            }
            let inode_bitmap = self.groups[group].inode_bitmap;
            let mut bitmap = Bitmap::from_bytes(self.read_block_vec(inode_bitmap as u64)?);
            let min_bit = layout::first_inode_bit(&self.superblock, group as u32);
            let Some(bit) = bitmap.first_free(valid, min_bit) else {
                if self.groups[group].free_inodes_count != 0 {
                    self.groups[group].set_free_inodes_count(0);
                    self.write_group(group)?;
                }
                continue;
            };
            bitmap.set(bit);
            self.write_block(inode_bitmap as u64, bitmap.as_bytes())?;
            let free = self.groups[group].free_inodes_count.saturating_sub(1);
            self.groups[group].set_free_inodes_count(free);
            if is_dir {
                let used = self.groups[group].used_dirs_count.saturating_add(1);
                self.groups[group].set_used_dirs_count(used);
            }
            let total = self.superblock.free_inodes_count().saturating_sub(1);
            self.superblock.set_free_inodes_count(total);
            self.write_group(group)?;
            self.write_superblock()?;
            return layout::inode_number(&self.superblock, group as u32, bit)
                .ok_or(Ext2Error::CorruptInode);
        }
        Err(Ext2Error::NoSpace)
    }

    /// Free an inode previously returned by [`Self::allocate_inode`].
    pub fn free_inode(&mut self, inode: u32, is_dir: bool) -> Result<(), Ext2Error> {
        self.require_writable()?;
        if inode < self.superblock.first_ino().max(2) {
            return Err(Ext2Error::CorruptInode);
        }
        let (group, bit) =
            layout::inode_location(&self.superblock, inode).ok_or(Ext2Error::InodeOutOfRange(inode))?;
        let group = group as usize;
        let inode_bitmap = self
            .groups
            .get(group)
            .ok_or(Ext2Error::CorruptInode)?
            .inode_bitmap;
        let mut bitmap = Bitmap::from_bytes(self.read_block_vec(inode_bitmap as u64)?);
        if !bitmap.is_set(bit) {
            return Err(Ext2Error::CorruptInode);
        }
        bitmap.clear(bit);
        self.write_block(inode_bitmap as u64, bitmap.as_bytes())?;
        let valid = layout::inodes_in_group(&self.superblock, group as u32);
        let free = self.groups[group]
            .free_inodes_count
            .saturating_add(1)
            .min(valid.min(u16::MAX as u32) as u16);
        self.groups[group].set_free_inodes_count(free);
        if is_dir {
            let used = self.groups[group].used_dirs_count.saturating_sub(1);
            self.groups[group].set_used_dirs_count(used);
        }
        let total = self
            .superblock
            .free_inodes_count()
            .saturating_add(1)
            .min(self.superblock.inodes_count());
        self.superblock.set_free_inodes_count(total);
        self.write_group(group)?;
        self.write_superblock()?;
        Ok(())
    }

    pub fn is_block_allocated(&mut self, block: u32) -> Result<bool, Ext2Error> {
        let (group, bit) =
            layout::block_location(&self.superblock, block).ok_or(Ext2Error::InvalidBlock)?;
        let block_bitmap = self
            .groups
            .get(group as usize)
            .ok_or(Ext2Error::CorruptInode)?
            .block_bitmap;
        let bitmap = Bitmap::from_bytes(self.read_block_vec(block_bitmap as u64)?);
        Ok(bitmap.is_set(bit))
    }

    pub fn is_inode_allocated(&mut self, inode: u32) -> Result<bool, Ext2Error> {
        let (group, bit) =
            layout::inode_location(&self.superblock, inode).ok_or(Ext2Error::InodeOutOfRange(inode))?;
        let inode_bitmap = self
            .groups
            .get(group as usize)
            .ok_or(Ext2Error::CorruptInode)?
            .inode_bitmap;
        let bitmap = Bitmap::from_bytes(self.read_block_vec(inode_bitmap as u64)?);
        Ok(bitmap.is_set(bit))
    }

    /// Write dirty group descriptors, mark the superblock clean, and flush.
    pub fn sync(&mut self) -> Result<(), Ext2Error> {
        if self.read_only {
            return Ok(());
        }
        for group in 0..self.groups.len() {
            if self.groups[group].dirty {
                self.write_group(group)?;
            }
        }
        let now = self.now();
        self.superblock.set_wtime(now);
        self.superblock.set_state(VALID_FS);
        self.write_superblock()?;
        self.device.flush()
    }

    fn now(&self) -> u32 {
        (self.clock)()
    }

    fn require_writable(&self) -> Result<(), Ext2Error> {
        if self.read_only {
            Err(Ext2Error::ReadOnly)
        } else {
            Ok(())
        }
    }

    fn write_superblock(&mut self) -> Result<(), Ext2Error> {
        let block_size = self.superblock.block_size();
        let block = SUPERBLOCK_OFFSET / block_size as u64;
        let within = SUPERBLOCK_OFFSET as usize % block_size;
        let mut data = self.read_block_vec(block)?;
        if within + SUPERBLOCK_SIZE > data.len() {
            return Err(Ext2Error::BadSuperblock);
        }
        data[within..within + SUPERBLOCK_SIZE].copy_from_slice(self.superblock.raw());
        self.write_block(block, &data)
    }

    fn write_group(&mut self, group: usize) -> Result<(), Ext2Error> {
        let descriptor = *self
            .groups
            .get(group)
            .ok_or(Ext2Error::CorruptInode)?;
        if !descriptor.dirty {
            return Ok(());
        }
        let block_size = self.superblock.block_size();
        let table_block = (self.superblock.first_data_block() as u64) + 1;
        let offset = group * GROUP_DESCRIPTOR_SIZE;
        let block = table_block + (offset / block_size) as u64;
        let within = offset % block_size;
        let mut data = self.read_block_vec(block)?;
        if within + GROUP_DESCRIPTOR_SIZE > data.len() {
            return Err(Ext2Error::CorruptInode);
        }
        descriptor.write_into(&mut data[within..within + GROUP_DESCRIPTOR_SIZE]);
        self.write_block(block, &data)?;
        self.groups[group].dirty = false;
        Ok(())
    }

    pub fn lookup(&mut self, path: &str) -> Result<NodeId, Ext2Error> {
        if path.len() > MAX_PATH {
            return Err(Ext2Error::InvalidPath);
        }
        let mut current = self.read_inode(2)?;
        for component in path.split('/').filter(|component| !component.is_empty()) {
            if component == "." {
                continue;
            }
            if component == ".." {
                let node = self.find_entry(&current, "..")?;
                current = self.read_inode(node)?;
                continue;
            }
            if component.len() > 255 {
                return Err(Ext2Error::InvalidPath);
            }
            let node = self.find_entry(&current, component)?;
            current = self.read_inode(node)?;
        }
        Ok(current.number)
    }

    pub fn stat_inode(&mut self, node: NodeId) -> Result<Stat, Ext2Error> {
        let inode = self.read_inode(node)?;
        Ok(Stat {
            node,
            size: inode.size,
            mode: inode.mode,
            kind: inode.kind(),
        })
    }

    pub fn read_file(
        &mut self,
        node: NodeId,
        offset: u64,
        buffer: &mut [u8],
    ) -> Result<usize, Ext2Error> {
        let inode = self.read_inode(node)?;
        if inode.kind() != FileType::Regular {
            return Err(Ext2Error::NotFile);
        }
        if offset >= inode.size || buffer.is_empty() {
            return Ok(0);
        }
        let block_size = self.superblock.block_size();
        let mut done = 0;
        while done < buffer.len() && offset + (done as u64) < inode.size {
            let current = offset + done as u64;
            let logical = current / block_size as u64;
            let within = (current % block_size as u64) as usize;
            let remaining_file = (inode.size - current) as usize;
            let available = (block_size - within)
                .min(buffer.len() - done)
                .min(remaining_file);
            let disk_block = self.map_block(&inode, logical)?;
            if disk_block == 0 {
                buffer[done..done + available].fill(0);
            } else {
                let data = self.block(disk_block as u64)?;
                buffer[done..done + available].copy_from_slice(&data[within..within + available]);
            }
            done += available;
        }
        Ok(done)
    }

    pub fn read_dir(
        &mut self,
        node: NodeId,
        cursor: &mut DirCursor,
    ) -> Result<Option<DirEntry>, Ext2Error> {
        let inode = self.read_inode(node)?;
        if !inode.is_dir() {
            return Err(Ext2Error::NotDirectory);
        }
        let block_size = self.superblock.block_size() as u64;
        let has_filetype = self.has_filetype;
        let inodes_count = self.superblock.inodes_count();
        loop {
            if cursor.block * block_size >= inode.size {
                return Ok(None);
            }
            let disk_block = self.map_block(&inode, cursor.block)?;
            if disk_block == 0 {
                return Err(Ext2Error::CorruptDirectory);
            }
            let data = self.block(disk_block as u64)?;
            let offset = cursor.offset as usize;
            if offset >= data.len() {
                cursor.block += 1;
                cursor.offset = 0;
                continue;
            }
            let (raw, next) = parse_dir_entry(data, offset, has_filetype)?;
            if next >= data.len() {
                cursor.block += 1;
                cursor.offset = 0;
            } else {
                cursor.offset = next as u16;
            }
            if raw.inode == 0 {
                continue;
            }
            if raw.inode > inodes_count {
                return Err(Ext2Error::CorruptDirectory);
            }
            let name = core::str::from_utf8(&data[raw.name_start..raw.name_start + raw.name_len])
                .map_err(|_| Ext2Error::CorruptDirectory)?;
            let kind = match raw.file_type {
                1 => FileType::Regular,
                2 => FileType::Directory,
                _ => FileType::Other,
            };
            return Ok(Some(DirEntry {
                node: raw.inode,
                name: String::from(name),
                kind,
            }));
        }
    }

    fn find_entry(&mut self, directory: &Inode, name: &str) -> Result<u32, Ext2Error> {
        if !directory.is_dir() {
            return Err(Ext2Error::NotDirectory);
        }
        let mut cursor = DirCursor::new();
        while let Some(entry) = self.read_dir(directory.number, &mut cursor)? {
            if entry.name == name {
                return Ok(entry.node);
            }
        }
        Err(Ext2Error::NotFound)
    }

    fn read_inode(&mut self, number: u32) -> Result<Inode, Ext2Error> {
        if number == 0 || number > self.superblock.inodes_count() {
            return Err(Ext2Error::InodeOutOfRange(number));
        }
        let index = (number - 1) as u64;
        let group = (index / self.superblock.inodes_per_group() as u64) as usize;
        let within_group = index % self.superblock.inodes_per_group() as u64;
        let descriptor = self
            .groups
            .get(group)
            .ok_or(Ext2Error::InodeOutOfRange(number))?;
        let inode_size = self.superblock.inode_size() as u64;
        let table_offset = descriptor.inode_table as u64 * self.superblock.block_size() as u64;
        let offset = table_offset + within_group * inode_size;
        let block_index = offset / self.superblock.block_size() as u64;
        let within_block = (offset % self.superblock.block_size() as u64) as usize;
        let size = self.superblock.inode_size() as usize;
        if within_block + size > self.superblock.block_size() {
            return Err(Ext2Error::CorruptInode);
        }
        let large_file = self.superblock.has_large_file();
        let data = self.block(block_index)?;
        parse_inode(number, &data[within_block..within_block + size], large_file)
    }

    fn block(&mut self, block: u64) -> Result<&[u8], Ext2Error> {
        if block >= self.superblock.blocks_count() as u64 {
            return Err(Ext2Error::OutOfBounds);
        }
        let block_size = self.superblock.block_size();
        self.cache.get(&mut self.device, block, block_size)
    }

    fn read_block_vec(&mut self, block: u64) -> Result<Vec<u8>, Ext2Error> {
        if block >= self.superblock.blocks_count() as u64 {
            return Err(Ext2Error::OutOfBounds);
        }
        let block_size = self.superblock.block_size();
        self.cache.read_vec(&mut self.device, block, block_size)
    }

    fn write_block(&mut self, block: u64, data: &[u8]) -> Result<(), Ext2Error> {
        self.require_writable()?;
        if block >= self.superblock.blocks_count() as u64 {
            return Err(Ext2Error::OutOfBounds);
        }
        let block_size = self.superblock.block_size();
        self.cache.write(&mut self.device, block, block_size, data)
    }

    fn map_block(&mut self, inode: &Inode, logical: u64) -> Result<u32, Ext2Error> {
        let pointers = (self.superblock.block_size() / 4) as u64;
        match block_path(logical, pointers) {
            BlockPath::Direct(index) => Ok(inode.blocks[index]),
            BlockPath::Singly(index) => {
                let indirect = inode.blocks[12];
                if indirect == 0 {
                    return Ok(0);
                }
                let data = self.block(indirect as u64)?;
                Ok(read_pointer(data, index as usize))
            }
            BlockPath::Doubly(first, second) => {
                let indirect = inode.blocks[13];
                if indirect == 0 {
                    return Ok(0);
                }
                let data = self.block(indirect as u64)?;
                let next = read_pointer(data, first as usize);
                if next == 0 {
                    return Ok(0);
                }
                let data = self.block(next as u64)?;
                Ok(read_pointer(data, second as usize))
            }
            BlockPath::Triply(first, second, third) => {
                let indirect = inode.blocks[14];
                if indirect == 0 {
                    return Ok(0);
                }
                let data = self.block(indirect as u64)?;
                let next = read_pointer(data, first as usize);
                if next == 0 {
                    return Ok(0);
                }
                let data = self.block(next as u64)?;
                let next = read_pointer(data, second as usize);
                if next == 0 {
                    return Ok(0);
                }
                let data = self.block(next as u64)?;
                Ok(read_pointer(data, third as usize))
            }
            BlockPath::OutOfRange => Err(Ext2Error::InvalidBlock),
        }
    }
}

fn read_groups<D: BlockDevice>(
    device: &mut D,
    cache: &mut BlockCache,
    superblock: &Superblock,
) -> Result<Vec<GroupDescriptor>, Ext2Error> {
    let groups = superblock.groups();
    let total = groups as usize * GROUP_DESCRIPTOR_SIZE;
    let table_block = (superblock.first_data_block() + 1) as u64;
    let table_blocks = (total + superblock.block_size() - 1) / superblock.block_size();
    let mut raw = Vec::with_capacity(total);
    for index in 0..table_blocks {
        let block = table_block + index as u64;
        if block >= superblock.blocks_count() as u64 {
            return Err(Ext2Error::OutOfBounds);
        }
        let data = cache.get(device, block, superblock.block_size())?;
        let remaining = total - raw.len();
        let take = remaining.min(superblock.block_size());
        raw.extend_from_slice(&data[..take]);
    }
    let mut descriptors = Vec::with_capacity(groups as usize);
    for index in 0..groups as usize {
        let descriptor = GroupDescriptor::parse(&raw[index * GROUP_DESCRIPTOR_SIZE..])?;
        if descriptor.block_bitmap == 0
            || descriptor.block_bitmap >= superblock.blocks_count()
            || descriptor.inode_bitmap == 0
            || descriptor.inode_bitmap >= superblock.blocks_count()
            || descriptor.inode_table == 0
            || descriptor.inode_table >= superblock.blocks_count()
        {
            return Err(Ext2Error::CorruptInode);
        }
        descriptors.push(descriptor);
    }
    Ok(descriptors)
}

impl<D: BlockDevice + Send> FileSystem for Ext2<D> {
    fn open(&mut self, path: &str) -> Result<NodeId, VfsError> {
        self.lookup(path).map_err(VfsError::from)
    }

    fn stat(&mut self, node: NodeId) -> Result<Stat, VfsError> {
        self.stat_inode(node).map_err(VfsError::from)
    }

    fn read(&mut self, node: NodeId, offset: u64, buffer: &mut [u8]) -> Result<usize, VfsError> {
        self.read_file(node, offset, buffer).map_err(VfsError::from)
    }

    fn readdir(
        &mut self,
        node: NodeId,
        cursor: &mut DirCursor,
    ) -> Result<Option<DirEntry>, VfsError> {
        self.read_dir(node, cursor).map_err(VfsError::from)
    }
}
