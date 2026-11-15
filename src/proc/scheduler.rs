use super::context::{self, Context};
use super::pid::{Pid, PidAllocator, INIT_PID};
use super::process::{ExitStatus, Process, ProcessState, SignalState, TaskEntry};
use super::queue::{ReadyQueue, MAX_PROCESSES};
use super::reap::ReapList;
use super::signal::{SigAction, SigSet, Signal};
use super::wait::WaitTarget;
use super::wait_queue::{PidQueue, QueueError};
use crate::arch::x86_64::cpu;
use crate::arch::x86_64::gdt;
use crate::arch::x86_64::syscall::SyscallFrame;
use crate::fs::fd::{FdEntry, FdError, FdTable};
use crate::fs::pipe::PipeDirection;
use crate::mm::address_space::AddressSpace;
use crate::mm::{paging, uaccess};
use crate::sync::SpinLock;
use alloc::sync::Arc;

pub const TIMESLICE_TICKS: u32 = 10;
pub const TIMESLICE_MS: u64 = 100;

/// `rt_sigprocmask` operations, using the Linux values.
pub const SIG_BLOCK: u64 = 0;
pub const SIG_UNBLOCK: u64 = 1;
pub const SIG_SETMASK: u64 = 2;

/// Fixed number of pipes that can have tasks blocked on them at once.
pub const MAX_PIPES: usize = 32;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SchedulerError {
    AlreadyInitialized,
    NotInitialized,
    NoCurrentProcess,
    KeyboardQueueFull,
    PipeTableFull,
    InvalidPipe,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpawnError {
    NotInitialized,
    NoFreeSlot,
    PidExhausted,
    OutOfMemory,
    QueueFull,
    InitNotFirst,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ForkError {
    NoCurrent,
    NoFreeSlot,
    PidExhausted,
    OutOfMemory,
    QueueFull,
}

/// Failures of the signal entry points.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SignalError {
    NotInitialized,
    NoCurrentProcess,
    NoSuchProcess,
    /// Signals to PID 1 are reserved to PID 1 itself.
    PermissionDenied,
    /// `SIGKILL`/`SIGSTOP` cannot be caught or queried.
    Uncatchable,
    /// Unknown `sa_flags` bits.
    UnsupportedFlags,
    /// A handler or restorer outside the user program window.
    InvalidHandler,
    /// Unknown `rt_sigprocmask` operation.
    InvalidHow,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WaitResult {
    Reaped(Pid, ExitStatus),
    Block,
    NoChild,
}

/// Wait queues for one pipe id.
///
/// Queues live in the scheduler so that registering a waiter and marking the
/// task blocked happen under a single lock. A pipe reserves an id for its whole
/// lifetime and releases it when the last endpoint is dropped.
struct PipeWaiters {
    used: bool,
    readers: PidQueue,
    writers: PidQueue,
}

impl PipeWaiters {
    const fn new() -> Self {
        Self {
            used: false,
            readers: PidQueue::new(),
            writers: PidQueue::new(),
        }
    }
}

struct Scheduler {
    processes: [Option<Process>; MAX_PROCESSES],
    ready: ReadyQueue,
    current: Option<Pid>,
    idle_context: Context,
    idle_stack_top: u64,
    pids: PidAllocator,
    reap: ReapList,
    keyboard_waiters: PidQueue,
    pipe_waiters: [PipeWaiters; MAX_PIPES],
    ticks_remaining: u32,
    timeslice_ticks: u32,
}

static SCHEDULER: SpinLock<Option<Scheduler>> = SpinLock::new(None);

impl Scheduler {
    fn new(timeslice_ticks: u32) -> Self {
        Self {
            processes: core::array::from_fn(|_| None),
            ready: ReadyQueue::new(),
            current: None,
            idle_context: Context::default(),
            idle_stack_top: gdt::boot_stack_top(),
            pids: PidAllocator::new(),
            reap: ReapList::new(),
            keyboard_waiters: PidQueue::new(),
            pipe_waiters: core::array::from_fn(|_| PipeWaiters::new()),
            ticks_remaining: timeslice_ticks,
            timeslice_ticks,
        }
    }

    fn process_mut(&mut self, pid: Pid) -> Option<&mut Process> {
        self.processes
            .iter_mut()
            .flatten()
            .find(|process| process.pid == pid)
    }

    fn allocate_pipe_id(&mut self) -> Option<usize> {
        for (id, slot) in self.pipe_waiters.iter_mut().enumerate() {
            if !slot.used {
                *slot = PipeWaiters::new();
                slot.used = true;
                return Some(id);
            }
        }
        None
    }

    fn release_pipe_id(&mut self, id: usize) {
        if let Some(slot) = self.pipe_waiters.get_mut(id) {
            *slot = PipeWaiters::new();
        }
    }

    fn register_pipe_waiter(&mut self, id: usize, direction: PipeDirection, pid: Pid) -> bool {
        let Some(slot) = self.pipe_waiters.get_mut(id) else {
            return false;
        };
        let queue = match direction {
            PipeDirection::Read => &mut slot.readers,
            PipeDirection::Write => &mut slot.writers,
        };
        queue.push(pid).is_ok()
    }

    fn has_zombie_child(&self, parent: Pid, target: WaitTarget) -> bool {
        self.processes.iter().flatten().any(|process| {
            process.parent == Some(parent)
                && target.matches(process.pid)
                && process.state == ProcessState::Zombie
        })
    }

    fn process(&self, pid: Pid) -> Option<&Process> {
        self.processes
            .iter()
            .flatten()
            .find(|process| process.pid == pid)
    }

    /// Rinux has no credentials yet, so every process shares one identity.
    /// PID 1 is the exception: it must survive to reap orphans, so only init
    /// may signal itself.
    fn can_signal(&self, sender: Pid, target: Pid) -> Result<(), SignalError> {
        if self.process(target).is_none() {
            return Err(SignalError::NoSuchProcess);
        }
        if target == INIT_PID && sender != INIT_PID {
            return Err(SignalError::PermissionDenied);
        }
        Ok(())
    }

    fn queue_signal(&mut self, target: Pid, signal: Signal) -> Result<(), SignalError> {
        let process = self
            .process_mut(target)
            .ok_or(SignalError::NoSuchProcess)?;
        process.signals.pending.insert(signal);
        let wake =
            process.state == ProcessState::Blocked && process.signals.needs_delivery(signal);
        if wake {
            self.wake_for_signal(target);
        }
        Ok(())
    }

    /// Moves a blocked task back to the ready queue after a signal arrived.
    ///
    /// The task may be registered in any wait queue (keyboard, either pipe
    /// side). Every registration is removed so a later wakeup cannot resurrect
    /// a stale entry, and `waiting_for` is cleared for the same reason.
    fn wake_for_signal(&mut self, pid: Pid) {
        let blocked = self
            .process_mut(pid)
            .map(|process| process.state == ProcessState::Blocked)
            .unwrap_or(false);
        if !blocked {
            return;
        }
        let _ = self.ready.remove(pid);
        let _ = self.keyboard_waiters.remove(pid);
        for waiters in self.pipe_waiters.iter_mut() {
            let _ = waiters.readers.remove(pid);
            let _ = waiters.writers.remove(pid);
        }
        if let Some(process) = self.process_mut(pid) {
            process.state = ProcessState::Ready;
            process.waiting_for = None;
            process.signals.interrupted = true;
        }
        let _ = self.ready.push(pid);
    }
}

pub fn init() -> Result<(), SchedulerError> {
    let result = cpu::without_interrupts(|| {
        let mut guard = SCHEDULER.lock();
        if guard.is_some() {
            return Err(SchedulerError::AlreadyInitialized);
        }
        *guard = Some(Scheduler::new(TIMESLICE_TICKS));
        Ok(())
    });
    if result.is_ok() {
        crate::fs::pipe::set_hooks(pipe_wake_hook, pipe_release_hook);
    }
    result
}

fn pipe_wake_hook(id: usize, direction: PipeDirection) {
    wake_pipe(id, direction);
}

fn pipe_release_hook(id: usize) {
    release_pipe(id);
}

/// Reserves a pipe id for `fs::pipe::create`.
pub fn allocate_pipe() -> Result<usize, SchedulerError> {
    cpu::without_interrupts(|| {
        let mut guard = SCHEDULER.lock();
        let scheduler = guard.as_mut().ok_or(SchedulerError::NotInitialized)?;
        scheduler
            .allocate_pipe_id()
            .ok_or(SchedulerError::PipeTableFull)
    })
}

/// Releases a pipe id once its last endpoint has been dropped.
pub fn release_pipe(id: usize) {
    cpu::without_interrupts(|| {
        let mut guard = SCHEDULER.lock();
        if let Some(scheduler) = guard.as_mut() {
            scheduler.release_pipe_id(id);
        }
    });
}

/// Blocks the current task until `ready` observes that the pipe made progress.
///
/// The predicate runs under the scheduler lock while interrupts are disabled,
/// so a wakeup racing with registration is impossible: the writer either runs
/// after registration (and finds the waiter) or before it (and the predicate
/// already observes the new data). The pipe lock is only taken inside
/// `ready`, giving the lock order scheduler -> pipe.
pub fn block_on_pipe<F>(
    id: usize,
    direction: PipeDirection,
    ready: F,
) -> Result<(), SchedulerError>
where
    F: FnOnce() -> bool,
{
    let flags = cpu::read_rflags();
    cpu::disable_interrupts();
    let wait = {
        let mut guard = SCHEDULER.lock();
        let Some(scheduler) = guard.as_mut() else {
            cpu::restore_rflags(flags);
            return Err(SchedulerError::NotInitialized);
        };
        let Some(pid) = scheduler.current else {
            cpu::restore_rflags(flags);
            return Err(SchedulerError::NoCurrentProcess);
        };
        if ready() {
            false
        } else {
            if !scheduler.register_pipe_waiter(id, direction, pid) {
                cpu::restore_rflags(flags);
                return Err(SchedulerError::InvalidPipe);
            }
            if let Some(process) = scheduler.process_mut(pid) {
                process.state = ProcessState::Blocked;
            }
            true
        }
    };
    if wait {
        schedule();
    }
    cpu::restore_rflags(flags);
    Ok(())
}

/// Readies every task blocked on one side of a pipe.
pub fn wake_pipe(id: usize, direction: PipeDirection) {
    cpu::without_interrupts(|| {
        let mut guard = SCHEDULER.lock();
        let Some(scheduler) = guard.as_mut() else {
            return;
        };
        let Some(slot) = scheduler.pipe_waiters.get_mut(id) else {
            return;
        };
        let queue = match direction {
            PipeDirection::Read => &mut slot.readers,
            PipeDirection::Write => &mut slot.writers,
        };
        let mut woken = [None; MAX_PROCESSES];
        let mut count = 0;
        while let Some(pid) = queue.pop() {
            if count < MAX_PROCESSES {
                woken[count] = Some(pid);
                count += 1;
            }
        }
        for pid in woken.iter().take(count).flatten() {
            if let Some(process) = scheduler.process_mut(*pid) {
                if process.state == ProcessState::Blocked {
                    process.state = ProcessState::Ready;
                    let _ = scheduler.ready.push(*pid);
                }
            }
        }
    });
}

pub fn spawn(entry: TaskEntry) -> Result<Pid, SpawnError> {
    cpu::without_interrupts(|| {
        let mut guard = SCHEDULER.lock();
        let scheduler = guard.as_mut().ok_or(SpawnError::NotInitialized)?;
        let slot = scheduler
            .processes
            .iter()
            .position(|process| process.is_none())
            .ok_or(SpawnError::NoFreeSlot)?;
        let pid = scheduler
            .pids
            .allocate()
            .map_err(|_| SpawnError::PidExhausted)?;
        let process = Process::new(pid, entry).map_err(|_| SpawnError::OutOfMemory)?;
        scheduler.processes[slot] = Some(process);
        if scheduler.ready.push(pid).is_err() {
            scheduler.processes[slot] = None;
            return Err(SpawnError::QueueFull);
        }
        Ok(pid)
    })
}

/// Spawns the first kernel thread and guarantees it owns PID 1.
pub fn spawn_init(entry: TaskEntry) -> Result<Pid, SpawnError> {
    let pid = spawn(entry)?;
    if pid != INIT_PID {
        return Err(SpawnError::InitNotFirst);
    }
    Ok(pid)
}

pub fn fork_current(frame: &SyscallFrame) -> Result<Pid, ForkError> {
    let mut discarded: Option<Process> = None;
    let result = cpu::without_interrupts(|| {
        let mut guard = SCHEDULER.lock();
        let scheduler = guard.as_mut().ok_or(ForkError::NoCurrent)?;
        let parent_pid = scheduler.current.ok_or(ForkError::NoCurrent)?;
        let (address_space, fds, signals) = {
            let parent = scheduler
                .process_mut(parent_pid)
                .ok_or(ForkError::NoCurrent)?;
            let address_space = parent
                .address_space
                .clone_user()
                .map_err(|_| ForkError::OutOfMemory)?;
            (
                address_space,
                parent.fds.clone(),
                SignalState::forked(&parent.signals),
            )
        };
        let slot = scheduler
            .processes
            .iter()
            .position(|process| process.is_none())
            .ok_or(ForkError::NoFreeSlot)?;
        let pid = match scheduler.pids.allocate() {
            Ok(pid) => pid,
            Err(_) => {
                return Err(ForkError::PidExhausted);
            }
        };
        let process = Process::new_fork(pid, parent_pid, address_space, fds, signals, frame)
            .map_err(|_| ForkError::OutOfMemory)?;
        scheduler.processes[slot] = Some(process);
        if scheduler.ready.push(pid).is_err() {
            // The child inherited pipe endpoints; detach and drop it outside
            // the scheduler lock.
            discarded = scheduler.processes[slot].take();
            return Err(ForkError::QueueFull);
        }
        Ok(pid)
    });
    drop(discarded);
    result
}

/// Attempts to resolve a COW write fault for the running process.
///
/// Called from the page-fault entry path; the scheduler lock also protects the
/// current address space from being torn down while the leaf is rewritten.
pub fn resolve_cow_fault(virt_addr: usize) -> bool {
    cpu::without_interrupts(|| {
        let mut guard = SCHEDULER.lock();
        let Some(scheduler) = guard.as_mut() else {
            return false;
        };
        let Some(pid) = scheduler.current else {
            return false;
        };
        match scheduler.process_mut(pid) {
            Some(process) if process.address_space.root() == paging::active_root() => {
                process.address_space.handle_cow_fault(virt_addr).is_ok()
            }
            _ => false,
        }
    })
}

/// Queues `signal` for `target`, waking it when the signal needs service.
pub fn send_signal(target: Pid, signal: Signal) -> Result<(), SignalError> {
    cpu::without_interrupts(|| {
        let mut guard = SCHEDULER.lock();
        let scheduler = guard.as_mut().ok_or(SignalError::NotInitialized)?;
        let sender = scheduler.current.ok_or(SignalError::NoCurrentProcess)?;
        scheduler.can_signal(sender, target)?;
        scheduler.queue_signal(target, signal)
    })
}

/// Broadcast form of [`send_signal`], used by `kill(-1, sig)`.
///
/// PID 1 and the caller are skipped; a `None` signal performs the existence
/// check without queueing anything.
pub fn send_signal_all(signal: Option<Signal>) -> Result<(), SignalError> {
    cpu::without_interrupts(|| {
        let mut guard = SCHEDULER.lock();
        let scheduler = guard.as_mut().ok_or(SignalError::NotInitialized)?;
        let sender = scheduler.current.ok_or(SignalError::NoCurrentProcess)?;
        let mut targets = [None; MAX_PROCESSES];
        let mut count = 0;
        for process in scheduler.processes.iter().flatten() {
            if process.pid == sender || process.pid == INIT_PID {
                continue;
            }
            if count == MAX_PROCESSES {
                break;
            }
            targets[count] = Some(process.pid);
            count += 1;
        }
        if let Some(signal) = signal {
            for target in targets.iter().take(count).flatten() {
                if scheduler.can_signal(sender, *target).is_ok() {
                    let _ = scheduler.queue_signal(*target, signal);
                }
            }
        }
        Ok(())
    })
}

/// Existence and permission check for `kill(pid, 0)`.
pub fn check_signal_target(target: Pid) -> Result<(), SignalError> {
    cpu::without_interrupts(|| {
        let guard = SCHEDULER.lock();
        let scheduler = guard.as_ref().ok_or(SignalError::NotInitialized)?;
        let sender = scheduler.current.ok_or(SignalError::NoCurrentProcess)?;
        scheduler.can_signal(sender, target)
    })
}

/// Installs `action` for `signal` and returns the previous disposition.
pub fn install_signal_action(
    signal: Signal,
    action: SigAction,
) -> Result<SigAction, SignalError> {
    if signal.is_uncatchable() {
        return Err(SignalError::Uncatchable);
    }
    if !action.flags_supported() {
        return Err(SignalError::UnsupportedFlags);
    }
    if action.is_caught() && !uaccess::program_valid(action.handler as usize, 1) {
        return Err(SignalError::InvalidHandler);
    }
    if action.uses_restorer() && !uaccess::program_valid(action.restorer as usize, 1) {
        return Err(SignalError::InvalidHandler);
    }
    let action = SigAction {
        mask: action.mask.without_uncatchable(),
        ..action
    };
    cpu::without_interrupts(|| {
        let mut guard = SCHEDULER.lock();
        let scheduler = guard.as_mut().ok_or(SignalError::NotInitialized)?;
        let pid = scheduler
            .current
            .ok_or(SignalError::NoCurrentProcess)?;
        let process = scheduler
            .process_mut(pid)
            .ok_or(SignalError::NoCurrentProcess)?;
        let previous = process.signals.actions[signal.index()];
        process.signals.actions[signal.index()] = action;
        Ok(previous)
    })
}

/// Reads the current disposition of `signal`.
pub fn signal_action(signal: Signal) -> Result<SigAction, SignalError> {
    if signal.is_uncatchable() {
        return Err(SignalError::Uncatchable);
    }
    cpu::without_interrupts(|| {
        let guard = SCHEDULER.lock();
        let scheduler = guard.as_ref().ok_or(SignalError::NotInitialized)?;
        let pid = scheduler
            .current
            .ok_or(SignalError::NoCurrentProcess)?;
        let process = scheduler
            .process(pid)
            .ok_or(SignalError::NoCurrentProcess)?;
        Ok(process.signals.actions[signal.index()])
    })
}

/// Returns the calling process's current blocked-signal mask.
pub fn blocked_signals() -> Result<SigSet, SignalError> {
    cpu::without_interrupts(|| {
        let guard = SCHEDULER.lock();
        let scheduler = guard.as_ref().ok_or(SignalError::NotInitialized)?;
        let pid = scheduler
            .current
            .ok_or(SignalError::NoCurrentProcess)?;
        let process = scheduler
            .process(pid)
            .ok_or(SignalError::NoCurrentProcess)?;
        Ok(process.signals.blocked)
    })
}

/// Applies `rt_sigprocmask`'s `how`/`set` to the calling process and returns
/// the previous mask. `SIGKILL` and `SIGSTOP` are silently removed.
pub fn set_blocked_signals(how: u64, set: SigSet) -> Result<SigSet, SignalError> {
    let set = set.without_uncatchable();
    cpu::without_interrupts(|| {
        let mut guard = SCHEDULER.lock();
        let scheduler = guard.as_mut().ok_or(SignalError::NotInitialized)?;
        let pid = scheduler
            .current
            .ok_or(SignalError::NoCurrentProcess)?;
        let process = scheduler
            .process_mut(pid)
            .ok_or(SignalError::NoCurrentProcess)?;
        let previous = process.signals.blocked;
        let updated = match how {
            SIG_BLOCK => previous.union(set),
            SIG_UNBLOCK => previous.difference(set),
            SIG_SETMASK => set,
            _ => return Err(SignalError::InvalidHow),
        };
        process.signals.blocked = updated.without_uncatchable();
        Ok(previous)
    })
}

/// Exec resets: caught dispositions become `SIG_DFL`, ignored ones stay
/// ignored, pending signals are discarded and handler nesting restarts.
pub fn reset_signals_for_exec() -> Result<(), SignalError> {
    cpu::without_interrupts(|| {
        let mut guard = SCHEDULER.lock();
        let scheduler = guard.as_mut().ok_or(SignalError::NotInitialized)?;
        let pid = scheduler
            .current
            .ok_or(SignalError::NoCurrentProcess)?;
        let process = scheduler
            .process_mut(pid)
            .ok_or(SignalError::NoCurrentProcess)?;
        let signals = &mut process.signals;
        for action in signals.actions.iter_mut() {
            if action.is_caught() {
                *action = SigAction::DEFAULT;
            }
        }
        signals.pending = SigSet::EMPTY;
        signals.depth = 0;
        signals.interrupted = false;
        Ok(())
    })
}

pub fn task_count() -> usize {
    cpu::without_interrupts(|| {
        let guard = SCHEDULER.lock();
        match guard.as_ref() {
            Some(scheduler) => scheduler
                .processes
                .iter()
                .flatten()
                .filter(|process| process.state != ProcessState::Zombie)
                .count(),
            None => 0,
        }
    })
}

pub fn ready_count() -> usize {
    cpu::without_interrupts(|| {
        let guard = SCHEDULER.lock();
        match guard.as_ref() {
            Some(scheduler) => scheduler.ready.len(),
            None => 0,
        }
    })
}

pub fn current_pid() -> Option<Pid> {
    cpu::without_interrupts(|| {
        let guard = SCHEDULER.lock();
        guard.as_ref().and_then(|scheduler| scheduler.current)
    })
}

/// Installs fd 0/1/2 for the running process.
pub fn init_current_stdio() -> Result<(), SchedulerError> {
    cpu::without_interrupts(|| {
        let mut guard = SCHEDULER.lock();
        let scheduler = guard.as_mut().ok_or(SchedulerError::NotInitialized)?;
        let pid = scheduler
            .current
            .ok_or(SchedulerError::NoCurrentProcess)?;
        let process = scheduler
            .process_mut(pid)
            .ok_or(SchedulerError::NoCurrentProcess)?;
        process.fds.init_stdio();
        Ok(())
    })
}

pub fn current_fd_entry(fd: u32) -> Result<FdEntry, FdError> {
    cpu::without_interrupts(|| {
        let guard = SCHEDULER.lock();
        let scheduler = guard.as_ref().ok_or(FdError::NoProcess)?;
        let pid = scheduler.current.ok_or(FdError::NoProcess)?;
        let process = scheduler
            .processes
            .iter()
            .flatten()
            .find(|process| process.pid == pid)
            .ok_or(FdError::NoProcess)?;
        process.fds.entry(fd)
    })
}

pub fn current_open_fd(node: u32) -> Result<u32, FdError> {
    cpu::without_interrupts(|| {
        let mut guard = SCHEDULER.lock();
        let scheduler = guard.as_mut().ok_or(FdError::NoProcess)?;
        let pid = scheduler.current.ok_or(FdError::NoProcess)?;
        let process = scheduler.process_mut(pid).ok_or(FdError::NoProcess)?;
        process.fds.open_file(node)
    })
}

pub fn current_close_fd(fd: u32) -> Result<(), FdError> {
    let removed = cpu::without_interrupts(|| {
        let mut guard = SCHEDULER.lock();
        let scheduler = guard.as_mut().ok_or(FdError::NoProcess)?;
        let pid = scheduler.current.ok_or(FdError::NoProcess)?;
        let process = scheduler.process_mut(pid).ok_or(FdError::NoProcess)?;
        process.fds.close(fd)
    })?;
    // Dropping a pipe endpoint may wake blocked peers, which takes the
    // scheduler lock again. Never run that destructor under the lock.
    drop(removed);
    Ok(())
}

/// Installs both ends of a new pipe and returns their descriptors.
///
/// Nothing can be installed under the scheduler lock: if the second descriptor
/// is unavailable the first one is detached and dropped after unlocking, since
/// a pipe endpoint destructor calls back into the scheduler to wake peers.
pub fn current_install_pipe(
    read: &Arc<crate::fs::pipe::PipeEnd>,
    write: &Arc<crate::fs::pipe::PipeEnd>,
) -> Result<(u32, u32), FdError> {
    let mut rollback: Option<FdEntry> = None;
    let result = cpu::without_interrupts(|| {
        let mut guard = SCHEDULER.lock();
        let scheduler = guard.as_mut().ok_or(FdError::NoProcess)?;
        let pid = scheduler.current.ok_or(FdError::NoProcess)?;
        let process = scheduler.process_mut(pid).ok_or(FdError::NoProcess)?;
        if process.fds.free_count() < 2 {
            return Err(FdError::NoFreeFd);
        }
        let read_fd = process
            .fds
            .install(FdEntry::Pipe(read.clone()))
            .map_err(|_| FdError::NoFreeFd)?;
        let write_fd = match process.fds.install(FdEntry::Pipe(write.clone())) {
            Ok(fd) => fd,
            Err(_) => {
                rollback = process.fds.take(read_fd).ok();
                return Err(FdError::NoFreeFd);
            }
        };
        Ok((read_fd, write_fd))
    });
    drop(rollback);
    result
}

pub fn current_set_fd_offset(fd: u32, offset: u64) -> Result<(), FdError> {
    cpu::without_interrupts(|| {
        let mut guard = SCHEDULER.lock();
        let scheduler = guard.as_mut().ok_or(FdError::NoProcess)?;
        let pid = scheduler.current.ok_or(FdError::NoProcess)?;
        let process = scheduler.process_mut(pid).ok_or(FdError::NoProcess)?;
        process.fds.set_offset(fd, offset)
    })
}

pub fn replace_address_space(new: AddressSpace) -> Result<AddressSpace, SchedulerError> {
    cpu::without_interrupts(|| {
        let mut guard = SCHEDULER.lock();
        let scheduler = guard.as_mut().ok_or(SchedulerError::NotInitialized)?;
        let pid = scheduler.current.ok_or(SchedulerError::NoCurrentProcess)?;
        let process = scheduler
            .process_mut(pid)
            .ok_or(SchedulerError::NoCurrentProcess)?;
        Ok(core::mem::replace(&mut process.address_space, new))
    })
}

pub fn try_wait(target: WaitTarget) -> WaitResult {
    let (result, reaped) = cpu::without_interrupts(|| {
        let mut guard = SCHEDULER.lock();
        let scheduler = match guard.as_mut() {
            Some(scheduler) => scheduler,
            None => return (WaitResult::NoChild, None),
        };
        let parent = match scheduler.current {
            Some(parent) => parent,
            None => return (WaitResult::NoChild, None),
        };
        let mut matched = false;
        let mut reap = None;
        for (index, process) in scheduler.processes.iter().enumerate() {
            let Some(process) = process else {
                continue;
            };
            if process.parent != Some(parent) || !target.matches(process.pid) {
                continue;
            }
            matched = true;
            if process.state == ProcessState::Zombie {
                reap = Some(index);
                break;
            }
        }
        if let Some(index) = reap {
            let process = scheduler.processes[index].take();
            let pid = process.as_ref().map(|process| process.pid).unwrap_or(0);
            let status = process
                .as_ref()
                .and_then(|process| process.exit_status)
                .unwrap_or(ExitStatus::Exited(0));
            (WaitResult::Reaped(pid, status), process)
        } else if matched {
            (WaitResult::Block, None)
        } else {
            (WaitResult::NoChild, None)
        }
    });
    // The zombie may own pipe descriptors; drop it outside the scheduler lock.
    drop(reaped);
    result
}

pub fn block_on_child(target: WaitTarget) -> Result<(), SchedulerError> {
    let flags = cpu::read_rflags();
    cpu::disable_interrupts();
    let (result, wait) = {
        let mut guard = SCHEDULER.lock();
        match guard.as_mut() {
            Some(scheduler) => match scheduler.current {
                Some(pid) => {
                    // A child may have exited after `try_wait` decided to block.
                    // Recheck while registering so the wakeup cannot be lost.
                    if scheduler.has_zombie_child(pid, target) {
                        (Ok(()), false)
                    } else {
                        match scheduler.process_mut(pid) {
                            Some(process) => {
                                process.state = ProcessState::Blocked;
                                process.waiting_for = Some(target);
                                (Ok(()), true)
                            }
                            None => (Err(SchedulerError::NoCurrentProcess), false),
                        }
                    }
                }
                None => (Err(SchedulerError::NoCurrentProcess), false),
            },
            None => (Err(SchedulerError::NotInitialized), false),
        }
    };
    if wait {
        schedule();
    }
    cpu::restore_rflags(flags);
    result
}

pub fn yield_now() {
    let flags = cpu::read_rflags();
    cpu::disable_interrupts();
    schedule();
    cpu::restore_rflags(flags);
}

pub fn on_timer_tick() {
    let preempt = {
        let mut guard = SCHEDULER.lock();
        match guard.as_mut() {
            Some(scheduler) => {
                scheduler.ticks_remaining = scheduler.ticks_remaining.saturating_sub(1);
                if scheduler.ticks_remaining == 0 {
                    scheduler.ticks_remaining = scheduler.timeslice_ticks;
                    true
                } else {
                    false
                }
            }
            None => false,
        }
    };
    if preempt {
        schedule();
    }
}

pub fn block_on_keyboard() -> Result<(), SchedulerError> {
    let flags = cpu::read_rflags();
    cpu::disable_interrupts();
    let result = {
        let mut guard = SCHEDULER.lock();
        match guard.as_mut() {
            Some(scheduler) => match scheduler.current {
                Some(pid) => match scheduler.keyboard_waiters.push(pid) {
                    Ok(()) => match scheduler.process_mut(pid) {
                        Some(process) => {
                            process.state = ProcessState::Blocked;
                            Ok(())
                        }
                        None => {
                            let _ = scheduler.keyboard_waiters.remove(pid);
                            Err(SchedulerError::NoCurrentProcess)
                        }
                    },
                    Err(QueueError::Full) => Err(SchedulerError::KeyboardQueueFull),
                    Err(_) => Err(SchedulerError::NoCurrentProcess),
                },
                None => Err(SchedulerError::NoCurrentProcess),
            },
            None => Err(SchedulerError::NotInitialized),
        }
    };
    if result.is_ok() {
        schedule();
    }
    cpu::restore_rflags(flags);
    result
}

pub fn wake_keyboard_waiter() {
    let mut guard = SCHEDULER.lock();
    let Some(scheduler) = guard.as_mut() else {
        return;
    };
    loop {
        let Some(pid) = scheduler.keyboard_waiters.pop() else {
            return;
        };
        if let Some(process) = scheduler.process_mut(pid) {
            if process.state == ProcessState::Blocked {
                process.state = ProcessState::Ready;
                let _ = scheduler.ready.push_front(pid);
                return;
            }
        }
    }
}

pub fn exit_current(code: i32) -> ! {
    cpu::disable_interrupts();
    // Descriptors are closed as part of exiting, not when the zombie is
    // reaped: a blocked reader must observe end-of-file as soon as the last
    // writer is gone, even if the parent has not called `waitpid` yet.
    // Detach the table here and drop it below, outside the scheduler lock,
    // because a pipe endpoint destructor wakes peers through that same lock.
    let mut orphaned_fds: Option<FdTable> = None;
    {
        let mut guard = SCHEDULER.lock();
        if let Some(scheduler) = guard.as_mut() {
            if let Some(pid) = scheduler.current {
                if pid == INIT_PID {
                    panic!("Attempted to kill init!");
                }
                let _ = scheduler.ready.remove(pid);
                let _ = scheduler.keyboard_waiters.remove(pid);
                for waiters in scheduler.pipe_waiters.iter_mut() {
                    let _ = waiters.readers.remove(pid);
                    let _ = waiters.writers.remove(pid);
                }
                let parent = match scheduler.process_mut(pid) {
                    Some(process) => {
                        process.state = ProcessState::Zombie;
                        process.exit_status = Some(ExitStatus::Exited(code));
                        orphaned_fds = Some(core::mem::replace(
                            &mut process.fds,
                            FdTable::empty(),
                        ));
                        process.parent
                    }
                    None => None,
                };
                let mut parent_alive = false;
                if let Some(parent_pid) = parent {
                    if let Some(parent) = scheduler.process_mut(parent_pid) {
                        if parent.state != ProcessState::Zombie {
                            parent_alive = true;
                            if parent.state == ProcessState::Blocked
                                && parent
                                    .waiting_for
                                    .map(|target| target.matches(pid))
                                    .unwrap_or(false)
                            {
                                parent.state = ProcessState::Ready;
                                parent.waiting_for = None;
                                let _ = scheduler.ready.push(parent_pid);
                            }
                        }
                    }
                }
                // Every orphan is reparented to PID 1, including zombies that
                // init will later collect with waitpid.
                for process in scheduler.processes.iter_mut().flatten() {
                    if process.parent == Some(pid) {
                        process.parent = Some(INIT_PID);
                    }
                }
                if !parent_alive {
                    let _ = scheduler.reap.push(pid);
                }
            }
        }
    }
    // Pipe endpoints (and any other owned descriptors) are released here,
    // where waking blocked peers cannot re-enter a held scheduler lock.
    drop(orphaned_fds);
    crate::kprintln!("process exited with code {}", code);
    schedule();
    cpu::halt_forever()
}

/// Detaches reaped processes and drops them without holding the scheduler lock.
///
/// Dropping a process frees its address space and closes its descriptor table;
/// a pipe endpoint destructor may in turn call back into the scheduler to wake
/// peers, so it must never run under `SCHEDULER`.
fn reap_processes() {
    loop {
        let pid = cpu::without_interrupts(|| {
            let mut guard = SCHEDULER.lock();
            guard
                .as_mut()
                .and_then(|scheduler| scheduler.reap.take_reapable(scheduler.current))
        });
        let Some(pid) = pid else {
            break;
        };
        let process = cpu::without_interrupts(|| {
            let mut guard = SCHEDULER.lock();
            let scheduler = guard.as_mut()?;
            let slot = scheduler
                .processes
                .iter()
                .position(|process| process.as_ref().map_or(false, |process| process.pid == pid))?;
            scheduler.processes[slot].take()
        });
        drop(process);
    }
}

fn schedule() {
    reap_processes();
    let (old_context, new_context, next_top, next_root) = {
        let mut guard = SCHEDULER.lock();
        let Some(scheduler) = guard.as_mut() else {
            return;
        };

        let current = scheduler.current;

        if let Some(pid) = current {
            let requeue = match scheduler.process_mut(pid) {
                Some(process) if process.state == ProcessState::Running => {
                    process.state = ProcessState::Ready;
                    true
                }
                _ => false,
            };
            if requeue {
                let _ = scheduler.ready.push(pid);
            }
        }

        let next = scheduler.ready.pop();

        let old_context = match current {
            Some(pid) => match scheduler.process_mut(pid) {
                Some(process) => core::ptr::addr_of_mut!(process.context),
                None => core::ptr::addr_of_mut!(scheduler.idle_context),
            },
            None => core::ptr::addr_of_mut!(scheduler.idle_context),
        };

        let (new_context, next_top, next_root) = match next {
            Some(pid) => match scheduler.process_mut(pid) {
                Some(process) => {
                    process.state = ProcessState::Running;
                    let context = core::ptr::addr_of_mut!(process.context);
                    let top = process.kernel_stack_top;
                    let root = process.address_space.root();
                    scheduler.current = Some(pid);
                    (context, top, root)
                }
                None => (
                    core::ptr::addr_of_mut!(scheduler.idle_context),
                    scheduler.idle_stack_top,
                    paging::kernel_root(),
                ),
            },
            None => (
                core::ptr::addr_of_mut!(scheduler.idle_context),
                scheduler.idle_stack_top,
                paging::kernel_root(),
            ),
        };

        (old_context, new_context, next_top, next_root)
    };

    gdt::set_rsp0(next_top);

    if old_context == new_context {
        return;
    }

    if next_root != paging::active_root() {
        cpu::write_cr3(next_root);
    }

    unsafe { context::switch(old_context, new_context) };
}
