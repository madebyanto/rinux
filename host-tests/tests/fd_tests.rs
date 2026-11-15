use hosttest::fs::fd::{FdEntry, FdTable, MAX_FDS};
use hosttest::fs::vfs::File;

#[test]
fn initializes_standard_file_descriptors() {
    let table = FdTable::new();
    assert_eq!(table.entry(0).unwrap(), FdEntry::Keyboard);
    assert_eq!(table.entry(1).unwrap(), FdEntry::Console);
    assert_eq!(table.entry(2).unwrap(), FdEntry::Console);
    assert_eq!(table.entry(3), Err(hosttest::fs::fd::FdError::InvalidFd));
}

#[test]
fn allocates_lowest_free_file_descriptor_and_reuses_closed() {
    let mut table = FdTable::new();
    assert_eq!(table.open_file(2), Ok(3));
    assert_eq!(table.open_file(5), Ok(4));
    assert_eq!(
        table.entry(3),
        Ok(FdEntry::File(File { node: 2, offset: 0 }))
    );
    table.close(3).unwrap();
    assert_eq!(table.open_file(9), Ok(3));
    assert_eq!(table.entry(4), Ok(FdEntry::File(File { node: 5, offset: 0 })));
}

#[test]
fn updates_offsets_and_rejects_invalid_descriptors() {
    let mut table = FdTable::new();
    assert_eq!(table.set_offset(0, 10), Err(hosttest::fs::fd::FdError::InvalidFd));
    assert_eq!(table.close(0), Err(hosttest::fs::fd::FdError::InvalidFd));
    assert_eq!(table.close(MAX_FDS as u32), Err(hosttest::fs::fd::FdError::InvalidFd));
    let fd = table.open_file(7).unwrap();
    table.set_offset(fd, 42).unwrap();
    assert_eq!(
        table.entry(fd),
        Ok(FdEntry::File(File { node: 7, offset: 42 }))
    );
    assert!(table.close(fd).is_ok());
    assert_eq!(table.entry(fd), Err(hosttest::fs::fd::FdError::InvalidFd));
}

#[test]
fn reports_no_free_descriptor() {
    let mut table = FdTable::new();
    for _ in 3..MAX_FDS {
        table.open_file(2).unwrap();
    }
    assert_eq!(
        table.open_file(2),
        Err(hosttest::fs::fd::FdError::NoFreeFd)
    );
    table.close(3).unwrap();
    assert_eq!(table.open_file(2), Ok(3));
}
