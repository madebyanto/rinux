use super::pid::Pid;
use super::queue::MAX_PROCESSES;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReapError {
    Full,
    AlreadyPresent,
}

pub struct ReapList {
    entries: [Option<Pid>; MAX_PROCESSES],
    len: usize,
}

impl ReapList {
    pub const fn new() -> Self {
        Self {
            entries: [None; MAX_PROCESSES],
            len: 0,
        }
    }

    pub fn push(&mut self, pid: Pid) -> Result<(), ReapError> {
        if self.contains(pid) {
            return Err(ReapError::AlreadyPresent);
        }
        if self.len == MAX_PROCESSES {
            return Err(ReapError::Full);
        }
        self.entries[self.len] = Some(pid);
        self.len += 1;
        Ok(())
    }

    pub fn take_reapable(&mut self, current: Option<Pid>) -> Option<Pid> {
        let mut index = 0;
        while index < self.len {
            let pid = self.entries[index]?;
            if Some(pid) == current {
                index += 1;
                continue;
            }
            for remaining in index..self.len - 1 {
                self.entries[remaining] = self.entries[remaining + 1];
            }
            self.entries[self.len - 1] = None;
            self.len -= 1;
            return Some(pid);
        }
        None
    }

    pub fn contains(&self, pid: Pid) -> bool {
        self.entries[..self.len]
            .iter()
            .flatten()
            .any(|entry| *entry == pid)
    }

    #[allow(dead_code)]
    pub fn len(&self) -> usize {
        self.len
    }

    #[allow(dead_code)]
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }
}
