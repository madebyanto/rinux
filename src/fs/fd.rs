use super::vfs::{File, NodeId};

pub const MAX_FDS: usize = 16;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FdError {
    InvalidFd,
    NoFreeFd,
    NoProcess,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FdEntry {
    Keyboard,
    Console,
    File(File),
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

    pub fn open_file(&mut self, node: NodeId) -> Result<u32, FdError> {
        for fd in 3..MAX_FDS {
            if self.entries[fd].is_none() {
                self.entries[fd] = Some(FdEntry::File(File::new(node)));
                return Ok(fd as u32);
            }
        }
        Err(FdError::NoFreeFd)
    }

    pub fn close(&mut self, fd: u32) -> Result<(), FdError> {
        if fd < 3 || fd as usize >= MAX_FDS {
            return Err(FdError::InvalidFd);
        }
        match self.entries[fd as usize].take() {
            Some(_) => Ok(()),
            None => Err(FdError::InvalidFd),
        }
    }

    pub fn entry(&self, fd: u32) -> Result<FdEntry, FdError> {
        if fd as usize >= MAX_FDS {
            return Err(FdError::InvalidFd);
        }
        self.entries[fd as usize].ok_or(FdError::InvalidFd)
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
