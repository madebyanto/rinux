pub const SYS_READ: u64 = 0;
pub const SYS_WRITE: u64 = 1;
pub const SYS_GETPID: u64 = 39;
pub const SYS_EXIT: u64 = 60;

pub const ESRCH: i64 = 3;
pub const EIO: i64 = 5;
pub const EBADF: i64 = 9;
pub const EFAULT: i64 = 14;
pub const EINVAL: i64 = 22;
pub const ENOSYS: i64 = 38;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Syscall {
    Read,
    Write,
    GetPid,
    Exit,
}

pub fn classify(number: u64) -> Option<Syscall> {
    match number {
        SYS_READ => Some(Syscall::Read),
        SYS_WRITE => Some(Syscall::Write),
        SYS_GETPID => Some(Syscall::GetPid),
        SYS_EXIT => Some(Syscall::Exit),
        _ => None,
    }
}

pub fn errno(code: i64) -> u64 {
    (-code) as u64
}
