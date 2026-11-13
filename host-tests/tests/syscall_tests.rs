use hosttest::syscall_abi::{
    classify, errno, Syscall, EBADF, EFAULT, EINVAL, ENOSYS, ESRCH, SYS_EXIT, SYS_GETPID,
    SYS_READ, SYS_WRITE,
};

#[test]
fn classifies_linux_syscall_numbers() {
    assert_eq!(classify(SYS_READ), Some(Syscall::Read));
    assert_eq!(classify(SYS_WRITE), Some(Syscall::Write));
    assert_eq!(classify(SYS_GETPID), Some(Syscall::GetPid));
    assert_eq!(classify(SYS_EXIT), Some(Syscall::Exit));
}

#[test]
fn rejects_unknown_syscall_numbers() {
    assert_eq!(classify(2), None);
    assert_eq!(classify(59), None);
    assert_eq!(classify(61), None);
    assert_eq!(classify(u64::MAX), None);
}

#[test]
fn errno_encodes_negative_values() {
    assert_eq!(errno(EBADF), (-(EBADF as i64)) as u64);
    assert_eq!(errno(EFAULT), (-(EFAULT as i64)) as u64);
    assert_eq!(errno(EINVAL), (-(EINVAL as i64)) as u64);
    assert_eq!(errno(ENOSYS), (-(ENOSYS as i64)) as u64);
    assert_eq!(errno(ESRCH), (-(ESRCH as i64)) as u64);
    assert_eq!(errno(EBADF) & (1 << 63), 1 << 63);
}
