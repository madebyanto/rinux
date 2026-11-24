pub mod bitmap;
pub mod dir;
pub mod inode;
pub mod layout;
pub mod structs;

pub use dir::parse_dir_entry;
pub use inode::{block_path, BlockPath};
pub use structs::{GroupDescriptor, Superblock};

use super::vfs::{
    DirCursor, DirEntry, FileSystem, FileType, NodeId, Stat, VfsError, O_ACCMODE, O_CREAT,
    O_EXCL, O_RDONLY, O_TRUNC,
};
use ::alloc::string::String;
use ::alloc::vec;
use ::alloc::vec::Vec;
use bitmap::Bitmap;
use dir::{init_empty_dir_block, insert_dir_entry};
use inode::{read_pointer, write_pointer};
use structs::{
    parse_inode, Inode, EXT2_INDEX_FL, GROUP_DESCRIPTOR_SIZE, SUPERBLOCK_OFFSET,
    SUPERBLOCK_SIZE, VALID_FS,
};

pub const MAX_PATH: usize = 256;
const CACHE_ENTRIES: usize = 16;
const S_IFREG: u16 = 0x8000;
const EXT2_FT_REG_FILE: u8 = 1;

#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ext2Error {
    Device,
    ReadOnly,
    NoSpace,
    FileTooLarge,
    Unsupported,
    AlreadyExists,
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
            Ext2Error::FileTooLarge => write!(formatter, "file too large"),
            Ext2Error::Unsupported => write!(formatter, "unsupported operation"),
            Ext2Error::AlreadyExists => write!(formatter, "file exists"),
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
            Ext2Error::ReadOnly => VfsError::ReadOnly,
            Ext2Error::NoSpace => VfsError::NoSpace,
            Ext2Error::FileTooLarge => VfsError::FileTooLarge,
            Ext2Error::Unsupported => VfsError::Unsupported,
            Ext2Error::AlreadyExists => VfsError::AlreadyExists,
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

/// A block allocated while resolving a logical block, recorded so a failed
/// allocation can undo every pointer and free the block again.
enum Allocation {
    /// Pointer lives in the inode's `i_block` array at the given index.
    Inode(usize, u32),
    /// Pointer lives in an indirect block at `(block, index)`.
    Parent(u32, usize, u32),
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

    /// Resolve `path`, creating the final component when `O_CREAT` is set.
    pub fn open_flags(&mut self, path: &str, flags: u32, mode: u16) -> Result<NodeId, Ext2Error> {
        if path.len() > MAX_PATH {
            return Err(Ext2Error::InvalidPath);
        }
        match self.lookup(path) {
            Ok(node) => {
                let inode = self.read_inode(node)?;
                if flags & O_CREAT != 0 && flags & O_EXCL != 0 {
                    return Err(Ext2Error::AlreadyExists);
                }
                if inode.is_dir()
                    && (flags & (O_CREAT | O_TRUNC) != 0 || flags & O_ACCMODE != O_RDONLY)
                {
                    return Err(Ext2Error::NotFile);
                }
                if flags & O_TRUNC != 0 {
                    if inode.kind() != FileType::Regular {
                        return Err(Ext2Error::NotFile);
                    }
                    self.truncate_inode(node, 0)?;
                }
                Ok(node)
            }
            Err(Ext2Error::NotFound) => {
                if flags & O_CREAT == 0 {
                    return Err(Ext2Error::NotFound);
                }
                let (parent_path, name) = split_parent(path)?;
                let parent = self.lookup(parent_path)?;
                self.create_file(parent, name, mode)
            }
            Err(error) => Err(error),
        }
    }

    /// Create a regular file in `parent`, failing if the name already exists.
    pub fn create_file(
        &mut self,
        parent: NodeId,
        name: &str,
        mode: u16,
    ) -> Result<NodeId, Ext2Error> {
        self.require_writable()?;
        let mut parent_inode = self.read_inode(parent)?;
        self.check_directory_mutable(&parent_inode)?;
        validate_name(name)?;
        match self.find_entry(&parent_inode, name) {
            Ok(_) => return Err(Ext2Error::AlreadyExists),
            Err(Ext2Error::NotFound) => {}
            Err(error) => return Err(error),
        }
        let group = layout::inode_location(&self.superblock, parent)
            .map(|(group, _)| group as usize)
            .unwrap_or(0);
        let child = self.allocate_inode(group, false)?;
        let mut inode = match Inode::empty(child, self.superblock.inode_size() as usize) {
            Ok(inode) => inode,
            Err(error) => {
                let _ = self.free_inode(child, false);
                return Err(error);
            }
        };
        inode.set_mode(S_IFREG | (mode & 0o7777));
        inode.set_links_count(1);
        let now = self.now();
        inode.set_atime(now);
        inode.set_mtime(now);
        inode.set_ctime(now);
        if let Err(error) = self.write_inode(&mut inode) {
            let _ = self.free_inode(child, false);
            return Err(error);
        }
        if let Err(error) = self.add_dir_entry(&mut parent_inode, name, child, EXT2_FT_REG_FILE) {
            let _ = self.free_inode(child, false);
            return Err(error);
        }
        Ok(child)
    }

    /// Insert a directory entry, allocating a new directory block if needed.
    fn add_dir_entry(
        &mut self,
        dir: &mut Inode,
        name: &str,
        child: u32,
        file_type: u8,
    ) -> Result<(), Ext2Error> {
        self.require_writable()?;
        self.check_directory_mutable(dir)?;
        validate_name(name)?;
        if child == 0 || child > self.superblock.inodes_count() {
            return Err(Ext2Error::CorruptInode);
        }
        let block_size = self.superblock.block_size();
        let block_count = (dir.size + block_size as u64 - 1) / block_size as u64;
        for logical in 0..block_count {
            let disk = self.map_block(dir, logical)?;
            if disk == 0 {
                return Err(Ext2Error::CorruptDirectory);
            }
            let mut data = self.read_block_vec(disk as u64)?;
            if insert_dir_entry(&mut data, name.as_bytes(), child, file_type, self.has_filetype) {
                self.write_block(disk as u64, &data)?;
                return self.touch_inode(dir);
            }
        }
        let logical = block_count;
        let block = self.ensure_block(dir, logical)?;
        let mut data = self.read_block_vec(block as u64)?;
        init_empty_dir_block(&mut data);
        if !insert_dir_entry(&mut data, name.as_bytes(), child, file_type, self.has_filetype) {
            return Err(Ext2Error::CorruptDirectory);
        }
        self.write_block(block as u64, &data)?;
        let new_size = (logical + 1) * block_size as u64;
        if new_size > dir.size {
            dir.set_size(new_size, self.superblock.has_large_file())?;
        }
        self.touch_inode(dir)
    }

    fn touch_inode(&mut self, inode: &mut Inode) -> Result<(), Ext2Error> {
        let now = self.now();
        inode.set_mtime(now);
        inode.set_ctime(now);
        self.write_inode(inode)
    }

    fn check_directory_mutable(&self, inode: &Inode) -> Result<(), Ext2Error> {
        if !inode.is_dir() {
            return Err(Ext2Error::NotDirectory);
        }
        if inode.flags() & EXT2_INDEX_FL != 0 {
            return Err(Ext2Error::Unsupported);
        }
        Ok(())
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

    /// Write `buffer` at `offset`, allocating blocks as needed.
    ///
    /// A partial write commits the bytes already written when the allocator
    /// runs out of space, which matches the usual short-write semantics.
    pub fn write_file(
        &mut self,
        node: NodeId,
        offset: u64,
        buffer: &[u8],
    ) -> Result<usize, Ext2Error> {
        self.require_writable()?;
        let mut inode = self.read_inode(node)?;
        if inode.kind() != FileType::Regular {
            return Err(Ext2Error::NotFile);
        }
        if buffer.is_empty() {
            return Ok(0);
        }
        let block_size = self.superblock.block_size();
        let old_size = inode.size;
        let large_file = self.superblock.has_large_file();
        let end = offset
            .checked_add(buffer.len() as u64)
            .ok_or(Ext2Error::FileTooLarge)?;
        if end > self.max_file_size(&inode) {
            return Err(Ext2Error::FileTooLarge);
        }
        // A write past the old end leaves a hole. Zero the stale tail of the
        // old last block so the gap reads back as zeros.
        if offset > old_size {
            let old_last = old_size / block_size as u64;
            let old_within = (old_size % block_size as u64) as usize;
            if old_within != 0 {
                let disk = self.map_block(&inode, old_last)?;
                if disk != 0 {
                    let mut data = self.read_block_vec(disk as u64)?;
                    data[old_within..].fill(0);
                    self.write_block(disk as u64, &data)?;
                }
            }
        }
        let mut done = 0usize;
        let mut failure: Option<Ext2Error> = None;
        while done < buffer.len() {
            let current = offset + done as u64;
            let logical = current / block_size as u64;
            let within = (current % block_size as u64) as usize;
            let chunk = (block_size - within).min(buffer.len() - done);
            let disk = match self.ensure_block(&mut inode, logical) {
                Ok(block) => block,
                Err(error) => {
                    failure = Some(error);
                    break;
                }
            };
            let extending = current + chunk as u64 > old_size;
            if within == 0 && chunk == block_size {
                self.write_block(disk as u64, &buffer[done..done + chunk])?;
            } else {
                let mut data = self.read_block_vec(disk as u64)?;
                if current > old_size {
                    let hole_start = if old_size / block_size as u64 == logical {
                        (old_size % block_size as u64) as usize
                    } else {
                        0
                    };
                    if hole_start < within {
                        data[hole_start..within].fill(0);
                    }
                }
                data[within..within + chunk].copy_from_slice(&buffer[done..done + chunk]);
                // Extending the file leaves a zero-filled tail in its last
                // partial block, so later reads never expose stale bytes.
                if done + chunk == buffer.len() && extending && within + chunk < block_size {
                    data[within + chunk..].fill(0);
                }
                self.write_block(disk as u64, &data)?;
            }
            done += chunk;
        }
        let new_size = (offset + done as u64).max(old_size);
        if done > 0 || new_size != old_size {
            inode.set_size(new_size, large_file)?;
            let now = self.now();
            inode.set_mtime(now);
            inode.set_ctime(now);
            self.write_inode(&mut inode)?;
        }
        match failure {
            Some(error) if done == 0 => Err(error),
            Some(_) => Ok(done),
            None => Ok(done),
        }
    }

    /// Change a regular file's size, zeroing the new tail when shrinking and
    /// leaving sparse holes when growing.
    pub fn truncate_inode(&mut self, node: NodeId, new_size: u64) -> Result<(), Ext2Error> {
        self.require_writable()?;
        let mut inode = self.read_inode(node)?;
        if inode.kind() != FileType::Regular {
            return Err(Ext2Error::NotFile);
        }
        let block_size = self.superblock.block_size();
        let large_file = self.superblock.has_large_file();
        if new_size > self.max_file_size(&inode) {
            return Err(Ext2Error::FileTooLarge);
        }
        let old_size = inode.size;
        if new_size > old_size {
            let old_last = old_size / block_size as u64;
            let old_within = (old_size % block_size as u64) as usize;
            if old_within != 0 {
                let disk = self.map_block(&inode, old_last)?;
                if disk != 0 {
                    let mut data = self.read_block_vec(disk as u64)?;
                    let end = if new_size / block_size as u64 == old_last {
                        (new_size % block_size as u64) as usize
                    } else {
                        block_size
                    };
                    data[old_within..end].fill(0);
                    self.write_block(disk as u64, &data)?;
                }
            }
        } else if new_size < old_size {
            let new_last = new_size / block_size as u64;
            let within = (new_size % block_size as u64) as usize;
            if within != 0 {
                let disk = self.map_block(&inode, new_last)?;
                if disk != 0 {
                    let mut data = self.read_block_vec(disk as u64)?;
                    data[within..].fill(0);
                    self.write_block(disk as u64, &data)?;
                }
            }
            let start_logical = (new_size + block_size as u64 - 1) / block_size as u64;
            self.free_blocks_from(&mut inode, start_logical)?;
        }
        inode.set_size(new_size, large_file)?;
        let now = self.now();
        inode.set_mtime(now);
        inode.set_ctime(now);
        self.write_inode(&mut inode)
    }

    /// Free every data and indirect block at or after `start_logical`.
    fn free_blocks_from(
        &mut self,
        inode: &mut Inode,
        start_logical: u64,
    ) -> Result<(), Ext2Error> {
        self.require_writable()?;
        let block_size = self.superblock.block_size();
        let pointers = (block_size / 4) as u64;
        let sectors = self.sectors_per_block();
        let mut freed = 0u64;
        if start_logical < 12 {
            for index in start_logical as usize..12 {
                let block = inode.block(index);
                if block != 0 {
                    self.free_block(block)?;
                    inode.set_block(index, 0);
                    freed += 1;
                }
            }
        }
        let singly_start = 12u64;
        let singly_end = singly_start + pointers;
        let doubly_end = singly_end + pointers * pointers;
        let triply_end = doubly_end + pointers * pointers * pointers;
        if start_logical < singly_end {
            let block = inode.block(12);
            if block != 0 {
                let count = if start_logical <= singly_start {
                    self.free_indirect_tree(block, 1)?
                } else {
                    self.free_indirect_range(block, 1, start_logical - singly_start)?
                };
                if start_logical <= singly_start {
                    inode.set_block(12, 0);
                }
                freed += count;
            }
        }
        if start_logical < doubly_end {
            let block = inode.block(13);
            if block != 0 {
                let count = if start_logical <= singly_end {
                    self.free_indirect_tree(block, 2)?
                } else {
                    self.free_indirect_range(block, 2, start_logical - singly_end)?
                };
                if start_logical <= singly_end {
                    inode.set_block(13, 0);
                }
                freed += count;
            }
        }
        if start_logical < triply_end {
            let block = inode.block(14);
            if block != 0 {
                let count = if start_logical <= doubly_end {
                    self.free_indirect_tree(block, 3)?
                } else {
                    self.free_indirect_range(block, 3, start_logical - doubly_end)?
                };
                if start_logical <= doubly_end {
                    inode.set_block(14, 0);
                }
                freed += count;
            }
        }
        if freed != 0 {
            let sectors_freed = freed
                .saturating_mul(sectors as u64)
                .min(u32::MAX as u64) as u32;
            inode.sub_sectors(sectors_freed);
        }
        Ok(())
    }

    fn max_file_size(&self, inode: &Inode) -> u64 {
        let block_size = self.superblock.block_size() as u64;
        let pointers = (self.superblock.block_size() / 4) as u64;
        let total = 12u64
            .saturating_add(pointers)
            .saturating_add(pointers.saturating_mul(pointers))
            .saturating_add(
                pointers
                    .saturating_mul(pointers)
                    .saturating_mul(pointers),
            );
        let max = total.saturating_mul(block_size);
        if inode.kind() == FileType::Regular && self.superblock.has_large_file() {
            max
        } else {
            max.min(u32::MAX as u64)
        }
    }

    fn sectors_per_block(&self) -> u32 {
        (self.superblock.block_size() / 512) as u32
    }

    /// Return the disk block for `logical`, allocating it (and any missing
    /// indirect blocks) when necessary.
    fn ensure_block(&mut self, inode: &mut Inode, logical: u64) -> Result<u32, Ext2Error> {
        self.require_writable()?;
        let block_size = self.superblock.block_size();
        let pointers = (block_size / 4) as u64;
        let path = block_path(logical, pointers);
        if path == BlockPath::OutOfRange {
            return Err(Ext2Error::FileTooLarge);
        }
        if !self.superblock.has_large_file()
            && logical
                .saturating_add(1)
                .saturating_mul(block_size as u64)
                > u32::MAX as u64
        {
            return Err(Ext2Error::FileTooLarge);
        }
        let group = layout::inode_location(&self.superblock, inode.number)
            .map(|(group, _)| group as usize)
            .unwrap_or(0);
        let sectors = self.sectors_per_block();
        let mut allocations: Vec<Allocation> = Vec::new();
        let result = match path {
            BlockPath::Direct(index) => {
                let existing = inode.block(index);
                if existing != 0 {
                    Ok(existing)
                } else {
                    let block = self.allocate_block(group)?;
                    allocations.push(Allocation::Inode(index, block));
                    self.zero_block(block)?;
                    inode.set_block(index, block);
                    Ok(block)
                }
            }
            BlockPath::Singly(index) => {
                self.ensure_indirect(inode, 12, &[index], group, &mut allocations)
            }
            BlockPath::Doubly(first, second) => {
                self.ensure_indirect(inode, 13, &[first, second], group, &mut allocations)
            }
            BlockPath::Triply(first, second, third) => {
                self.ensure_indirect(inode, 14, &[first, second, third], group, &mut allocations)
            }
            BlockPath::OutOfRange => Err(Ext2Error::FileTooLarge),
        };
        match result {
            Ok(block) => {
                if !allocations.is_empty() {
                    inode.add_sectors(sectors.saturating_mul(allocations.len() as u32));
                }
                Ok(block)
            }
            Err(error) => {
                self.rollback_allocations(inode, &allocations);
                Err(error)
            }
        }
    }

    fn ensure_indirect(
        &mut self,
        inode: &mut Inode,
        root_index: usize,
        indices: &[u64],
        group: usize,
        allocations: &mut Vec<Allocation>,
    ) -> Result<u32, Ext2Error> {
        if inode.block(root_index) == 0 {
            let block = self.allocate_block(group)?;
            allocations.push(Allocation::Inode(root_index, block));
            self.zero_block(block)?;
            inode.set_block(root_index, block);
        }
        let mut current = inode.block(root_index);
        for (depth, &index) in indices.iter().enumerate() {
            let last = depth + 1 == indices.len();
            let mut data = self.read_block_vec(current as u64)?;
            let child = read_pointer(&data, index as usize);
            if last {
                if child != 0 {
                    return Ok(child);
                }
                let block = self.allocate_block(group)?;
                allocations.push(Allocation::Parent(current, index as usize, block));
                self.zero_block(block)?;
                write_pointer(&mut data, index as usize, block);
                self.write_block(current as u64, &data)?;
                return Ok(block);
            }
            if child == 0 {
                let block = self.allocate_block(group)?;
                allocations.push(Allocation::Parent(current, index as usize, block));
                self.zero_block(block)?;
                write_pointer(&mut data, index as usize, block);
                self.write_block(current as u64, &data)?;
                current = block;
            } else {
                current = child;
            }
        }
        Err(Ext2Error::InvalidBlock)
    }

    fn rollback_allocations(&mut self, inode: &mut Inode, allocations: &[Allocation]) {
        for allocation in allocations.iter().rev() {
            match *allocation {
                Allocation::Inode(index, block) => {
                    inode.set_block(index, 0);
                    let _ = self.free_block(block);
                }
                Allocation::Parent(parent, index, block) => {
                    if let Ok(mut data) = self.read_block_vec(parent as u64) {
                        write_pointer(&mut data, index, 0);
                        let _ = self.write_block(parent as u64, &data);
                    }
                    let _ = self.free_block(block);
                }
            }
        }
    }

    fn zero_block(&mut self, block: u32) -> Result<(), Ext2Error> {
        let zeros = vec![0u8; self.superblock.block_size()];
        self.write_block(block as u64, &zeros)
    }

    fn free_indirect_tree(&mut self, block: u32, level: u8) -> Result<u64, Ext2Error> {
        if block == 0 {
            return Ok(0);
        }
        let pointers = (self.superblock.block_size() / 4) as usize;
        let data = self.read_block_vec(block as u64)?;
        let mut freed = 0u64;
        for index in 0..pointers {
            let child = read_pointer(&data, index);
            if child == 0 {
                continue;
            }
            if level == 1 {
                self.free_block(child)?;
                freed += 1;
            } else {
                freed += self.free_indirect_tree(child, level - 1)?;
            }
        }
        self.free_block(block)?;
        Ok(freed + 1)
    }

    /// Free entries at or after `start` inside an indirect subtree. The
    /// subtree root is kept unless `start` is zero.
    fn free_indirect_range(
        &mut self,
        block: u32,
        level: u8,
        start: u64,
    ) -> Result<u64, Ext2Error> {
        if block == 0 {
            return Ok(0);
        }
        if start == 0 {
            return self.free_indirect_tree(block, level);
        }
        let pointers = (self.superblock.block_size() / 4) as u64;
        let mut data = self.read_block_vec(block as u64)?;
        let mut freed = 0u64;
        if level == 1 {
            for index in start..pointers {
                let child = read_pointer(&data, index as usize);
                if child != 0 {
                    self.free_block(child)?;
                    freed += 1;
                    write_pointer(&mut data, index as usize, 0);
                }
            }
        } else {
            let span = pointers.pow((level - 1) as u32);
            let first = start / span;
            let offset = start % span;
            for index in first..pointers {
                let child = read_pointer(&data, index as usize);
                if child == 0 {
                    continue;
                }
                if index == first && offset != 0 {
                    freed += self.free_indirect_range(child, level - 1, offset)?;
                } else {
                    freed += self.free_indirect_tree(child, level - 1)?;
                }
                write_pointer(&mut data, index as usize, 0);
            }
        }
        self.write_block(block as u64, &data)?;
        Ok(freed)
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

    /// Read-modify-write the inode-table block, preserving fields that this
    /// kernel never interpreted.
    fn write_inode(&mut self, inode: &mut Inode) -> Result<(), Ext2Error> {
        self.require_writable()?;
        if inode.number == 0 || inode.number > self.superblock.inodes_count() {
            return Err(Ext2Error::InodeOutOfRange(inode.number));
        }
        inode.sync_raw(self.superblock.has_large_file())?;
        let inode_size = self.superblock.inode_size() as usize;
        if inode.raw().len() != inode_size {
            return Err(Ext2Error::CorruptInode);
        }
        let index = (inode.number - 1) as u64;
        let group = (index / self.superblock.inodes_per_group() as u64) as usize;
        let within_group = index % self.superblock.inodes_per_group() as u64;
        let descriptor = *self
            .groups
            .get(group)
            .ok_or(Ext2Error::InodeOutOfRange(inode.number))?;
        let block_size = self.superblock.block_size();
        let table_offset = descriptor.inode_table as u64 * block_size as u64;
        let offset = table_offset + within_group * inode_size as u64;
        let block_index = offset / block_size as u64;
        let within_block = (offset % block_size as u64) as usize;
        if within_block + inode_size > block_size {
            return Err(Ext2Error::CorruptInode);
        }
        let mut data = self.read_block_vec(block_index)?;
        data[within_block..within_block + inode_size].copy_from_slice(inode.raw());
        self.write_block(block_index, &data)
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

fn split_parent(path: &str) -> Result<(&str, &str), Ext2Error> {
    if path.is_empty() || path.ends_with('/') {
        return Err(Ext2Error::InvalidPath);
    }
    let (parent, name) = match path.rfind('/') {
        Some(index) => {
            let parent = if index == 0 { "/" } else { &path[..index] };
            (parent, &path[index + 1..])
        }
        None => ("/", path),
    };
    if name.is_empty() || name == "." || name == ".." {
        return Err(Ext2Error::InvalidPath);
    }
    Ok((parent, name))
}

fn validate_name(name: &str) -> Result<(), Ext2Error> {
    let bytes = name.as_bytes();
    if bytes.is_empty() || bytes.len() > 255 {
        return Err(Ext2Error::InvalidPath);
    }
    if bytes.iter().any(|byte| *byte == b'/' || *byte == 0) {
        return Err(Ext2Error::InvalidPath);
    }
    Ok(())
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

    fn open_with_flags(
        &mut self,
        path: &str,
        flags: u32,
        mode: u16,
    ) -> Result<NodeId, VfsError> {
        self.open_flags(path, flags, mode).map_err(VfsError::from)
    }

    fn stat(&mut self, node: NodeId) -> Result<Stat, VfsError> {
        self.stat_inode(node).map_err(VfsError::from)
    }

    fn read(&mut self, node: NodeId, offset: u64, buffer: &mut [u8]) -> Result<usize, VfsError> {
        self.read_file(node, offset, buffer).map_err(VfsError::from)
    }

    fn write(&mut self, node: NodeId, offset: u64, buffer: &[u8]) -> Result<usize, VfsError> {
        self.write_file(node, offset, buffer).map_err(VfsError::from)
    }

    fn truncate(&mut self, node: NodeId, size: u64) -> Result<(), VfsError> {
        self.truncate_inode(node, size).map_err(VfsError::from)
    }

    fn sync(&mut self) -> Result<(), VfsError> {
        Ext2::sync(self).map_err(VfsError::from)
    }

    fn readdir(
        &mut self,
        node: NodeId,
        cursor: &mut DirCursor,
    ) -> Result<Option<DirEntry>, VfsError> {
        self.read_dir(node, cursor).map_err(VfsError::from)
    }
}
