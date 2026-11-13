use super::context::{self, Context};
use super::pid::{Pid, PidAllocator};
use super::process::{free_user_pages, Process, ProcessState, TaskEntry, UserPage};
use super::queue::{ReadyQueue, MAX_PROCESSES};
use super::reap::ReapList;
use crate::arch::x86_64::cpu;
use crate::arch::x86_64::gdt;
use crate::sync::SpinLock;

pub const TIMESLICE_TICKS: u32 = 10;
pub const TIMESLICE_MS: u64 = 100;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SchedulerError {
    AlreadyInitialized,
    NotInitialized,
    NoCurrentProcess,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpawnError {
    NotInitialized,
    NoFreeSlot,
    PidExhausted,
    OutOfMemory,
    QueueFull,
}

struct Scheduler {
    processes: [Option<Process>; MAX_PROCESSES],
    ready: ReadyQueue,
    current: Option<Pid>,
    idle_context: Context,
    idle_stack_top: u64,
    pids: PidAllocator,
    reap: ReapList,
    keyboard_waiter: Option<Pid>,
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
            keyboard_waiter: None,
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

pub fn spawn_user(
    entry: u64,
    stack_top: u64,
    pages: alloc::vec::Vec<UserPage>,
) -> Result<Pid, SpawnError> {
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
        let process =
            Process::new_user(pid, entry, stack_top, pages).map_err(|_| SpawnError::OutOfMemory)?;
        scheduler.processes[slot] = Some(process);
        if scheduler.ready.push(pid).is_err() {
            if let Some(mut process) = scheduler.processes[slot].take() {
                free_user_pages(&mut process.user_pages);
            }
            return Err(SpawnError::QueueFull);
        }
        Ok(pid)
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
                .filter(|process| process.state != ProcessState::Terminated)
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
                Some(pid) => {
                    let blocked = match scheduler.process_mut(pid) {
                        Some(process) => {
                            process.state = ProcessState::Blocked;
                            true
                        }
                        None => false,
                    };
                    if blocked {
                        scheduler.keyboard_waiter = Some(pid);
                        Ok(())
                    } else {
                        Err(SchedulerError::NoCurrentProcess)
                    }
                }
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
    let Some(pid) = scheduler.keyboard_waiter.take() else {
        return;
    };
    let woken = match scheduler.process_mut(pid) {
        Some(process) if process.state == ProcessState::Blocked => {
            process.state = ProcessState::Ready;
            true
        }
        _ => false,
    };
    if woken {
        let _ = scheduler.ready.push_front(pid);
    }
}

pub fn exit_current(code: i32) -> ! {
    cpu::disable_interrupts();
    {
        let mut guard = SCHEDULER.lock();
        if let Some(scheduler) = guard.as_mut() {
            if let Some(pid) = scheduler.current {
                let _ = scheduler.ready.remove(pid);
                if let Some(process) = scheduler.process_mut(pid) {
                    process.state = ProcessState::Terminated;
                    process.exit_code = Some(code);
                }
                let _ = scheduler.reap.push(pid);
            }
        }
    }
    crate::kprintln!("process exited with code {}", code);
    schedule();
    cpu::halt_forever()
}

fn schedule() {
    let (old_context, new_context, next_top) = {
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
                if let Some(mut process) = scheduler.processes[slot].take() {
                    free_user_pages(&mut process.user_pages);
                }
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

        let (new_context, next_top) = match next {
            Some(pid) => match scheduler.process_mut(pid) {
                Some(process) => {
                    process.state = ProcessState::Running;
                    let context = core::ptr::addr_of_mut!(process.context);
                    let top = process.kernel_stack_top;
                    scheduler.current = Some(pid);
                    (context, top)
                }
                None => (
                    core::ptr::addr_of_mut!(scheduler.idle_context),
                    scheduler.idle_stack_top,
                ),
            },
            None => (
                core::ptr::addr_of_mut!(scheduler.idle_context),
                scheduler.idle_stack_top,
            ),
        };

        (old_context, new_context, next_top)
    };

    gdt::set_rsp0(next_top);

    if old_context == new_context {
        return;
    }

    unsafe { context::switch(old_context, new_context) };
}
