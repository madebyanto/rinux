use hosttest::frame::{FrameAllocator, FrameError, FRAME_SIZE};
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

    assert_eq!(frames.deallocate(base), Ok(()));
    assert_eq!(frames.deallocate(base), Err(FrameError::NotAllocated));
    assert_eq!(frames.deallocate(base + 1), Err(FrameError::Misaligned));
    assert_eq!(frames.deallocate(1 << 30), Err(FrameError::OutOfRange));
    assert_eq!(frames.free_frames(), 1);
    assert_eq!(frames.allocate(), Some(base));
}

#[test]
fn frame_allocator_covers_full_managed_window() {
    let mut frames = Box::new(FrameAllocator::new());
    frames.add_usable_range(0, 2 << 30);
    assert_eq!(frames.free_frames(), (1 << 30) / FRAME_SIZE);
    let last = frames.deallocate((1 << 30) - FRAME_SIZE);
    assert_eq!(last, Err(FrameError::NotAllocated));
}
