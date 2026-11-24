use alloc::boxed::Box;
use alloc::string::String;

pub type NodeId = u32;

/// `open` access mode and flag bits (Linux x86-64 values).
pub const O_RDONLY: u32 = 0o0;
pub const O_WRONLY: u32 = 0o1;
pub const O_RDWR: u32 = 0o2;
pub const O_ACCMODE: u32 = 0o3;
pub const O_CREAT: u32 = 0o100;
pub const O_EXCL: u32 = 0o200;
pub const O_TRUNC: u32 = 0o1000;
pub const O_APPEND: u32 = 0o2000;
pub const O_NONBLOCK: u32 = 0o4000;
pub const O_DSYNC: u32 = 0o10000;
pub const O_SYNC: u32 = 0o4010000;
pub const O_CLOEXEC: u32 = 0o2000000;

pub const O_SUPPORTED: u32 = O_ACCMODE
    | O_CREAT
    | O_EXCL
    | O_TRUNC
    | O_APPEND
    | O_NONBLOCK
    | O_DSYNC
    | O_SYNC
    | O_CLOEXEC;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileType {
    Regular,
    Directory,
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Stat {
    pub node: NodeId,
    pub size: u64,
    pub mode: u16,
    pub kind: FileType,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirEntry {
    pub node: NodeId,
    pub name: String,
    pub kind: FileType,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DirCursor {
    pub block: u64,
    pub offset: u16,
}

impl DirCursor {
    pub const fn new() -> Self {
        Self {
            block: 0,
            offset: 0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct File {
    pub node: NodeId,
    pub offset: u64,
    pub flags: u32,
}

impl File {
    pub const fn new(node: NodeId) -> Self {
        Self {
            node,
            offset: 0,
            flags: 0,
        }
    }

    pub const fn with_flags(node: NodeId, flags: u32) -> Self {
        Self {
            node,
            offset: 0,
            flags,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VfsError {
    NotMounted,
    NotFound,
    NotDirectory,
    NotFile,
    AlreadyExists,
    InvalidPath,
    InvalidOffset,
    Corrupt,
    Device,
    Unsupported,
    ReadOnly,
    NoSpace,
    FileTooLarge,
}

pub trait FileSystem: Send {
    fn open(&mut self, path: &str) -> Result<NodeId, VfsError>;

    /// Resolve `path`, optionally creating the final component. Filesystems
    /// that cannot create files keep the default `open` behaviour.
    fn open_with_flags(
        &mut self,
        path: &str,
        flags: u32,
        mode: u16,
    ) -> Result<NodeId, VfsError> {
        let _ = (flags, mode);
        self.open(path)
    }

    fn stat(&mut self, node: NodeId) -> Result<Stat, VfsError>;
    fn read(&mut self, node: NodeId, offset: u64, buffer: &mut [u8]) -> Result<usize, VfsError>;
    fn write(&mut self, node: NodeId, offset: u64, buffer: &[u8]) -> Result<usize, VfsError>;
    fn truncate(&mut self, node: NodeId, size: u64) -> Result<(), VfsError>;
    fn readdir(
        &mut self,
        node: NodeId,
        cursor: &mut DirCursor,
    ) -> Result<Option<DirEntry>, VfsError>;

    fn sync(&mut self) -> Result<(), VfsError> {
        Ok(())
    }
}

pub struct Vfs {
    root: Box<dyn FileSystem>,
}

impl Vfs {
    pub fn new(root: Box<dyn FileSystem>) -> Self {
        Self { root }
    }

    pub fn root_node(&mut self) -> Result<NodeId, VfsError> {
        self.root.open("/")
    }

    pub fn open(&mut self, path: &str) -> Result<File, VfsError> {
        let node = self.root.open(path)?;
        Ok(File::new(node))
    }

    pub fn open_with_flags(
        &mut self,
        path: &str,
        flags: u32,
        mode: u16,
    ) -> Result<File, VfsError> {
        let node = self.root.open_with_flags(path, flags, mode)?;
        Ok(File::with_flags(node, flags))
    }

    pub fn stat(&mut self, path: &str) -> Result<Stat, VfsError> {
        let node = self.root.open(path)?;
        self.root.stat(node)
    }

    #[allow(dead_code)]
    pub fn stat_node(&mut self, node: NodeId) -> Result<Stat, VfsError> {
        self.root.stat(node)
    }

    pub fn read_at(
        &mut self,
        node: NodeId,
        offset: u64,
        buffer: &mut [u8],
    ) -> Result<usize, VfsError> {
        let stat = self.root.stat(node)?;
        if stat.kind != FileType::Regular {
            return Err(VfsError::NotFile);
        }
        if offset >= stat.size {
            return Ok(0);
        }
        let remaining = (stat.size - offset) as usize;
        let limit = buffer.len().min(remaining);
        self.root.read(node, offset, &mut buffer[..limit])
    }

    #[allow(dead_code)]
    pub fn read(&mut self, file: &mut File, buffer: &mut [u8]) -> Result<usize, VfsError> {
        let count = self.read_at(file.node, file.offset, buffer)?;
        file.offset += count as u64;
        Ok(count)
    }

    pub fn write_at(
        &mut self,
        node: NodeId,
        offset: u64,
        buffer: &[u8],
    ) -> Result<usize, VfsError> {
        let stat = self.root.stat(node)?;
        if stat.kind != FileType::Regular {
            return Err(VfsError::NotFile);
        }
        self.root.write(node, offset, buffer)
    }

    #[allow(dead_code)]
    pub fn write(&mut self, file: &mut File, buffer: &[u8]) -> Result<usize, VfsError> {
        let count = self.write_at(file.node, file.offset, buffer)?;
        file.offset += count as u64;
        Ok(count)
    }

    /// Append atomically with respect to other VFS writers and return the new
    /// end offset.
    pub fn append(&mut self, file: &mut File, buffer: &[u8]) -> Result<(usize, u64), VfsError> {
        let stat = self.root.stat(file.node)?;
        if stat.kind != FileType::Regular {
            return Err(VfsError::NotFile);
        }
        let count = self.root.write(file.node, stat.size, buffer)?;
        let end = stat.size + count as u64;
        file.offset = end;
        Ok((count, end))
    }

    pub fn truncate(&mut self, node: NodeId, size: u64) -> Result<(), VfsError> {
        let stat = self.root.stat(node)?;
        if stat.kind != FileType::Regular {
            return Err(VfsError::NotFile);
        }
        self.root.truncate(node, size)
    }

    pub fn sync(&mut self) -> Result<(), VfsError> {
        self.root.sync()
    }

    pub fn readdir(
        &mut self,
        node: NodeId,
        cursor: &mut DirCursor,
    ) -> Result<Option<DirEntry>, VfsError> {
        self.root.readdir(node, cursor)
    }

    pub fn seek(&mut self, file: &mut File, offset: i64, whence: u8) -> Result<u64, VfsError> {
        let base = match whence {
            0 => 0i64,
            1 => file.offset as i64,
            2 => self.root.stat(file.node)?.size as i64,
            _ => return Err(VfsError::InvalidOffset),
        };
        let target = base.checked_add(offset).ok_or(VfsError::InvalidOffset)?;
        if target < 0 {
            return Err(VfsError::InvalidOffset);
        }
        file.offset = target as u64;
        Ok(file.offset)
    }
}

impl core::fmt::Display for VfsError {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            VfsError::NotMounted => write!(formatter, "filesystem not mounted"),
            VfsError::NotFound => write!(formatter, "not found"),
            VfsError::NotDirectory => write!(formatter, "not a directory"),
            VfsError::NotFile => write!(formatter, "not a regular file"),
            VfsError::AlreadyExists => write!(formatter, "already exists"),
            VfsError::InvalidPath => write!(formatter, "invalid path"),
            VfsError::InvalidOffset => write!(formatter, "invalid file offset"),
            VfsError::Corrupt => write!(formatter, "corrupt filesystem"),
            VfsError::Device => write!(formatter, "block device error"),
            VfsError::Unsupported => write!(formatter, "unsupported filesystem feature"),
            VfsError::ReadOnly => write!(formatter, "filesystem is read-only"),
            VfsError::NoSpace => write!(formatter, "no space left on device"),
            VfsError::FileTooLarge => write!(formatter, "file too large"),
        }
    }
}
