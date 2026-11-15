use hosttest::fs::fd::{FdEntry, FdTable};
use hosttest::fs::pipe::{
    self, PipeDirection, ReadOutcome, WriteOutcome, PIPE_CAPACITY,
};

#[test]
fn dropping_a_descriptor_table_closes_its_pipe_ends() {
    let (read, write) = pipe::create(0).expect("pipe");
    let mut table = FdTable::empty();
    let write_fd = table.install(FdEntry::Pipe(write)).unwrap();
    assert_eq!(write_fd, 0);

    // A process exits by dropping its descriptor table while staying around as
    // a zombie; the last writer must be released at that point so that blocked
    // readers observe end-of-file without waiting for `waitpid`.
    drop(table);

    let mut buffer = [0u8; 4];
    assert!(read.readable());
    assert_eq!(read.try_read(&mut buffer), ReadOutcome::EndOfFile);
}

#[test]
fn creates_paired_endpoints() {
    let (read, write) = pipe::create(0).expect("pipe");
    assert_eq!(read.direction(), PipeDirection::Read);
    assert_eq!(write.direction(), PipeDirection::Write);
    assert_eq!(read.id(), write.id());
    assert!(!read.readable());
    assert!(write.writable());
}

#[test]
fn reports_empty_pipe_and_partial_reads() {
    let (read, write) = pipe::create(0).expect("pipe");
    let mut buffer = [0u8; 8];

    assert_eq!(read.try_read(&mut buffer), ReadOutcome::Empty);
    assert_eq!(write.try_write(b"abcdef"), WriteOutcome::Written(6));
    assert!(read.readable());

    assert_eq!(read.try_read(&mut buffer[..3]), ReadOutcome::Data(3));
    assert_eq!(&buffer[..3], b"abc");
    assert_eq!(read.try_read(&mut buffer), ReadOutcome::Data(3));
    assert_eq!(&buffer[..3], b"def");
    assert_eq!(read.try_read(&mut buffer), ReadOutcome::Empty);
}

#[test]
fn ring_buffer_wraps_and_preserves_order() {
    let (read, write) = pipe::create(0).expect("pipe");
    let first = [0x11u8; PIPE_CAPACITY];
    let second = [0x22u8; 4000];
    let mut buffer = [0u8; PIPE_CAPACITY];

    assert_eq!(
        write.try_write(&first),
        WriteOutcome::Written(PIPE_CAPACITY)
    );
    assert_eq!(write.try_write(b"x"), WriteOutcome::Full);
    assert!(!write.writable());

    // Leave a 96-byte tail, then write 4000 bytes so the ring wraps.
    assert_eq!(read.try_read(&mut buffer[..4000]), ReadOutcome::Data(4000));
    assert_eq!(write.try_write(&second), WriteOutcome::Written(4000));
    assert_eq!(read.try_read(&mut buffer), ReadOutcome::Data(PIPE_CAPACITY));

    assert!(buffer[..96].iter().all(|byte| *byte == 0x11));
    assert!(buffer[96..].iter().all(|byte| *byte == 0x22));
    assert!(write.writable());
}

#[test]
fn last_writer_close_produces_end_of_file() {
    let (read, write) = pipe::create(0).expect("pipe");
    let mut buffer = [0u8; 8];

    assert_eq!(write.try_write(b"hi"), WriteOutcome::Written(2));
    drop(write);
    assert!(read.readable());

    assert_eq!(read.try_read(&mut buffer), ReadOutcome::Data(2));
    assert_eq!(&buffer[..2], b"hi");
    assert_eq!(read.try_read(&mut buffer), ReadOutcome::EndOfFile);
}

#[test]
fn last_reader_close_produces_broken_pipe() {
    let (read, write) = pipe::create(0).expect("pipe");
    drop(read);

    assert!(write.writable());
    assert_eq!(write.try_write(b"data"), WriteOutcome::BrokenPipe);
}

#[test]
fn rejects_reads_from_the_write_end_and_vice_versa() {
    let (read, write) = pipe::create(0).expect("pipe");
    let mut buffer = [0u8; 4];

    assert_eq!(write.try_read(&mut buffer), ReadOutcome::WrongDirection);
    assert_eq!(read.try_write(b"nope"), WriteOutcome::WrongDirection);
}

#[test]
fn forked_descriptor_tables_share_pipe_endpoints() {
    let (read, write) = pipe::create(0).expect("pipe");
    let mut parent = FdTable::new();
    let read_fd = parent.install(FdEntry::Pipe(read.clone())).unwrap();
    let write_fd = parent.install(FdEntry::Pipe(write.clone())).unwrap();

    // `fork` clones the table; both copies refer to the same endpoints.
    let mut child = parent.clone();
    child.close(write_fd).unwrap();

    let parent_write = match parent.entry(write_fd).unwrap() {
        FdEntry::Pipe(end) => end,
        _ => panic!("expected pipe write end"),
    };
    assert_eq!(parent_write.try_write(b"x"), WriteOutcome::Written(1));

    // The child still holds its own descriptor for the read end.
    let child_read = match child.entry(read_fd).unwrap() {
        FdEntry::Pipe(end) => end,
        _ => panic!("expected pipe read end"),
    };
    let mut buffer = [0u8; 1];
    assert_eq!(child_read.try_read(&mut buffer), ReadOutcome::Data(1));
    assert_eq!(buffer[0], b'x');

    // Closing the parent's read descriptor only affects that table entry.
    parent.close(read_fd).unwrap();
    assert!(parent.entry(read_fd).is_err());
}
