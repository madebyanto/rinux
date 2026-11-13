use super::context::{self, Context, KERNEL_STACK_SIZE};
use alloc::vec::Vec;

pub use super::pid::Pid;

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
    pub context: Context,
    #[allow(dead_code)]
    pub exit_code: Option<i32>,
}

impl Process {
    pub fn new(pid: Pid, entry: TaskEntry) -> Result<Self, ProcessError> {
        let mut kernel_stack = Vec::new();
        kernel_stack
            .try_reserve_exact(KERNEL_STACK_SIZE)
            .map_err(|_| ProcessError::OutOfMemory)?;
        kernel_stack.resize(KERNEL_STACK_SIZE, 0);
        let context = context::prepare_stack(&mut kernel_stack, entry);
        Ok(Self {
            pid,
            state: ProcessState::Ready,
            kernel_stack,
            context,
            exit_code: None,
        })
    }
}
