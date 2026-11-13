use super::context::{self, Context};
use super::pid::{Pid, PidAllocator};
use super::process::{Process, ProcessState, TaskEntry};
use super::queue::{ReadyQueue, MAX_PROCESSES};
use crate::arch::x86_64::cpu;
use crate::sync::SpinLock;

pub const TIMESLICE_TICKS: u32 = 10;
pub const TIMESLICE_MS: u64 = 100;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SchedulerError {
    AlreadyInitialized,
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
    pids: PidAllocator,
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
            pids: PidAllocator::new(),
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
            }
        }
    }
    schedule();
    cpu::halt_forever()
}

fn schedule() {
    let (old_context, new_context) = {
        let mut guard = SCHEDULER.lock();
        let Some(scheduler) = guard.as_mut() else {
            return;
        };

        let previous = scheduler.current;
        if let Some(pid) = previous {
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

        let old_context = match previous {
            Some(pid) => match scheduler.process_mut(pid) {
                Some(process) => core::ptr::addr_of_mut!(process.context),
                None => core::ptr::addr_of_mut!(scheduler.idle_context),
            },
            None => core::ptr::addr_of_mut!(scheduler.idle_context),
        };

        let new_context = match next {
            Some(pid) => match scheduler.process_mut(pid) {
                Some(process) => {
                    process.state = ProcessState::Running;
                    let context = core::ptr::addr_of_mut!(process.context);
                    scheduler.current = Some(pid);
                    context
                }
                None => {
                    scheduler.current = None;
                    core::ptr::addr_of_mut!(scheduler.idle_context)
                }
            },
            None => {
                scheduler.current = None;
                core::ptr::addr_of_mut!(scheduler.idle_context)
            }
        };

        (old_context, new_context)
    };

    if old_context == new_context {
        return;
    }

    unsafe { context::switch(old_context, new_context) };
}
