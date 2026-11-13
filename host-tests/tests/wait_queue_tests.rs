use hosttest::queue::MAX_PROCESSES;
use hosttest::wait_queue::{PidQueue, QueueError};

#[test]
fn queue_is_fifo_and_rejects_duplicates() {
    let mut queue = PidQueue::new();
    queue.push(3).unwrap();
    queue.push(4).unwrap();
    queue.push(5).unwrap();
    assert_eq!(queue.push(4), Err(QueueError::AlreadyQueued));
    assert_eq!(queue.pop(), Some(3));
    assert_eq!(queue.pop(), Some(4));
    assert_eq!(queue.pop(), Some(5));
    assert_eq!(queue.pop(), None);
}

#[test]
fn queue_removes_waiters_and_reports_full() {
    let mut queue = PidQueue::new();
    for pid in 1..=MAX_PROCESSES as u32 {
        queue.push(pid).unwrap();
    }
    assert_eq!(
        queue.push(MAX_PROCESSES as u32 + 1),
        Err(QueueError::Full)
    );
    assert!(queue.remove(7));
    assert!(!queue.remove(7));
    assert_eq!(queue.len(), MAX_PROCESSES - 1);
    assert_eq!(queue.pop(), Some(1));
    assert_eq!(queue.len(), MAX_PROCESSES - 2);
}
