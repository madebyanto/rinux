use core::mem::size_of;

use hosttest::syscall_abi::{
    classify, errno, validate_open_flags, KernelSigAction, OpenAccess, Syscall, E2BIG, EBADF,
    ECHILD, EEXIST, EFAULT, EINTR, EINVAL, ENOMEM, ENOSYS, ENOENT, EOPNOTSUPP, EPERM, EROFS,
    ESRCH, O_ACCMODE, O_APPEND, O_CLOEXEC, O_CREAT, O_DSYNC, O_EXCL, O_NONBLOCK, O_RDONLY,
    O_RDWR, O_SYNC, O_TRUNC, O_WRONLY, SIGSET_SIZE, SYS_CLOSE, SYS_EXECVE, SYS_EXIT, SYS_FORK,
    SYS_FTRUNCATE, SYS_GETPID, SYS_KILL, SYS_LSEEK, SYS_OPEN, SYS_READ, SYS_RT_SIGACTION,
    SYS_RT_SIGPROCMASK, SYS_RT_SIGRETURN, SYS_STAT, SYS_SYNC, SYS_TRUNCATE, SYS_WAITPID,
    SYS_WRITE,
};

#[test]
fn classifies_linux_syscall_numbers() {
    assert_eq!(classify(SYS_READ), Some(Syscall::Read));
    assert_eq!(classify(SYS_WRITE), Some(Syscall::Write));
    assert_eq!(classify(SYS_OPEN), Some(Syscall::Open));
    assert_eq!(classify(SYS_CLOSE), Some(Syscall::Close));
    assert_eq!(classify(SYS_STAT), Some(Syscall::Stat));
    assert_eq!(classify(SYS_LSEEK), Some(Syscall::Lseek));
    assert_eq!(classify(SYS_RT_SIGACTION), Some(Syscall::RtSigaction));
    assert_eq!(classify(SYS_RT_SIGPROCMASK), Some(Syscall::RtSigprocmask));
    assert_eq!(classify(SYS_RT_SIGRETURN), Some(Syscall::RtSigreturn));
    assert_eq!(classify(SYS_FORK), Some(Syscall::Fork));
    assert_eq!(classify(SYS_EXECVE), Some(Syscall::Execve));
    assert_eq!(classify(SYS_WAITPID), Some(Syscall::WaitPid));
    assert_eq!(classify(SYS_KILL), Some(Syscall::Kill));
    assert_eq!(classify(SYS_GETPID), Some(Syscall::GetPid));
    assert_eq!(classify(SYS_TRUNCATE), Some(Syscall::Truncate));
    assert_eq!(classify(SYS_FTRUNCATE), Some(Syscall::Ftruncate));
    assert_eq!(classify(SYS_SYNC), Some(Syscall::Sync));
    assert_eq!(classify(SYS_EXIT), Some(Syscall::Exit));
}

#[test]
fn validates_open_flags_and_access_modes() {
    assert_eq!(validate_open_flags(O_RDONLY), Some(OpenAccess::ReadOnly));
    assert_eq!(validate_open_flags(O_WRONLY), Some(OpenAccess::WriteOnly));
    assert_eq!(validate_open_flags(O_RDWR), Some(OpenAccess::ReadWrite));
    assert_eq!(validate_open_flags(O_ACCMODE), None);
    assert_eq!(validate_open_flags(0o200000), None);

    let flags = O_RDWR | O_CREAT | O_EXCL | O_TRUNC | O_APPEND | O_CLOEXEC | O_NONBLOCK;
    assert_eq!(validate_open_flags(flags), Some(OpenAccess::ReadWrite));
    assert_eq!(
        validate_open_flags(O_WRONLY | O_APPEND),
        Some(OpenAccess::WriteOnly)
    );
    assert_eq!(
        validate_open_flags(O_RDWR | O_SYNC | O_DSYNC),
        Some(OpenAccess::ReadWrite)
    );
    assert!(OpenAccess::ReadOnly.readable());
    assert!(!OpenAccess::ReadOnly.writable());
    assert!(OpenAccess::WriteOnly.writable());
    assert!(OpenAccess::ReadWrite.readable() && OpenAccess::ReadWrite.writable());
}

#[test]
fn truncate_and_sync_use_linux_numbers() {
    assert_eq!(SYS_TRUNCATE, 76);
    assert_eq!(SYS_FTRUNCATE, 77);
    assert_eq!(SYS_SYNC, 162);
    assert_eq!(O_TRUNC, 0o1000);
    assert_eq!(O_APPEND, 0o2000);
    assert_eq!(EEXIST, 17);
    assert_eq!(EOPNOTSUPP, 95);
}

#[test]
fn rejects_unknown_syscall_numbers() {
    assert_eq!(classify(5), None);
    assert_eq!(classify(58), None);
    assert_eq!(classify(100), None);
    assert_eq!(classify(u64::MAX), None);
}

#[test]
fn kernel_sigaction_matches_the_linux_abi() {
    assert_eq!(size_of::<KernelSigAction>(), 32);
    assert_eq!(SIGSET_SIZE, 8);
    assert_eq!(SYS_RT_SIGACTION, 13);
    assert_eq!(SYS_RT_SIGPROCMASK, 14);
    assert_eq!(SYS_RT_SIGRETURN, 15);
    assert_eq!(SYS_KILL, 62);
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
    assert_eq!(errno(EPERM), (-(EPERM as i64)) as u64);
    assert_eq!(errno(EINTR), (-(EINTR as i64)) as u64);
    assert_eq!(errno(EBADF) & (1 << 63), 1 << 63);
}
