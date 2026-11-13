#![allow(dead_code)]
use super::pid::Pid;
use super::queue::MAX_PROCESSES;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QueueError {
    Full,
    AlreadyQueued,
    Empty,
    NotPresent,
}

pub struct PidQueue {
    entries: [Option<Pid>; MAX_PROCESSES],
    head: usize,
    tail: usize,
    len: usize,
}

impl PidQueue {
    pub const fn new() -> Self {
        Self {
            entries: [None; MAX_PROCESSES],
            head: 0,
            tail: 0,
            len: 0,
        }
    }

    pub fn push(&mut self, pid: Pid) -> Result<(), QueueError> {
        if self.contains(pid) {
            return Err(QueueError::AlreadyQueued);
        }
        if self.len == MAX_PROCESSES {
            return Err(QueueError::Full);
        }
        self.entries[self.tail] = Some(pid);
        self.tail = (self.tail + 1) % MAX_PROCESSES;
        self.len += 1;
        Ok(())
    }

    pub fn pop(&mut self) -> Option<Pid> {
        if self.len == 0 {
            return None;
        }
        let pid = self.entries[self.head].take();
        self.head = (self.head + 1) % MAX_PROCESSES;
        self.len -= 1;
        pid
    }

    pub fn remove(&mut self, pid: Pid) -> bool {
        let mut index = self.head;
        for offset in 0..self.len {
            if self.entries[index] == Some(pid) {
                for remaining in offset..self.len - 1 {
                    let from = (self.head + remaining + 1) % MAX_PROCESSES;
                    let to = (self.head + remaining) % MAX_PROCESSES;
                    self.entries[to] = self.entries[from];
                }
                self.tail = (self.tail + MAX_PROCESSES - 1) % MAX_PROCESSES;
                self.entries[self.tail] = None;
                self.len -= 1;
                return true;
            }
            index = (index + 1) % MAX_PROCESSES;
        }
        false
    }

    pub fn contains(&self, pid: Pid) -> bool {
        let mut index = self.head;
        for _ in 0..self.len {
            if self.entries[index] == Some(pid) {
                return true;
            }
            index = (index + 1) % MAX_PROCESSES;
        }
        false
    }

    pub fn len(&self) -> usize {
        self.len
    }
}
