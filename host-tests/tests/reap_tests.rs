use hosttest::queue::MAX_PROCESSES;
use hosttest::reap::{ReapError, ReapList};

#[test]
fn reap_list_takes_fifo_when_nothing_is_current() {
    let mut list = ReapList::new();
    for pid in 1..=3 {
        list.push(pid).unwrap();
    }
    assert_eq!(list.len(), 3);
    assert_eq!(list.take_reapable(None), Some(1));
    assert_eq!(list.take_reapable(None), Some(2));
    assert_eq!(list.take_reapable(None), Some(3));
    assert_eq!(list.take_reapable(None), None);
    assert!(list.is_empty());
}

#[test]
fn reap_list_skips_the_current_task() {
    let mut list = ReapList::new();
    list.push(1).unwrap();
    list.push(2).unwrap();
    list.push(3).unwrap();
    assert_eq!(list.take_reapable(Some(1)), Some(2));
    assert_eq!(list.take_reapable(Some(1)), Some(3));
    assert_eq!(list.take_reapable(Some(1)), None);
    assert_eq!(list.len(), 1);
    assert_eq!(list.take_reapable(None), Some(1));
    assert!(list.is_empty());
}

#[test]
fn reap_list_rejects_duplicates_and_overflow() {
    let mut list = ReapList::new();
    list.push(1).unwrap();
    assert_eq!(list.push(1), Err(ReapError::AlreadyPresent));
    for pid in 2..=MAX_PROCESSES as u32 {
        list.push(pid).unwrap();
    }
    assert_eq!(list.len(), MAX_PROCESSES);
    assert_eq!(
        list.push(MAX_PROCESSES as u32 + 1),
        Err(ReapError::Full)
    );
}

#[test]
fn reap_list_can_reuse_space_after_removal() {
    let mut list = ReapList::new();
    list.push(7).unwrap();
    list.push(8).unwrap();
    assert_eq!(list.take_reapable(None), Some(7));
    list.push(9).unwrap();
    assert!(list.contains(8));
    assert!(list.contains(9));
    assert!(!list.contains(7));
    assert_eq!(list.take_reapable(None), Some(8));
    assert_eq!(list.take_reapable(None), Some(9));
    assert_eq!(list.take_reapable(None), None);
}
