use super::context::{self, Context, KERNEL_STACK_SIZE};
use super::pid::Pid;
use crate::fs::fd::FdTable;
use crate::mm::address_space::AddressSpace;
use alloc::vec::Vec;

pub type TaskEntry = extern "C" fn();

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProcessState {
    Ready,
    Running,
    #[allow(dead_code)]
    Blocked,
    Terminated,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProcessError {
    OutOfMemory,
}

pub struct Process {
    pub pid: Pid,
    pub state: ProcessState,
    #[allow(dead_code)]
    pub kernel_stack: Vec<u8>,
    pub kernel_stack_top: u64,
    pub context: Context,
    pub address_space: AddressSpace,
    pub fds: FdTable,
    #[allow(dead_code)]
    pub exit_code: Option<i32>,
}

impl Process {
    fn allocate_stack() -> Result<(Vec<u8>, u64), ProcessError> {
        let mut kernel_stack = Vec::new();
        kernel_stack
            .try_reserve_exact(KERNEL_STACK_SIZE)
            .map_err(|_| ProcessError::OutOfMemory)?;
        kernel_stack.resize(KERNEL_STACK_SIZE, 0);
        let top = ((kernel_stack.as_ptr() as usize + kernel_stack.len()) & !0xF) as u64;
        Ok((kernel_stack, top))
    }

    pub fn new(pid: Pid, entry: TaskEntry) -> Result<Self, ProcessError> {
        let address_space = AddressSpace::new().map_err(|_| ProcessError::OutOfMemory)?;
        let (mut kernel_stack, kernel_stack_top) = Self::allocate_stack()?;
        let context = context::prepare_stack(&mut kernel_stack, entry);
        Ok(Self {
            pid,
            state: ProcessState::Ready,
            kernel_stack,
            kernel_stack_top,
            context,
            address_space,
            fds: FdTable::new(),
            exit_code: None,
        })
    }

    pub fn new_user(
        pid: Pid,
        entry: u64,
        stack_top: u64,
        address_space: AddressSpace,
    ) -> Result<Self, ProcessError> {
        let (mut kernel_stack, kernel_stack_top) = Self::allocate_stack()?;
        let context = context::prepare_user_stack(&mut kernel_stack, entry, stack_top);
        Ok(Self {
            pid,
            state: ProcessState::Ready,
            kernel_stack,
            kernel_stack_top,
            context,
            address_space,
            fds: FdTable::new(),
            exit_code: None,
        })
    }
}
