use super::vfs::{DirCursor, DirEntry, FileSystem, FileType, NodeId, Stat, VfsError};
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

pub const MAX_PATH: usize = 256;
const MAX_GROUPS: u32 = 1024;
const CACHE_ENTRIES: usize = 16;

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

const EXTENTS_FL: u32 = 0x80000;
const MAGIC: u16 = 0xEF53;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ext2Error {
    Device,
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
}

fn u16_at(data: &[u8], offset: usize) -> u16 {
    u16::from_le_bytes([data[offset], data[offset + 1]])
}

fn u32_at(data: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes([
        data[offset],
        data[offset + 1],
        data[offset + 2],
        data[offset + 3],
    ])
}

pub struct Superblock {
    inodes_count: u32,
    blocks_count: u32,
    first_data_block: u32,
    block_size: usize,
    blocks_per_group: u32,
    inodes_per_group: u32,
    inode_size: u16,
    feature_incompat: u32,
    feature_ro_compat: u32,
    volume_name: [u8; 16],
}

impl Superblock {
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

    pub fn volume_name(&self) -> &str {
        let mut end = self.volume_name.len();
        while end > 0 && (self.volume_name[end - 1] == 0 || self.volume_name[end - 1] == b' ') {
            end -= 1;
        }
        core::str::from_utf8(&self.volume_name[..end]).unwrap_or("unknown")
    }

    fn groups(&self) -> u32 {
        (self.blocks_count + self.blocks_per_group - 1) / self.blocks_per_group
    }

    fn has_filetype(&self) -> bool {
        self.feature_incompat & INCOMPAT_FILETYPE != 0
    }

    fn has_large_file(&self) -> bool {
        self.feature_ro_compat & RO_LARGE_FILE != 0
    }
}

fn parse_superblock(raw: &[u8]) -> Result<Superblock, Ext2Error> {
    if raw.len() < 1024 {
        return Err(Ext2Error::BadSuperblock);
    }
    let magic = u16_at(raw, 56);
    if magic != MAGIC {
        return Err(Ext2Error::BadMagic(magic));
    }
    let log_block_size = u32_at(raw, 24);
    if log_block_size > 2 {
        return Err(Ext2Error::UnsupportedBlockSize(log_block_size));
    }
    let block_size = 1024usize << log_block_size;
    let first_data_block = u32_at(raw, 20);
    let expected_first = if block_size == 1024 { 1 } else { 0 };
    if first_data_block != expected_first {
        return Err(Ext2Error::BadSuperblock);
    }
    let inodes_count = u32_at(raw, 0);
    let blocks_count = u32_at(raw, 4);
    let blocks_per_group = u32_at(raw, 32);
    let inodes_per_group = u32_at(raw, 40);
    if inodes_count == 0 || blocks_count == 0 || blocks_per_group == 0 || inodes_per_group == 0 {
        return Err(Ext2Error::BadSuperblock);
    }
    let revision = u32_at(raw, 76);
    let inode_size = if revision == 0 { 128 } else { u16_at(raw, 88) };
    if inode_size < 128 || inode_size as usize > block_size || block_size % inode_size as usize != 0
    {
        return Err(Ext2Error::UnsupportedInodeSize(inode_size));
    }
    let feature_compat = u32_at(raw, 92);
    let feature_incompat = u32_at(raw, 96);
    let feature_ro_compat = u32_at(raw, 100);
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
    let groups = (blocks_count + blocks_per_group - 1) / blocks_per_group;
    if groups == 0 || groups > MAX_GROUPS {
        return Err(Ext2Error::BadSuperblock);
    }
    let mut volume_name = [0u8; 16];
    volume_name.copy_from_slice(&raw[120..136]);
    Ok(Superblock {
        inodes_count,
        blocks_count,
        first_data_block,
        block_size,
        blocks_per_group,
        inodes_per_group,
        inode_size,
        feature_incompat,
        feature_ro_compat,
        volume_name,
    })
}

#[derive(Debug, Clone, Copy)]
struct GroupDescriptor {
    inode_table: u32,
}

fn parse_group_descriptor(raw: &[u8]) -> Result<GroupDescriptor, Ext2Error> {
    if raw.len() < 32 {
        return Err(Ext2Error::BadSuperblock);
    }
    Ok(GroupDescriptor {
        inode_table: u32_at(raw, 8),
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlockPath {
    Direct(usize),
    Singly(u64),
    Doubly(u64, u64),
    Triply(u64, u64, u64),
    OutOfRange,
}

pub fn block_path(logical: u64, pointers: u64) -> BlockPath {
    let singly_end = 12 + pointers;
    let doubly_end = singly_end + pointers * pointers;
    let triply_end = doubly_end + pointers * pointers * pointers;
    if logical < 12 {
        BlockPath::Direct(logical as usize)
    } else if logical < singly_end {
        BlockPath::Singly(logical - 12)
    } else if logical < doubly_end {
        let index = logical - singly_end;
        BlockPath::Doubly(index / pointers, index % pointers)
    } else if logical < triply_end {
        let index = logical - doubly_end;
        BlockPath::Triply(
            index / (pointers * pointers),
            (index / pointers) % pointers,
            index % pointers,
        )
    } else {
        BlockPath::OutOfRange
    }
}

#[derive(Debug, Clone, Copy)]
struct Inode {
    number: u32,
    mode: u16,
    size: u64,
    blocks: [u32; 15],
}

impl Inode {
    fn kind(&self) -> FileType {
        match self.mode & 0xF000 {
            0x8000 => FileType::Regular,
            0x4000 => FileType::Directory,
            _ => FileType::Other,
        }
    }

    fn is_dir(&self) -> bool {
        self.kind() == FileType::Directory
    }
}

fn parse_inode(number: u32, raw: &[u8], large_file: bool) -> Result<Inode, Ext2Error> {
    if raw.len() < 128 {
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
    })
}

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
}

impl<D: BlockDevice> Ext2<D> {
    pub fn mount(mut device: D) -> Result<Self, Ext2Error> {
        let mut raw_superblock = [0u8; 1024];
        device.read_at(1024, &mut raw_superblock)?;
        let superblock = parse_superblock(&raw_superblock)?;
        let mut cache = BlockCache::new(superblock.block_size);
        let groups = read_groups(&mut device, &mut cache, &superblock)?;
        let has_filetype = superblock.has_filetype();
        let mut filesystem = Self {
            device,
            superblock,
            groups,
            cache,
            has_filetype,
        };
        let root = filesystem.read_inode(2)?;
        if !root.is_dir() {
            return Err(Ext2Error::CorruptInode);
        }
        Ok(filesystem)
    }

    pub fn superblock(&self) -> &Superblock {
        &self.superblock
    }

    #[allow(dead_code)]
    pub fn cache_stats(&self) -> (u64, u64) {
        self.cache.stats()
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
        let block_size = self.superblock.block_size;
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
        let block_size = self.superblock.block_size as u64;
        let has_filetype = self.has_filetype;
        let inodes_count = self.superblock.inodes_count;
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
        if number == 0 || number > self.superblock.inodes_count {
            return Err(Ext2Error::InodeOutOfRange(number));
        }
        let index = (number - 1) as u64;
        let group = (index / self.superblock.inodes_per_group as u64) as usize;
        let within_group = index % self.superblock.inodes_per_group as u64;
        let descriptor = self
            .groups
            .get(group)
            .ok_or(Ext2Error::InodeOutOfRange(number))?;
        let inode_size = self.superblock.inode_size as u64;
        let table_offset = descriptor.inode_table as u64 * self.superblock.block_size as u64;
        let offset = table_offset + within_group * inode_size;
        let block_index = offset / self.superblock.block_size as u64;
        let within_block = (offset % self.superblock.block_size as u64) as usize;
        let size = self.superblock.inode_size as usize;
        if within_block + size > self.superblock.block_size {
            return Err(Ext2Error::CorruptInode);
        }
        let large_file = self.superblock.has_large_file();
        let data = self.block(block_index)?;
        parse_inode(number, &data[within_block..within_block + size], large_file)
    }

    fn block(&mut self, block: u64) -> Result<&[u8], Ext2Error> {
        if block >= self.superblock.blocks_count as u64 {
            return Err(Ext2Error::OutOfBounds);
        }
        let block_size = self.superblock.block_size;
        self.cache.get(&mut self.device, block, block_size)
    }

    fn map_block(&mut self, inode: &Inode, logical: u64) -> Result<u32, Ext2Error> {
        let pointers = (self.superblock.block_size / 4) as u64;
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

fn read_pointer(data: &[u8], index: usize) -> u32 {
    let offset = index * 4;
    u32::from_le_bytes([
        data[offset],
        data[offset + 1],
        data[offset + 2],
        data[offset + 3],
    ])
}

fn read_groups<D: BlockDevice>(
    device: &mut D,
    cache: &mut BlockCache,
    superblock: &Superblock,
) -> Result<Vec<GroupDescriptor>, Ext2Error> {
    let groups = superblock.groups();
    let total = groups as usize * 32;
    let table_block = (superblock.first_data_block + 1) as u64;
    let table_blocks = (total + superblock.block_size - 1) / superblock.block_size;
    let mut raw = Vec::with_capacity(total);
    for index in 0..table_blocks {
        let block = table_block + index as u64;
        if block >= superblock.blocks_count as u64 {
            return Err(Ext2Error::OutOfBounds);
        }
        let data = cache.get(device, block, superblock.block_size)?;
        let remaining = total - raw.len();
        let take = remaining.min(superblock.block_size);
        raw.extend_from_slice(&data[..take]);
    }
    let mut descriptors = Vec::with_capacity(groups as usize);
    for index in 0..groups as usize {
        let descriptor = parse_group_descriptor(&raw[index * 32..index * 32 + 32])?;
        if descriptor.inode_table == 0 || descriptor.inode_table >= superblock.blocks_count {
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
