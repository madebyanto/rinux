use alloc::boxed::Box;
use alloc::string::String;

pub type NodeId = u32;

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
}

impl File {
    pub const fn new(node: NodeId) -> Self {
        Self { node, offset: 0 }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VfsError {
    NotMounted,
    NotFound,
    NotDirectory,
    NotFile,
    InvalidPath,
    InvalidOffset,
    Corrupt,
    Device,
    Unsupported,
}

pub trait FileSystem: Send {
    fn open(&mut self, path: &str) -> Result<NodeId, VfsError>;
    fn stat(&mut self, node: NodeId) -> Result<Stat, VfsError>;
    fn read(&mut self, node: NodeId, offset: u64, buffer: &mut [u8]) -> Result<usize, VfsError>;
    fn readdir(
        &mut self,
        node: NodeId,
        cursor: &mut DirCursor,
    ) -> Result<Option<DirEntry>, VfsError>;
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

    pub fn stat(&mut self, path: &str) -> Result<Stat, VfsError> {
        let node = self.root.open(path)?;
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

    pub fn read(&mut self, file: &mut File, buffer: &mut [u8]) -> Result<usize, VfsError> {
        let count = self.read_at(file.node, file.offset, buffer)?;
        file.offset += count as u64;
        Ok(count)
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
            VfsError::InvalidPath => write!(formatter, "invalid path"),
            VfsError::InvalidOffset => write!(formatter, "invalid file offset"),
            VfsError::Corrupt => write!(formatter, "corrupt filesystem"),
            VfsError::Device => write!(formatter, "block device error"),
            VfsError::Unsupported => write!(formatter, "unsupported filesystem feature"),
        }
    }
}
