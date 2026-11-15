pub type Pid = u32;

/// The kernel init task, which becomes the ancestor of every orphan.
pub const INIT_PID: Pid = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PidError {
    Exhausted,
}

#[derive(Debug, Clone, Copy)]
pub struct PidAllocator {
    next: Option<Pid>,
}

impl PidAllocator {
    pub const fn new() -> Self {
        Self::starting_at(1)
    }

    pub const fn starting_at(next: Pid) -> Self {
        Self { next: Some(next) }
    }

    pub fn allocate(&mut self) -> Result<Pid, PidError> {
        let pid = self.next.ok_or(PidError::Exhausted)?;
        self.next = pid.checked_add(1);
        Ok(pid)
    }
}
