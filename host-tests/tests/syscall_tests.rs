use hosttest::syscall_abi::{
    classify, errno, Syscall, E2BIG, EBADF, ECHILD, EFAULT, EINVAL, ENOMEM, ENOSYS, ENOENT,
    EROFS, ESRCH, SYS_CLOSE, SYS_EXECVE, SYS_EXIT, SYS_FORK, SYS_GETPID, SYS_LSEEK, SYS_OPEN,
    SYS_READ, SYS_STAT, SYS_WAITPID, SYS_WRITE,
};

#[test]
fn classifies_linux_syscall_numbers() {
    assert_eq!(classify(SYS_READ), Some(Syscall::Read));
    assert_eq!(classify(SYS_WRITE), Some(Syscall::Write));
    assert_eq!(classify(SYS_OPEN), Some(Syscall::Open));
    assert_eq!(classify(SYS_CLOSE), Some(Syscall::Close));
    assert_eq!(classify(SYS_STAT), Some(Syscall::Stat));
    assert_eq!(classify(SYS_LSEEK), Some(Syscall::Lseek));
    assert_eq!(classify(SYS_FORK), Some(Syscall::Fork));
    assert_eq!(classify(SYS_EXECVE), Some(Syscall::Execve));
    assert_eq!(classify(SYS_WAITPID), Some(Syscall::WaitPid));
    assert_eq!(classify(SYS_GETPID), Some(Syscall::GetPid));
    assert_eq!(classify(SYS_EXIT), Some(Syscall::Exit));
}

#[test]
fn rejects_unknown_syscall_numbers() {
    assert_eq!(classify(5), None);
    assert_eq!(classify(58), None);
    assert_eq!(classify(62), None);
    assert_eq!(classify(u64::MAX), None);
}

#[test]
fn errno_encodes_negative_values() {
    assert_eq!(errno(EBADF), (-(EBADF as i64)) as u64);
    assert_eq!(errno(EFAULT), (-(EFAULT as i64)) as u64);
    assert_eq!(errno(EINVAL), (-(EINVAL as i64)) as u64);
    assert_eq!(errno(ENOSYS), (-(ENOSYS as i64)) as u64);
    assert_eq!(errno(ESRCH), (-(ESRCH as i64)) as u64);
    assert_eq!(errno(ENOENT), (-(ENOENT as i64)) as u64);
    assert_eq!(errno(EROFS), (-(EROFS as i64)) as u64);
    assert_eq!(errno(E2BIG), (-(E2BIG as i64)) as u64);
    assert_eq!(errno(ECHILD), (-(ECHILD as i64)) as u64);
    assert_eq!(errno(ENOMEM), (-(ENOMEM as i64)) as u64);
    assert_eq!(errno(EBADF) & (1 << 63), 1 << 63);
}
