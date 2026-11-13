pub mod ext2;
pub mod fd;
pub mod mbr;
pub mod vfs;

use crate::drivers::ata;
use crate::sync::SpinLock;
use alloc::boxed::Box;
use alloc::string::String;
use ext2::{BlockDevice, Ext2, Ext2Error};
use fd::{FdEntry, FdError, FdTable};
use vfs::{DirCursor, DirEntry, File, NodeId, Stat, Vfs, VfsError};

static VFS: SpinLock<Option<Vfs>> = SpinLock::new(None);
static FDS: SpinLock<Option<FdTable>> = SpinLock::new(None);

pub struct MountInfo {
    pub partition: mbr::Partition,
    pub volume: String,
    pub block_size: usize,
    pub inode_size: u16,
    pub inodes: u32,
    pub blocks: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MountError {
    Ata,
    Mbr(mbr::MbrError),
    PartitionOutOfRange,
    Ext2(Ext2Error),
}

impl core::fmt::Display for MountError {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            MountError::Ata => write!(formatter, "ATA read failed"),
            MountError::Mbr(mbr::MbrError::Signature) => {
                write!(formatter, "no MBR signature 0x55AA")
            }
            MountError::Mbr(mbr::MbrError::NoPartition) => {
                write!(formatter, "no ext2 (type 0x83) partition")
            }
            MountError::Mbr(mbr::MbrError::OutOfRange) => {
                write!(formatter, "partition LBA out of range")
            }
            MountError::Mbr(mbr::MbrError::Misaligned) => {
                write!(formatter, "partition read misaligned")
            }
            MountError::PartitionOutOfRange => {
                write!(formatter, "partition extends beyond the ATA disk")
            }
            MountError::Ext2(error) => write!(formatter, "{}", error),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FsError {
    Vfs(VfsError),
    Fd(FdError),
}

impl From<VfsError> for FsError {
    fn from(error: VfsError) -> Self {
        FsError::Vfs(error)
    }
}

impl From<FdError> for FsError {
    fn from(error: FdError) -> Self {
        FsError::Fd(error)
    }
}

struct AtaDevice {
    start_lba: u32,
    sectors: u32,
}

impl BlockDevice for AtaDevice {
    fn read_at(&mut self, offset: u64, buffer: &mut [u8]) -> Result<(), Ext2Error> {
        if offset % 512 != 0 || buffer.len() % 512 != 0 || buffer.is_empty() {
            return Err(Ext2Error::Device);
        }
        let count = (buffer.len() / 512) as u32;
        let lba = mbr::absolute_lba(self.start_lba, offset).map_err(|_| Ext2Error::OutOfBounds)?;
        let end = lba as u64 + count as u64;
        let partition_end = self.start_lba as u64 + self.sectors as u64;
        if end > partition_end {
            return Err(Ext2Error::OutOfBounds);
        }
        ata::read_sectors(lba, count as usize, buffer).map_err(|_| Ext2Error::Device)
    }
}

pub fn mount(disk_sectors: u32) -> Result<MountInfo, MountError> {
    let mut mbr_block = [0u8; 512];
    ata::read_sectors(0, 1, &mut mbr_block).map_err(|_| MountError::Ata)?;
    let partition = mbr::partition(&mbr_block).map_err(MountError::Mbr)?;
    let end = partition
        .start_lba
        .checked_add(partition.sectors)
        .ok_or(MountError::PartitionOutOfRange)?;
    if end > disk_sectors {
        return Err(MountError::PartitionOutOfRange);
    }
    let device = AtaDevice {
        start_lba: partition.start_lba,
        sectors: partition.sectors,
    };
    let filesystem = Ext2::mount(device).map_err(MountError::Ext2)?;
    let volume = String::from(filesystem.superblock().volume_name());
    let info = MountInfo {
        partition,
        volume,
        block_size: filesystem.superblock().block_size(),
        inode_size: filesystem.superblock().inode_size(),
        inodes: filesystem.superblock().inodes_count(),
        blocks: filesystem.superblock().blocks_count(),
    };
    {
        let mut vfs = VFS.lock();
        *vfs = Some(Vfs::new(Box::new(filesystem)));
    }
    {
        let mut fds = FDS.lock();
        *fds = Some(FdTable::new());
    }
    Ok(info)
}

pub fn root() -> Result<NodeId, VfsError> {
    let mut guard = VFS.lock();
    let vfs = guard.as_mut().ok_or(VfsError::NotMounted)?;
    vfs.root_node()
}

pub fn readdir(node: NodeId, cursor: &mut DirCursor) -> Result<Option<DirEntry>, VfsError> {
    let mut guard = VFS.lock();
    let vfs = guard.as_mut().ok_or(VfsError::NotMounted)?;
    vfs.readdir(node, cursor)
}

pub fn open(path: &str) -> Result<File, VfsError> {
    let mut guard = VFS.lock();
    let vfs = guard.as_mut().ok_or(VfsError::NotMounted)?;
    vfs.open(path)
}

pub fn read(file: &mut File, buffer: &mut [u8]) -> Result<usize, VfsError> {
    let mut guard = VFS.lock();
    let vfs = guard.as_mut().ok_or(VfsError::NotMounted)?;
    vfs.read(file, buffer)
}

pub fn stat_path(path: &str) -> Result<Stat, VfsError> {
    let mut guard = VFS.lock();
    let vfs = guard.as_mut().ok_or(VfsError::NotMounted)?;
    vfs.stat(path)
}

pub fn read_node(node: NodeId, offset: u64, buffer: &mut [u8]) -> Result<usize, VfsError> {
    let mut guard = VFS.lock();
    let vfs = guard.as_mut().ok_or(VfsError::NotMounted)?;
    vfs.read_at(node, offset, buffer)
}

pub fn open_fd(path: &str) -> Result<u32, FsError> {
    let node = {
        let mut guard = VFS.lock();
        let vfs = guard.as_mut().ok_or(VfsError::NotMounted)?;
        vfs.open(path)?.node
    };
    let mut guard = FDS.lock();
    let table = guard.as_mut().ok_or(VfsError::NotMounted)?;
    Ok(table.open_file(node)?)
}

pub fn close_fd(fd: u32) -> Result<(), FsError> {
    let mut guard = FDS.lock();
    let table = guard.as_mut().ok_or(VfsError::NotMounted)?;
    Ok(table.close(fd)?)
}

pub fn fd_entry(fd: u32) -> Result<FdEntry, FsError> {
    let guard = FDS.lock();
    let table = guard.as_ref().ok_or(VfsError::NotMounted)?;
    Ok(table.entry(fd)?)
}

pub fn set_fd_offset(fd: u32, offset: u64) -> Result<(), FsError> {
    let mut guard = FDS.lock();
    let table = guard.as_mut().ok_or(VfsError::NotMounted)?;
    Ok(table.set_offset(fd, offset)?)
}

pub fn read_file_fd(fd: u32, buffer: &mut [u8]) -> Result<usize, FsError> {
    let file = match fd_entry(fd)? {
        FdEntry::File(file) => file,
        _ => return Err(FsError::Vfs(VfsError::NotFile)),
    };
    let count = read_node(file.node, file.offset, buffer)?;
    if count > 0 {
        set_fd_offset(fd, file.offset + count as u64)?;
    }
    Ok(count)
}

pub fn seek_fd(fd: u32, offset: i64, whence: u8) -> Result<u64, FsError> {
    let mut file = match fd_entry(fd)? {
        FdEntry::File(file) => file,
        _ => return Err(FsError::Vfs(VfsError::NotFile)),
    };
    let new_offset = {
        let mut guard = VFS.lock();
        let vfs = guard.as_mut().ok_or(VfsError::NotMounted)?;
        vfs.seek(&mut file, offset, whence)?
    };
    set_fd_offset(fd, new_offset)?;
    Ok(new_offset)
}
