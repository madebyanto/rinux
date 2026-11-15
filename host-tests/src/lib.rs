extern crate alloc;

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, Once};

pub mod arch {
    pub mod x86_64 {
        pub mod cpu {
            use core::sync::atomic::{AtomicUsize, Ordering};

            static ACTIVE_ROOT: AtomicUsize = AtomicUsize::new(0);

            pub fn read_cr3() -> u64 {
                ACTIVE_ROOT.load(Ordering::SeqCst) as u64
            }

            pub fn write_cr3(value: usize) {
                ACTIVE_ROOT.store(value, Ordering::SeqCst);
            }

            pub fn invlpg(_address: usize) {}
        }
    }
}

#[path = "../../src/mm/frame.rs"]
pub mod frame;
#[path = "../../src/mm/heap.rs"]
pub mod heap;
#[path = "../../src/mm/pte.rs"]
pub mod pte;
#[path = "../../src/mm/fault.rs"]
pub mod fault;
#[path = "../../src/mm/paging.rs"]
pub mod paging;
#[path = "../../src/mm/address_space.rs"]
pub mod address_space;
#[path = "../../src/drivers/ps2.rs"]
pub mod ps2;
#[path = "../../src/drivers/ata.rs"]
pub mod ata;
#[path = "../../src/proc/pid.rs"]
pub mod pid;
#[path = "../../src/proc/queue.rs"]
pub mod queue;
#[path = "../../src/proc/reap.rs"]
pub mod reap;
#[path = "../../src/mm/uaccess.rs"]
pub mod uaccess;
#[path = "../../src/syscall/abi.rs"]
pub mod syscall_abi;
#[path = "../../src/arch/x86_64/descriptor.rs"]
pub mod descriptor;
#[path = "../../src/sync/mod.rs"]
pub mod sync;
#[path = "../../src/init/cmdline.rs"]
pub mod cmdline;

pub mod fs;
#[path = "../../src/proc/wait.rs"]
pub mod wait;
#[path = "../../src/proc/wait_queue.rs"]
pub mod wait_queue;

const HOST_FRAME_REGION: usize = 1 << 20;
const HOST_FRAME_SIZE: usize = 4096;

struct HostFrames {
    next: usize,
    end: usize,
    free: usize,
    refs: std::collections::HashMap<usize, u16>,
}

static HOST_FRAMES: Mutex<Option<HostFrames>> = Mutex::new(None);

/// Serializes tests that build and switch page tables.
///
/// The host `cpu::write_cr3` shim keeps a single process-wide "active root", so
/// two tests mapping user pages concurrently would otherwise activate each
/// other's tables.
static PAGE_TABLE_TEST_LOCK: Mutex<()> = Mutex::new(());

pub fn lock_page_table_tests() -> std::sync::MutexGuard<'static, ()> {
    PAGE_TABLE_TEST_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}
static HOST_FRAME_BASE: AtomicUsize = AtomicUsize::new(0);
static HOST_FRAMES_INIT: Once = Once::new();

/// Allocates a page-aligned region backing the host frame allocator and points
/// the fake CR3 at its first (reserved) page.
pub fn init_frames() -> usize {
    HOST_FRAMES_INIT.call_once(|| {
        let layout = std::alloc::Layout::from_size_align(HOST_FRAME_REGION, HOST_FRAME_SIZE)
            .expect("invalid frame region layout");
        let base = unsafe { std::alloc::alloc_zeroed(layout) } as usize;
        assert!(base != 0, "host frame region allocation failed");
        let frames = HostFrames {
            next: base + HOST_FRAME_SIZE,
            end: base + HOST_FRAME_REGION,
            free: HOST_FRAME_REGION / HOST_FRAME_SIZE - 1,
            refs: std::collections::HashMap::new(),
        };
        *HOST_FRAMES.lock().expect("frame lock poisoned") = Some(frames);
        HOST_FRAME_BASE.store(base, Ordering::SeqCst);
        arch::x86_64::cpu::write_cr3(base);
    });
    HOST_FRAME_BASE.load(Ordering::SeqCst)
}

fn with_frames<R>(function: impl FnOnce(&mut HostFrames) -> R) -> R {
    let mut guard = HOST_FRAMES.lock().expect("frame lock poisoned");
    let frames = guard.as_mut().expect("host frames not initialized");
    function(frames)
}

pub fn allocate_frame() -> Option<usize> {
    with_frames(|frames| {
        if frames.next == frames.end {
            return None;
        }
        let address = frames.next;
        frames.next += HOST_FRAME_SIZE;
        frames.free -= 1;
        frames.refs.insert(address, 1);
        Some(address)
    })
}

pub fn retain_frame(address: usize) -> Result<u16, frame::FrameError> {
    if address % HOST_FRAME_SIZE != 0 {
        return Err(frame::FrameError::Misaligned);
    }
    with_frames(|frames| {
        let refs = frames
            .refs
            .get_mut(&address)
            .ok_or(frame::FrameError::NotAllocated)?;
        if *refs == u16::MAX {
            return Err(frame::FrameError::RefcountOverflow);
        }
        *refs += 1;
        Ok(*refs)
    })
}

pub fn release_frame(address: usize) -> Result<bool, frame::FrameError> {
    if address % HOST_FRAME_SIZE != 0 {
        return Err(frame::FrameError::Misaligned);
    }
    with_frames(|frames| {
        let refs = frames
            .refs
            .get_mut(&address)
            .ok_or(frame::FrameError::NotAllocated)?;
        *refs -= 1;
        if *refs != 0 {
            return Ok(false);
        }
        frames.refs.remove(&address);
        frames.free += 1;
        Ok(true)
    })
}

pub fn frame_refcount(address: usize) -> Result<u16, frame::FrameError> {
    if address % HOST_FRAME_SIZE != 0 {
        return Err(frame::FrameError::Misaligned);
    }
    with_frames(|frames| {
        frames
            .refs
            .get(&address)
            .copied()
            .ok_or(frame::FrameError::NotAllocated)
    })
}

pub fn free_frame_count() -> usize {
    with_frames(|frames| frames.free)
}
