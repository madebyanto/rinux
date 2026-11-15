use super::context::{self, Context};
use super::pid::{Pid, PidAllocator, INIT_PID};
use super::process::{Process, ProcessState, TaskEntry};
use super::queue::{ReadyQueue, MAX_PROCESSES};
use super::reap::ReapList;
use super::wait::WaitTarget;
use super::wait_queue::{PidQueue, QueueError};
use crate::arch::x86_64::cpu;
use crate::arch::x86_64::gdt;
use crate::arch::x86_64::syscall::SyscallFrame;
use crate::fs::fd::{FdEntry, FdError};
use crate::mm::address_space::AddressSpace;
use crate::mm::paging;
use crate::sync::SpinLock;

pub const TIMESLICE_TICKS: u32 = 10;
pub const TIMESLICE_MS: u64 = 100;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SchedulerError {
    AlreadyInitialized,
    NotInitialized,
    NoCurrentProcess,
    KeyboardQueueFull,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WaitResult {
    Reaped(Pid, i32),
    Block,
    NoChild,
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
}

pub fn init() -> Result<(), SchedulerError> {
    cpu::without_interrupts(|| {
        let mut guard = SCHEDULER.lock();
        if guard.is_some() {
            return Err(SchedulerError::AlreadyInitialized);
        }
        *guard = Some(Scheduler::new(TIMESLICE_TICKS));
        Ok(())
    })
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
    cpu::without_interrupts(|| {
        let mut guard = SCHEDULER.lock();
        let scheduler = guard.as_mut().ok_or(ForkError::NoCurrent)?;
        let parent_pid = scheduler.current.ok_or(ForkError::NoCurrent)?;
        let (address_space, fds) = {
            let parent = scheduler
                .process_mut(parent_pid)
                .ok_or(ForkError::NoCurrent)?;
            let address_space = parent
                .address_space
                .clone_user()
                .map_err(|_| ForkError::OutOfMemory)?;
            (address_space, parent.fds.clone())
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
        let process = Process::new_fork(pid, parent_pid, address_space, fds, frame)
            .map_err(|_| ForkError::OutOfMemory)?;
        scheduler.processes[slot] = Some(process);
        if scheduler.ready.push(pid).is_err() {
            scheduler.processes[slot] = None;
            return Err(ForkError::QueueFull);
        }
        Ok(pid)
    })
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
    cpu::without_interrupts(|| {
        let mut guard = SCHEDULER.lock();
        let scheduler = guard.as_mut().ok_or(FdError::NoProcess)?;
        let pid = scheduler.current.ok_or(FdError::NoProcess)?;
        let process = scheduler.process_mut(pid).ok_or(FdError::NoProcess)?;
        process.fds.close(fd)
    })
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
    cpu::without_interrupts(|| {
        let mut guard = SCHEDULER.lock();
        let scheduler = match guard.as_mut() {
            Some(scheduler) => scheduler,
            None => return WaitResult::NoChild,
        };
        let parent = match scheduler.current {
            Some(parent) => parent,
            None => return WaitResult::NoChild,
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
            let pid = scheduler.processes[index]
                .as_ref()
                .map(|process| process.pid)
                .unwrap_or(0);
            let code = scheduler.processes[index]
                .as_ref()
                .and_then(|process| process.exit_code)
                .unwrap_or(0);
            let _ = scheduler.processes[index].take();
            WaitResult::Reaped(pid, code)
        } else if matched {
            WaitResult::Block
        } else {
            WaitResult::NoChild
        }
    })
}

pub fn block_on_child(target: WaitTarget) -> Result<(), SchedulerError> {
    let flags = cpu::read_rflags();
    cpu::disable_interrupts();
    let result = {
        let mut guard = SCHEDULER.lock();
        match guard.as_mut() {
            Some(scheduler) => match scheduler.current {
                Some(pid) => match scheduler.process_mut(pid) {
                    Some(process) => {
                        process.state = ProcessState::Blocked;
                        process.waiting_for = Some(target);
                        Ok(())
                    }
                    None => Err(SchedulerError::NoCurrentProcess),
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
    {
        let mut guard = SCHEDULER.lock();
        if let Some(scheduler) = guard.as_mut() {
            if let Some(pid) = scheduler.current {
                if pid == INIT_PID {
                    panic!("Attempted to kill init!");
                }
                let _ = scheduler.ready.remove(pid);
                let _ = scheduler.keyboard_waiters.remove(pid);
                let parent = match scheduler.process_mut(pid) {
                    Some(process) => {
                        process.state = ProcessState::Zombie;
                        process.exit_code = Some(code);
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
    crate::kprintln!("process exited with code {}", code);
    schedule();
    cpu::halt_forever()
}

fn schedule() {
    let (old_context, new_context, next_top, next_root) = {
        let mut guard = SCHEDULER.lock();
        let Some(scheduler) = guard.as_mut() else {
            return;
        };

        let current = scheduler.current;
        while let Some(pid) = scheduler.reap.take_reapable(current) {
            let slot = scheduler
                .processes
                .iter()
                .position(|process| process.as_ref().map_or(false, |process| process.pid == pid));
            if let Some(slot) = slot {
                let _ = scheduler.processes[slot].take();
            }
        }

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
