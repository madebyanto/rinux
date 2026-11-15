//! Bounded kernel pipes shared by a read and a write endpoint.
//!
//! The byte ring lives behind a `SpinLock`. Endpoint lifetime is tracked with
//! `Arc`, so the two file descriptors created by `pipe` share one buffer and
//! `fork` inherits both ends; an end is considered closed only when every
//! duplicated descriptor has been dropped.
//!
//! Blocking is deliberately not implemented here. `try_read`/`try_write`
//! report `Empty`/`Full`, and the syscall layer waits on the scheduler's pipe
//! wait queues before retrying. This keeps the ring buffer host-testable and
//! avoids taking the scheduler lock from inside the pipe lock.
//!
//! Closing the last writer wakes blocked readers so they observe `EndOfFile`;
//! closing the last reader wakes blocked writers so they observe `BrokenPipe`.
//! Those notifications go through small hooks installed by the scheduler, and
//! are no-ops in host tests.

use crate::sync::SpinLock;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::fmt;

/// Capacity of every pipe, matching one page.
pub const PIPE_CAPACITY: usize = 4096;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PipeDirection {
    Read,
    Write,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PipeError {
    NoResources,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReadOutcome {
    Data(usize),
    Empty,
    EndOfFile,
    WrongDirection,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WriteOutcome {
    Written(usize),
    Full,
    BrokenPipe,
    WrongDirection,
}

/// Wake/release notifications used by the blocking layer.
#[derive(Clone, Copy)]
pub struct PipeHooks {
    pub wake: fn(usize, PipeDirection),
    pub release: fn(usize),
}

static HOOKS: SpinLock<Option<PipeHooks>> = SpinLock::new(None);

/// Installs the scheduler callbacks. Called once from `scheduler::init`.
pub fn set_hooks(wake: fn(usize, PipeDirection), release: fn(usize)) {
    *HOOKS.lock() = Some(PipeHooks { wake, release });
}

fn notify_wake(id: usize, direction: PipeDirection) {
    let hook = HOOKS.lock().as_ref().map(|hooks| hooks.wake);
    if let Some(wake) = hook {
        wake(id, direction);
    }
}

fn notify_release(id: usize) {
    let hook = HOOKS.lock().as_ref().map(|hooks| hooks.release);
    if let Some(release) = hook {
        release(id);
    }
}

struct PipeState {
    buffer: Vec<u8>,
    start: usize,
    len: usize,
    readers: u32,
    writers: u32,
}

impl PipeState {
    fn read_from(&mut self, out: &mut [u8]) -> ReadOutcome {
        if out.is_empty() {
            return ReadOutcome::Data(0);
        }
        if self.len == 0 {
            return if self.writers == 0 {
                ReadOutcome::EndOfFile
            } else {
                ReadOutcome::Empty
            };
        }
        let count = out.len().min(self.len);
        let capacity = self.buffer.len();
        for (index, byte) in out.iter_mut().take(count).enumerate() {
            *byte = self.buffer[(self.start + index) % capacity];
        }
        self.start = (self.start + count) % capacity;
        self.len -= count;
        ReadOutcome::Data(count)
    }

    fn write_from(&mut self, data: &[u8]) -> WriteOutcome {
        if data.is_empty() {
            return WriteOutcome::Written(0);
        }
        if self.readers == 0 {
            return WriteOutcome::BrokenPipe;
        }
        let capacity = self.buffer.len();
        let space = capacity - self.len;
        if space == 0 {
            return WriteOutcome::Full;
        }
        let count = data.len().min(space);
        for (index, byte) in data.iter().take(count).enumerate() {
            let tail = (self.start + self.len + index) % capacity;
            self.buffer[tail] = *byte;
        }
        self.len += count;
        WriteOutcome::Written(count)
    }
}

pub struct Pipe {
    id: usize,
    state: SpinLock<PipeState>,
}

impl Pipe {
    fn new(id: usize) -> Result<Self, PipeError> {
        let mut buffer = Vec::new();
        buffer
            .try_reserve_exact(PIPE_CAPACITY)
            .map_err(|_| PipeError::NoResources)?;
        buffer.resize(PIPE_CAPACITY, 0);
        Ok(Self {
            id,
            state: SpinLock::new(PipeState {
                buffer,
                start: 0,
                len: 0,
                readers: 1,
                writers: 1,
            }),
        })
    }
}
impl Drop for Pipe {
    fn drop(&mut self) {
        notify_release(self.id);
    }
}

pub struct PipeEnd {
    pipe: Arc<Pipe>,
    direction: PipeDirection,
}

impl PipeEnd {
    #[allow(dead_code)]
    pub fn direction(&self) -> PipeDirection {
        self.direction
    }

    pub fn id(&self) -> usize {
        self.pipe.id
    }

    /// Reads up to `buffer.len()` bytes without blocking.
    pub fn try_read(&self, buffer: &mut [u8]) -> ReadOutcome {
        if self.direction != PipeDirection::Read {
            return ReadOutcome::WrongDirection;
        }
        let outcome = {
            let mut state = self.pipe.state.lock();
            state.read_from(buffer)
        };
        if let ReadOutcome::Data(count) = outcome {
            if count > 0 {
                notify_wake(self.pipe.id, PipeDirection::Write);
            }
        }
        outcome
    }

    /// Writes up to `buffer.len()` bytes without blocking.
    ///
    /// Short writes are possible and reported through `Written(count)`.
    pub fn try_write(&self, buffer: &[u8]) -> WriteOutcome {
        if self.direction != PipeDirection::Write {
            return WriteOutcome::WrongDirection;
        }
        let outcome = {
            let mut state = self.pipe.state.lock();
            state.write_from(buffer)
        };
        if let WriteOutcome::Written(count) = outcome {
            if count > 0 {
                notify_wake(self.pipe.id, PipeDirection::Read);
            }
        }
        outcome
    }

    /// Whether a read would make progress: data available or every writer gone.
    pub fn readable(&self) -> bool {
        if self.direction != PipeDirection::Read {
            return true;
        }
        let state = self.pipe.state.lock();
        state.len > 0 || state.writers == 0
    }

    /// Whether a write would make progress: free space or every reader gone.
    pub fn writable(&self) -> bool {
        if self.direction != PipeDirection::Write {
            return true;
        }
        let state = self.pipe.state.lock();
        state.len < state.buffer.len() || state.readers == 0
    }
}

impl Drop for PipeEnd {
    fn drop(&mut self) {
        let (readers_closed, writers_closed) = {
            let mut state = self.pipe.state.lock();
            match self.direction {
                PipeDirection::Read => state.readers = state.readers.saturating_sub(1),
                PipeDirection::Write => state.writers = state.writers.saturating_sub(1),
            }
            (state.readers == 0, state.writers == 0)
        };
        if writers_closed {
            notify_wake(self.pipe.id, PipeDirection::Read);
        }
        if readers_closed {
            notify_wake(self.pipe.id, PipeDirection::Write);
        }
    }
}

impl fmt::Debug for PipeEnd {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PipeEnd")
            .field("id", &self.pipe.id)
            .field("direction", &self.direction)
            .finish()
    }
}

/// Allocates a pipe with one read end and one write end.
///
/// The caller owns the pipe id (see `scheduler::allocate_pipe`); it is passed
/// to `notify_release` through `Pipe::drop`, including when an endpoint
/// allocation fails.
pub fn create(id: usize) -> Result<(Arc<PipeEnd>, Arc<PipeEnd>), PipeError> {
    let pipe = match Pipe::new(id) {
        Ok(pipe) => Arc::new(pipe),
        Err(error) => {
            notify_release(id);
            return Err(error);
        }
    };
    let read = Arc::new(PipeEnd {
        pipe: pipe.clone(),
        direction: PipeDirection::Read,
    });
    let write = Arc::new(PipeEnd {
        pipe,
        direction: PipeDirection::Write,
    });
    Ok((read, write))
}
