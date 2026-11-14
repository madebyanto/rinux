use hosttest::frame::{FrameAllocator, FrameError, FRAME_SIZE, REFCOUNT_MAX};
use std::collections::HashSet;

#[test]
fn frame_range_accounting_and_exhaustion() {
    let mut frames = Box::new(FrameAllocator::new());
    let base = 0x10_0000;
    frames.add_usable_range(base, base + 64 * FRAME_SIZE);
    assert_eq!(frames.free_frames(), 64);

    frames.reserve_range(base + 8 * FRAME_SIZE, base + 12 * FRAME_SIZE);
    assert_eq!(frames.free_frames(), 60);

    frames.add_usable_range(0x20_0100, 0x20_0100 + 2 * FRAME_SIZE);
    assert_eq!(frames.free_frames(), 61);

    let mut seen = HashSet::new();
    while let Some(address) = frames.allocate() {
        assert_eq!(address % FRAME_SIZE, 0);
        assert!(seen.insert(address), "frame handed out twice");
    }
    assert_eq!(seen.len(), 61);
    assert_eq!(frames.free_frames(), 0);
    assert!(!seen.contains(&(base + 8 * FRAME_SIZE)));
    assert!(seen.contains(&0x20_1000));

    assert_eq!(frames.release(base), Ok(true));
    assert_eq!(frames.release(base), Err(FrameError::NotAllocated));
    assert_eq!(frames.release(base + 1), Err(FrameError::Misaligned));
    assert_eq!(frames.release(1 << 30), Err(FrameError::OutOfRange));
    assert_eq!(frames.free_frames(), 1);
    assert_eq!(frames.allocate(), Some(base));
}

#[test]
fn frame_allocator_covers_full_managed_window() {
    let mut frames = Box::new(FrameAllocator::new());
    frames.add_usable_range(0, 2 << 30);
    assert_eq!(frames.free_frames(), (1 << 30) / FRAME_SIZE);
    let last = frames.release((1 << 30) - FRAME_SIZE);
    assert_eq!(last, Err(FrameError::NotAllocated));
}

#[test]
fn frame_refcounts_track_shared_owners() {
    let mut frames = Box::new(FrameAllocator::new());
    let base = 0x30_0000;
    frames.add_usable_range(base, base + 4 * FRAME_SIZE);

    let frame = frames.allocate().expect("frame allocation failed");
    assert_eq!(frames.refcount(frame), Ok(1));
    assert_eq!(frames.retain(frame), Ok(2));
    assert_eq!(frames.retain(frame), Ok(3));
    assert_eq!(frames.refcount(frame), Ok(3));
    assert_eq!(frames.free_frames(), 3);

    assert_eq!(frames.release(frame), Ok(false));
    assert_eq!(frames.release(frame), Ok(false));
    assert_eq!(frames.refcount(frame), Ok(1));
    assert_eq!(frames.free_frames(), 3);

    assert_eq!(frames.release(frame), Ok(true));
    assert_eq!(frames.free_frames(), 4);
    assert_eq!(frames.refcount(frame), Err(FrameError::NotAllocated));
    assert_eq!(frames.retain(frame), Err(FrameError::NotAllocated));
    assert_eq!(frames.release(frame), Err(FrameError::NotAllocated));
}

#[test]
fn reserved_frames_cannot_be_retained_or_released() {
    let mut frames = Box::new(FrameAllocator::new());
    let base = 0x40_0000;
    frames.add_usable_range(base, base + 2 * FRAME_SIZE);
    frames.reserve_range(base, base + FRAME_SIZE);

    assert_eq!(frames.free_frames(), 1);
    assert_eq!(frames.refcount(base), Err(FrameError::NotAllocated));
    assert_eq!(frames.retain(base), Err(FrameError::NotAllocated));
    assert_eq!(frames.release(base), Err(FrameError::NotAllocated));

    assert_eq!(frames.allocate(), Some(base + FRAME_SIZE));
    assert_eq!(frames.free_frames(), 0);
}

#[test]
fn refcount_overflow_is_rejected() {
    let mut frames = Box::new(FrameAllocator::new());
    let base = 0x50_0000;
    frames.add_usable_range(base, base + FRAME_SIZE);

    let frame = frames.allocate().expect("frame allocation failed");
    for expected in 2..=REFCOUNT_MAX {
        assert_eq!(frames.retain(frame), Ok(expected));
    }
    assert_eq!(frames.refcount(frame), Ok(REFCOUNT_MAX));
    assert_eq!(
        frames.retain(frame),
        Err(FrameError::RefcountOverflow)
    );
    assert_eq!(frames.refcount(frame), Ok(REFCOUNT_MAX));
    assert_eq!(frames.release(frame), Ok(false));
    assert_eq!(frames.refcount(frame), Ok(REFCOUNT_MAX - 1));
}
