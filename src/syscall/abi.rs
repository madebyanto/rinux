use core::mem::size_of;

pub const SYS_READ: u64 = 0;
pub const SYS_WRITE: u64 = 1;
pub const SYS_OPEN: u64 = 2;
pub const SYS_CLOSE: u64 = 3;
pub const SYS_STAT: u64 = 4;
pub const SYS_LSEEK: u64 = 8;
pub const SYS_RT_SIGACTION: u64 = 13;
pub const SYS_RT_SIGPROCMASK: u64 = 14;
pub const SYS_RT_SIGRETURN: u64 = 15;
pub const SYS_PIPE: u64 = 22;
pub const SYS_FORK: u64 = 57;
pub const SYS_EXECVE: u64 = 59;
pub const SYS_GETPID: u64 = 39;
pub const SYS_EXIT: u64 = 60;
pub const SYS_WAITPID: u64 = 61;
pub const SYS_KILL: u64 = 62;
pub const SYS_TRUNCATE: u64 = 76;
pub const SYS_FTRUNCATE: u64 = 77;
pub const SYS_RENAME: u64 = 82;
pub const SYS_MKDIR: u64 = 83;
pub const SYS_RMDIR: u64 = 84;
pub const SYS_UNLINK: u64 = 87;
pub const SYS_SYNC: u64 = 162;

pub const EPERM: i64 = 1;
pub const ENOENT: i64 = 2;
pub const E2BIG: i64 = 7;
pub const ENOEXEC: i64 = 8;
pub const ECHILD: i64 = 10;
pub const EAGAIN: i64 = 11;
pub const ENOMEM: i64 = 12;
pub const ESRCH: i64 = 3;
pub const EINTR: i64 = 4;
pub const EIO: i64 = 5;
pub const EBADF: i64 = 9;
pub const EACCES: i64 = 13;
pub const EFAULT: i64 = 14;
pub const EBUSY: i64 = 16;
pub const EEXIST: i64 = 17;
pub const ENOTDIR: i64 = 20;
pub const EISDIR: i64 = 21;
pub const EINVAL: i64 = 22;
pub const ENFILE: i64 = 23;
pub const EMFILE: i64 = 24;
pub const EFBIG: i64 = 27;
pub const ENOSPC: i64 = 28;
pub const ENOTEMPTY: i64 = 39;
pub const EPIPE: i64 = 32;
pub const EROFS: i64 = 30;
pub const ENAMETOOLONG: i64 = 36;
pub const ENOSYS: i64 = 38;
pub const EOPNOTSUPP: i64 = 95;

/// `open` flags live in the VFS layer so filesystem code can use them too.
#[allow(unused_imports)]
pub use crate::fs::vfs::{
    O_ACCMODE, O_APPEND, O_CLOEXEC, O_CREAT, O_DSYNC, O_EXCL, O_NONBLOCK, O_RDONLY, O_RDWR,
    O_SUPPORTED, O_SYNC, O_TRUNC, O_WRONLY,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpenAccess {
    ReadOnly,
    WriteOnly,
    ReadWrite,
}

impl OpenAccess {
    #[allow(dead_code)]
    pub const fn readable(self) -> bool {
        !matches!(self, OpenAccess::WriteOnly)
    }

    pub const fn writable(self) -> bool {
        matches!(self, OpenAccess::WriteOnly | OpenAccess::ReadWrite)
    }
}

/// Validate the low 32 bits of an `open` flags argument.
pub fn validate_open_flags(flags: u32) -> Option<OpenAccess> {
    if flags & !O_SUPPORTED != 0 {
        return None;
    }
    match flags & O_ACCMODE {
        O_RDONLY => Some(OpenAccess::ReadOnly),
        O_WRONLY => Some(OpenAccess::WriteOnly),
        O_RDWR => Some(OpenAccess::ReadWrite),
        _ => None,
    }
}

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
    RtSigreturn,
    Pipe,
    GetPid,
    Fork,
    Execve,
    WaitPid,
    Kill,
    Truncate,
    Ftruncate,
    Rename,
    Mkdir,
    Rmdir,
    Unlink,
    Sync,
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
        SYS_RT_SIGRETURN => Some(Syscall::RtSigreturn),
        SYS_PIPE => Some(Syscall::Pipe),
        SYS_GETPID => Some(Syscall::GetPid),
        SYS_FORK => Some(Syscall::Fork),
        SYS_EXECVE => Some(Syscall::Execve),
        SYS_WAITPID => Some(Syscall::WaitPid),
        SYS_KILL => Some(Syscall::Kill),
        SYS_TRUNCATE => Some(Syscall::Truncate),
        SYS_FTRUNCATE => Some(Syscall::Ftruncate),
        SYS_RENAME => Some(Syscall::Rename),
        SYS_MKDIR => Some(Syscall::Mkdir),
        SYS_RMDIR => Some(Syscall::Rmdir),
        SYS_UNLINK => Some(Syscall::Unlink),
        SYS_SYNC => Some(Syscall::Sync),
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
