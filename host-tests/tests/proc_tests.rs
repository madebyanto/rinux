use hosttest::pid::{PidAllocator, PidError};
use hosttest::queue::{QueueError, ReadyQueue, MAX_PROCESSES};

#[test]
fn pid_allocation_is_unique_and_starts_at_one() {
    let mut pids = PidAllocator::new();
    let mut seen = Vec::new();
    for expected in 1..=64u32 {
        let pid = pids.allocate().unwrap();
        assert_eq!(pid, expected);
        assert!(!seen.contains(&pid));
        seen.push(pid);
    }
}

#[test]
fn pid_allocator_reports_exhaustion() {
    let mut pids = PidAllocator::starting_at(u32::MAX);
    assert_eq!(pids.allocate(), Ok(u32::MAX));
    assert_eq!(pids.allocate(), Err(PidError::Exhausted));
    assert_eq!(pids.allocate(), Err(PidError::Exhausted));
}

#[test]
fn ready_queue_is_fifo() {
    let mut queue = ReadyQueue::new();
    assert!(queue.is_empty());
    assert_eq!(queue.pop(), None);
    for pid in [7, 3, 9] {
        queue.push(pid).unwrap();
    }
    assert_eq!(queue.len(), 3);
    assert_eq!(queue.pop(), Some(7));
    assert_eq!(queue.pop(), Some(3));
    assert_eq!(queue.pop(), Some(9));
    assert_eq!(queue.pop(), None);
    assert!(queue.is_empty());
}

#[test]
fn ready_queue_rejects_duplicates_and_overflow() {
    let mut queue = ReadyQueue::new();
    queue.push(1).unwrap();
    assert_eq!(queue.push(1), Err(QueueError::AlreadyQueued));
    for pid in 2..=MAX_PROCESSES as u32 {
        queue.push(pid).unwrap();
    }
    assert_eq!(queue.len(), MAX_PROCESSES);
    assert_eq!(queue.push(MAX_PROCESSES as u32 + 1), Err(QueueError::Full));
    assert_eq!(queue.push(2), Err(QueueError::AlreadyQueued));
}

#[test]
fn ready_queue_wraps_and_preserves_order() {
    let mut queue = ReadyQueue::new();
    for pid in 0..MAX_PROCESSES as u32 {
        queue.push(pid).unwrap();
    }
    for pid in 0..MAX_PROCESSES as u32 / 2 {
        assert_eq!(queue.pop(), Some(pid));
    }
    for pid in 0..MAX_PROCESSES as u32 / 2 {
        queue.push(100 + pid).unwrap();
    }
    for pid in MAX_PROCESSES as u32 / 2..MAX_PROCESSES as u32 {
        assert_eq!(queue.pop(), Some(pid));
    }
    for pid in 0..MAX_PROCESSES as u32 / 2 {
        assert_eq!(queue.pop(), Some(100 + pid));
    }
    assert!(queue.is_empty());
}

#[test]
fn ready_queue_removes_any_position() {
    let mut queue = ReadyQueue::new();
    for pid in 0..5 {
        queue.push(pid).unwrap();
    }
    assert!(queue.remove(0));
    assert!(queue.remove(4));
    assert!(queue.remove(2));
    assert!(!queue.remove(4));
    assert_eq!(queue.len(), 2);
    assert_eq!(queue.pop(), Some(1));
    assert_eq!(queue.pop(), Some(3));
    assert_eq!(queue.pop(), None);
}

#[test]
fn ready_queue_remove_after_wrap() {
    let mut queue = ReadyQueue::new();
    for pid in 0..MAX_PROCESSES as u32 {
        queue.push(pid).unwrap();
    }
    for _ in 0..MAX_PROCESSES / 2 {
        queue.pop();
    }
    for pid in 0..MAX_PROCESSES as u32 / 2 {
        queue.push(100 + pid).unwrap();
    }
    assert!(queue.remove(104));
    let mut expected: Vec<u32> = (MAX_PROCESSES as u32 / 2..MAX_PROCESSES as u32).collect();
    expected.extend((100..100 + MAX_PROCESSES as u32 / 2).filter(|pid| *pid != 104));
    for pid in expected {
        assert_eq!(queue.pop(), Some(pid));
    }
    assert_eq!(queue.pop(), None);
}
