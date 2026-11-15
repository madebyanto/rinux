use super::context::{self, Context, KERNEL_STACK_SIZE};
use super::pid::Pid;
use super::signal::{default_action, DefaultAction, SigAction, SigSet, Signal, NSIG};
use super::wait::WaitTarget;
use crate::arch::x86_64::syscall::SyscallFrame;
use crate::fs::fd::FdTable;
use crate::mm::address_space::AddressSpace;
use alloc::vec::Vec;

pub type TaskEntry = extern "C" fn() -> !;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProcessState {
    Ready,
    Running,
    #[allow(dead_code)]
    Blocked,
    Zombie,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProcessError {
    OutOfMemory,
}

/// Per-process signal state.
///
/// Rinux supports the standard signals only, so the state is one pending bit
/// per signal and one disposition per signal. `interrupted` records that a
/// blocking syscall was woken by a signal, so the syscall layer can turn the
/// wakeup into `EINTR` once delivery is wired up.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SignalState {
    #[allow(dead_code)] // read by the delivery loop in the next step
    pub pending: SigSet,
    pub blocked: SigSet,
    pub actions: [SigAction; NSIG],
    #[allow(dead_code)] // handler nesting, incremented by delivery
    pub depth: u8,
    #[allow(dead_code)] // consumed by interruptible syscalls once wired up
    pub interrupted: bool,
}

impl SignalState {
    pub const fn new() -> Self {
        Self {
            pending: SigSet::EMPTY,
            blocked: SigSet::EMPTY,
            actions: [SigAction::DEFAULT; NSIG],
            depth: 0,
            interrupted: false,
        }
    }

    /// State inherited at `fork`: dispositions and the blocked mask survive,
    /// while pending signals, handler nesting and interruption do not.
    pub fn forked(parent: &Self) -> Self {
        Self {
            pending: SigSet::EMPTY,
            blocked: parent.blocked,
            actions: parent.actions,
            depth: 0,
            interrupted: false,
        }
    }

    pub fn is_blocked(&self, signal: Signal) -> bool {
        self.blocked.contains(signal)
    }

    /// Whether a pending `signal` has work to do under the current mask and
    /// disposition.
    ///
    /// `SIGKILL`/`SIGSTOP` always do. Used by `send_signal` to decide whether
    /// an incoming signal must wake a task blocked in a syscall.
    pub fn needs_delivery(&self, signal: Signal) -> bool {
        if signal.is_uncatchable() {
            return true;
        }
        if self.is_blocked(signal) {
            return false;
        }
        let action = self.actions[signal.index()];
        !(action.is_ignored()
            || (action.is_default()
                && matches!(
                    default_action(signal),
                    DefaultAction::Ignore | DefaultAction::Stop
                )))
    }
}

impl Default for SignalState {
    fn default() -> Self {
        Self::new()
    }
}

/// How a process left the CPU.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExitStatus {
    Exited(i32),
    /// Produced by the delivery path once fatal signals are wired up.
    #[allow(dead_code)]
    Signaled(Signal),
}

impl ExitStatus {
    /// Encodes the status word `waitpid(2)` writes.
    pub const fn wait_status(self) -> i32 {
        match self {
            Self::Exited(code) => (code & 0xFF) << 8,
            Self::Signaled(signal) => signal.number() as i32,
        }
    }
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
    pub signals: SignalState,
    pub parent: Option<Pid>,
    pub waiting_for: Option<WaitTarget>,
    pub exit_status: Option<ExitStatus>,
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
            signals: SignalState::new(),
            parent: None,
            waiting_for: None,
            exit_status: None,
        })
    }

    pub fn new_fork(
        pid: Pid,
        parent: Pid,
        address_space: AddressSpace,
        fds: FdTable,
        signals: SignalState,
        frame: &SyscallFrame,
    ) -> Result<Self, ProcessError> {
        let (mut kernel_stack, kernel_stack_top) = Self::allocate_stack()?;
        let context = context::prepare_fork_stack(&mut kernel_stack, frame);
        Ok(Self {
            pid,
            state: ProcessState::Ready,
            kernel_stack,
            kernel_stack_top,
            context,
            address_space,
            fds,
            signals,
            parent: Some(parent),
            waiting_for: None,
            exit_status: None,
        })
    }
}
