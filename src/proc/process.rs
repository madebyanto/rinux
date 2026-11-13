use super::context::{self, Context, KERNEL_STACK_SIZE};
use super::pid::Pid;
use crate::mm::{deallocate_frame, paging};
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

#[derive(Debug, Clone, Copy)]
pub struct UserPage {
    pub virt: usize,
    pub phys: usize,
}

pub struct Process {
    pub pid: Pid,
    pub state: ProcessState,
    #[allow(dead_code)]
    pub kernel_stack: Vec<u8>,
    pub kernel_stack_top: u64,
    pub context: Context,
    pub user_pages: Vec<UserPage>,
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
        let (mut kernel_stack, kernel_stack_top) = Self::allocate_stack()?;
        let context = context::prepare_stack(&mut kernel_stack, entry);
        Ok(Self {
            pid,
            state: ProcessState::Ready,
            kernel_stack,
            kernel_stack_top,
            context,
            user_pages: Vec::new(),
            exit_code: None,
        })
    }

    pub fn new_user(
        pid: Pid,
        entry: u64,
        stack_top: u64,
        mut user_pages: Vec<UserPage>,
    ) -> Result<Self, ProcessError> {
        let (mut kernel_stack, kernel_stack_top) = match Self::allocate_stack() {
            Ok(stack) => stack,
            Err(error) => {
                free_user_pages(&mut user_pages);
                return Err(error);
            }
        };
        let context = context::prepare_user_stack(&mut kernel_stack, entry, stack_top);
        Ok(Self {
            pid,
            state: ProcessState::Ready,
            kernel_stack,
            kernel_stack_top,
            context,
            user_pages,
            exit_code: None,
        })
    }
}

pub fn free_user_pages(pages: &mut Vec<UserPage>) {
    for page in pages.drain(..) {
        match paging::unmap(page.virt) {
            Ok(phys) => {
                let _ = deallocate_frame(phys);
            }
            Err(_) => {
                let _ = deallocate_frame(page.phys);
            }
        }
    }
}
