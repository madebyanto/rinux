use super::pipe::PipeEnd;
use super::vfs::{File, NodeId};
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::fmt;

pub const MAX_FDS: usize = 16;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FdError {
    InvalidFd,
    NoFreeFd,
    NoProcess,
}

#[derive(Clone)]
pub enum FdEntry {
    Keyboard,
    Console,
    File(File),
    /// A pipe endpoint. Cloning shares the pipe, which is what makes `fork`
    /// inherit both ends and why an end closes only when every descriptor
    /// referring to it is gone.
    Pipe(Arc<PipeEnd>),
}

impl PartialEq for FdEntry {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (FdEntry::Keyboard, FdEntry::Keyboard) | (FdEntry::Console, FdEntry::Console) => true,
            (FdEntry::File(left), FdEntry::File(right)) => left == right,
            (FdEntry::Pipe(left), FdEntry::Pipe(right)) => Arc::ptr_eq(left, right),
            _ => false,
        }
    }
}

impl Eq for FdEntry {}

impl fmt::Debug for FdEntry {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FdEntry::Keyboard => formatter.write_str("Keyboard"),
            FdEntry::Console => formatter.write_str("Console"),
            FdEntry::File(file) => formatter.debug_tuple("File").field(file).finish(),
            FdEntry::Pipe(end) => formatter.debug_tuple("Pipe").field(end).finish(),
        }
    }
}

#[derive(Clone)]
pub struct FdTable {
    entries: [Option<FdEntry>; MAX_FDS],
}

impl FdTable {
    pub const fn new() -> Self {
        Self {
            entries: [
                Some(FdEntry::Keyboard),
                Some(FdEntry::Console),
                Some(FdEntry::Console),
                None,
                None,
                None,
                None,
                None,
                None,
                None,
                None,
                None,
                None,
                None,
                None,
                None,
            ],
        }
    }

    /// Installs the standard console file descriptors.
    ///
    /// fd 0 reads from the keyboard, fd 1 and fd 2 write to the console.
    pub fn init_stdio(&mut self) {
        self.entries[0] = Some(FdEntry::Keyboard);
        self.entries[1] = Some(FdEntry::Console);
        self.entries[2] = Some(FdEntry::Console);
    }

    /// Table with every descriptor closed.
    ///
    /// Used when a process exits: the descriptor table is dropped immediately
    /// while the process stays behind as a zombie.
    pub const fn empty() -> Self {
        Self {
            entries: [
                None, None, None, None, None, None, None, None, None, None, None, None, None, None,
                None, None,
            ],
        }
    }

    /// Installs `entry` at the lowest free descriptor.
    pub fn install(&mut self, entry: FdEntry) -> Result<u32, FdError> {
        for fd in 0..MAX_FDS {
            if self.entries[fd].is_none() {
                self.entries[fd] = Some(entry);
                return Ok(fd as u32);
            }
        }
        Err(FdError::NoFreeFd)
    }

    pub fn open_file(&mut self, node: NodeId, flags: u32) -> Result<u32, FdError> {
        self.install(FdEntry::File(File::with_flags(node, flags)))
    }

    /// Number of descriptors that can still be allocated.
    pub fn free_count(&self) -> usize {
        self.entries.iter().filter(|entry| entry.is_none()).count()
    }

    /// Removes and returns a descriptor without dropping it under the caller's
    /// lock. Close paths use this so pipe teardown can wake peers safely.
    pub fn take(&mut self, fd: u32) -> Result<FdEntry, FdError> {
        if fd as usize >= MAX_FDS {
            return Err(FdError::InvalidFd);
        }
        self.entries[fd as usize].take().ok_or(FdError::InvalidFd)
    }

    /// Removes `fd` and hands the entry back to the caller.
    ///
    /// Descriptors 0-2 are the standard console descriptors and cannot be
    /// closed. The entry is returned rather than dropped here so that a close
    /// path holding the scheduler lock can release a pipe endpoint (whose
    /// destructor wakes blocked peers) after unlocking.
    pub fn close(&mut self, fd: u32) -> Result<FdEntry, FdError> {
        if fd < 3 {
            return Err(FdError::InvalidFd);
        }
        self.take(fd)
    }

    /// One entry per open file descriptor, used by `fork` to duplicate the
    /// VFS open counts and by exit to release them.
    pub fn file_nodes(&self) -> Vec<NodeId> {
        self.entries
            .iter()
            .filter_map(|entry| match entry {
                Some(FdEntry::File(file)) => Some(file.node),
                _ => None,
            })
            .collect()
    }

    pub fn entry(&self, fd: u32) -> Result<FdEntry, FdError> {
        if fd as usize >= MAX_FDS {
            return Err(FdError::InvalidFd);
        }
        self.entries[fd as usize]
            .clone()
            .ok_or(FdError::InvalidFd)
    }

    pub fn set_offset(&mut self, fd: u32, offset: u64) -> Result<(), FdError> {
        match self
            .entries
            .get_mut(fd as usize)
            .and_then(|entry| entry.as_mut())
        {
            Some(FdEntry::File(file)) => {
                file.offset = offset;
                Ok(())
            }
            _ => Err(FdError::InvalidFd),
        }
    }
}
