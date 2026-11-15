use core::mem::size_of;

pub const SYS_READ: u64 = 0;
pub const SYS_WRITE: u64 = 1;
pub const SYS_OPEN: u64 = 2;
pub const SYS_CLOSE: u64 = 3;
pub const SYS_STAT: u64 = 4;
pub const SYS_LSEEK: u64 = 8;
pub const SYS_RT_SIGACTION: u64 = 13;
pub const SYS_RT_SIGPROCMASK: u64 = 14;
pub const SYS_PIPE: u64 = 22;
pub const SYS_FORK: u64 = 57;
pub const SYS_EXECVE: u64 = 59;
pub const SYS_GETPID: u64 = 39;
pub const SYS_EXIT: u64 = 60;
pub const SYS_WAITPID: u64 = 61;
pub const SYS_KILL: u64 = 62;

pub const EPERM: i64 = 1;
pub const ENOENT: i64 = 2;
pub const E2BIG: i64 = 7;
pub const ENOEXEC: i64 = 8;
pub const ECHILD: i64 = 10;
pub const EAGAIN: i64 = 11;
pub const ENOMEM: i64 = 12;
pub const ESRCH: i64 = 3;
#[allow(dead_code)] // returned by interruptible syscalls once delivery is wired up
pub const EINTR: i64 = 4;
pub const EIO: i64 = 5;
pub const EBADF: i64 = 9;
pub const EACCES: i64 = 13;
pub const EFAULT: i64 = 14;
pub const ENOTDIR: i64 = 20;
pub const EISDIR: i64 = 21;
pub const EINVAL: i64 = 22;
pub const ENFILE: i64 = 23;
pub const EMFILE: i64 = 24;
pub const EPIPE: i64 = 32;
pub const EROFS: i64 = 30;
pub const ENAMETOOLONG: i64 = 36;
pub const ENOSYS: i64 = 38;

/// `waitpid` option: return immediately when no child has exited.
pub const WNOHANG: u64 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Syscall {
    Read,
    Write,
    Open,
    Close,
    Stat,
    Lseek,
    RtSigaction,
    RtSigprocmask,
    Pipe,
    GetPid,
    Fork,
    Execve,
    WaitPid,
    Kill,
    Exit,
}

pub fn classify(number: u64) -> Option<Syscall> {
    match number {
        SYS_READ => Some(Syscall::Read),
        SYS_WRITE => Some(Syscall::Write),
        SYS_OPEN => Some(Syscall::Open),
        SYS_CLOSE => Some(Syscall::Close),
        SYS_STAT => Some(Syscall::Stat),
        SYS_LSEEK => Some(Syscall::Lseek),
        SYS_RT_SIGACTION => Some(Syscall::RtSigaction),
        SYS_RT_SIGPROCMASK => Some(Syscall::RtSigprocmask),
        SYS_PIPE => Some(Syscall::Pipe),
        SYS_GETPID => Some(Syscall::GetPid),
        SYS_FORK => Some(Syscall::Fork),
        SYS_EXECVE => Some(Syscall::Execve),
        SYS_WAITPID => Some(Syscall::WaitPid),
        SYS_KILL => Some(Syscall::Kill),
        SYS_EXIT => Some(Syscall::Exit),
        _ => None,
    }
}

/// Linux x86-64 `struct sigaction` as copied by `rt_sigaction`.
///
/// The field order is part of the kernel ABI: handler, flags, restorer, then
/// the 64-bit signal mask. Rinux accepts `sigsetsize == 8`.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KernelSigAction {
    pub handler: u64,
    pub flags: u64,
    pub restorer: u64,
    pub mask: u64,
}

/// Width of the `sigset_t` exchanged by `rt_sigaction`/`rt_sigprocmask`.
pub const SIGSET_SIZE: usize = 8;

const _: () = assert!(size_of::<KernelSigAction>() == 32);

pub fn errno(code: i64) -> u64 {
    (-code) as u64
}
