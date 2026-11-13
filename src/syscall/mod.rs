pub mod abi;

use crate::arch::x86_64::syscall::SyscallFrame;
use crate::console;
use crate::drivers::ps2;
use crate::fs;
use crate::fs::fd::{FdEntry, FdError};
use crate::fs::vfs::{FileType, VfsError};
use crate::fs::FsError;
use crate::proc::scheduler;
use crate::user;
use alloc::string::String;
use alloc::vec::Vec;
use core::mem::size_of;

const MAX_IO: usize = 4096;
const MAX_PATH_BYTES: usize = 256;

#[repr(C)]
#[derive(Clone, Copy)]
struct UserStat {
    ino: u32,
    mode: u32,
    size: u64,
    kind: u32,
    pad: u32,
}

const _: () = assert!(size_of::<UserStat>() == 24);

#[no_mangle]
pub extern "C" fn syscall_dispatch(frame: *mut SyscallFrame) -> u64 {
    let frame = unsafe { &*frame };
    let number = frame.rax;
    let arg0 = frame.rdi;
    let arg1 = frame.rsi;
    let arg2 = frame.rdx;
    match abi::classify(number) {
        Some(abi::Syscall::Read) => sys_read(arg0, arg1, arg2),
        Some(abi::Syscall::Write) => sys_write(arg0, arg1, arg2),
        Some(abi::Syscall::Open) => sys_open(arg0, arg1, arg2),
        Some(abi::Syscall::Close) => sys_close(arg0),
        Some(abi::Syscall::Stat) => sys_stat(arg0, arg1),
        Some(abi::Syscall::Lseek) => sys_lseek(arg0, arg1, arg2),
        Some(abi::Syscall::GetPid) => sys_getpid(),
        Some(abi::Syscall::Exit) => sys_exit(arg0),
        None => abi::errno(abi::ENOSYS),
    }
}

fn sys_read(fd: u64, buf: u64, len: u64) -> u64 {
    let entry = match fs::fd_entry(fd as u32) {
        Ok(entry) => entry,
        Err(error) => return fs_errno(error),
    };
    match entry {
        FdEntry::Keyboard => {
            if len == 0 {
                return 0;
            }
            if !user::stack_valid(buf as usize, 1) {
                return abi::errno(abi::EFAULT);
            }
            loop {
                if let Some(key) = ps2::read_key() {
                    unsafe { core::ptr::write_volatile(buf as *mut u8, key) };
                    return 1;
                }
                if scheduler::block_on_keyboard().is_err() {
                    return abi::errno(abi::EIO);
                }
            }
        }
        FdEntry::Console => abi::errno(abi::EINVAL),
        FdEntry::File(_) => {
            if len == 0 {
                return 0;
            }
            let len = len as usize;
            if len > MAX_IO {
                return abi::errno(abi::EINVAL);
            }
            if !user::stack_valid(buf as usize, len) {
                return abi::errno(abi::EFAULT);
            }
            let buffer = unsafe { core::slice::from_raw_parts_mut(buf as *mut u8, len) };
            match fs::read_file_fd(fd as u32, buffer) {
                Ok(count) => count as u64,
                Err(error) => fs_errno(error),
            }
        }
    }
}

fn sys_write(fd: u64, buf: u64, len: u64) -> u64 {
    let entry = match fs::fd_entry(fd as u32) {
        Ok(entry) => entry,
        Err(error) => return fs_errno(error),
    };
    match entry {
        FdEntry::Console => {
            if len == 0 {
                return 0;
            }
            let len = len as usize;
            if len > MAX_IO {
                return abi::errno(abi::EINVAL);
            }
            if !user::range_valid(buf as usize, len) {
                return abi::errno(abi::EFAULT);
            }
            let bytes = unsafe { core::slice::from_raw_parts(buf as *const u8, len) };
            for byte in bytes {
                console::write_byte(*byte);
            }
            len as u64
        }
        FdEntry::Keyboard => abi::errno(abi::EBADF),
        FdEntry::File(_) => abi::errno(abi::EROFS),
    }
}

fn sys_open(path: u64, flags: u64, _mode: u64) -> u64 {
    if flags != 0 {
        return abi::errno(abi::EACCES);
    }
    let path = match user_path(path) {
        Ok(path) => path,
        Err(error) => return error,
    };
    match fs::open_fd(&path) {
        Ok(fd) => fd as u64,
        Err(error) => fs_errno(error),
    }
}

fn sys_close(fd: u64) -> u64 {
    match fs::close_fd(fd as u32) {
        Ok(()) => 0,
        Err(error) => fs_errno(error),
    }
}

fn sys_stat(path: u64, buffer: u64) -> u64 {
    let path = match user_path(path) {
        Ok(path) => path,
        Err(error) => return error,
    };
    let stat = match fs::stat_path(&path) {
        Ok(stat) => stat,
        Err(error) => return vfs_errno(error),
    };
    if !user::stack_valid(buffer as usize, size_of::<UserStat>()) {
        return abi::errno(abi::EFAULT);
    }
    let kind = match stat.kind {
        FileType::Regular => 1,
        FileType::Directory => 2,
        FileType::Other => 0,
    };
    let user_stat = UserStat {
        ino: stat.node,
        mode: stat.mode as u32,
        size: stat.size,
        kind,
        pad: 0,
    };
    unsafe { core::ptr::write_unaligned(buffer as *mut UserStat, user_stat) };
    0
}

fn sys_lseek(fd: u64, offset: u64, whence: u64) -> u64 {
    match fs::seek_fd(fd as u32, offset as i64, whence as u8) {
        Ok(offset) => offset,
        Err(error) => fs_errno(error),
    }
}

fn sys_getpid() -> u64 {
    match scheduler::current_pid() {
        Some(pid) => pid as u64,
        None => abi::errno(abi::ESRCH),
    }
}

fn sys_exit(code: u64) -> u64 {
    scheduler::exit_current(code as i32)
}

fn user_path(pointer: u64) -> Result<String, u64> {
    let mut bytes = Vec::new();
    for offset in 0..MAX_PATH_BYTES {
        let address = (pointer as usize)
            .checked_add(offset)
            .ok_or_else(|| abi::errno(abi::EFAULT))?;
        if !user::range_valid(address, 1) {
            return Err(abi::errno(abi::EFAULT));
        }
        let byte = unsafe { core::ptr::read_volatile(address as *const u8) };
        if byte == 0 {
            return core::str::from_utf8(&bytes)
                .map(String::from)
                .map_err(|_| abi::errno(abi::EINVAL));
        }
        bytes.push(byte);
    }
    Err(abi::errno(abi::ENAMETOOLONG))
}

fn fs_errno(error: FsError) -> u64 {
    match error {
        FsError::Fd(FdError::InvalidFd) => abi::errno(abi::EBADF),
        FsError::Fd(FdError::NoFreeFd) => abi::errno(abi::EMFILE),
        FsError::Vfs(error) => vfs_errno(error),
    }
}

fn vfs_errno(error: VfsError) -> u64 {
    match error {
        VfsError::NotMounted => abi::errno(abi::EIO),
        VfsError::NotFound => abi::errno(abi::ENOENT),
        VfsError::NotDirectory => abi::errno(abi::ENOTDIR),
        VfsError::NotFile => abi::errno(abi::EISDIR),
        VfsError::InvalidPath => abi::errno(abi::EINVAL),
        VfsError::InvalidOffset => abi::errno(abi::EINVAL),
        VfsError::Corrupt => abi::errno(abi::EIO),
        VfsError::Device => abi::errno(abi::EIO),
        VfsError::Unsupported => abi::errno(abi::EINVAL),
    }
}
