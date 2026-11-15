pub mod abi;

use crate::arch::x86_64::cpu;
use crate::arch::x86_64::syscall::SyscallFrame;
use crate::console;
use crate::drivers::ps2;
use crate::fs;
use crate::fs::binfmt_elf;
use crate::fs::binfmt_elf::LoadError;
use crate::fs::elf;
use crate::fs::fd::{FdEntry, FdError};
use crate::fs::vfs::{FileType, VfsError};
use crate::mm::uaccess;
use crate::proc::scheduler::{self, ForkError, WaitResult};
use crate::proc::wait::WaitTarget;
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
    let frame = unsafe { &mut *frame };
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
        Some(abi::Syscall::Fork) => sys_fork(frame),
        Some(abi::Syscall::Execve) => sys_execve(frame, arg0, arg1, arg2),
        Some(abi::Syscall::WaitPid) => sys_waitpid(arg0, arg1, arg2),
        Some(abi::Syscall::Exit) => sys_exit(arg0),
        None => abi::errno(abi::ENOSYS),
    }
}

fn sys_read(fd: u64, buf: u64, len: u64) -> u64 {
    let entry = match scheduler::current_fd_entry(fd as u32) {
        Ok(entry) => entry,
        Err(error) => return fd_errno(error),
    };
    match entry {
        FdEntry::Keyboard => {
            if len == 0 {
                return 0;
            }
            if !uaccess::stack_valid(buf as usize, 1) {
                return abi::errno(abi::EFAULT);
            }
            loop {
                if let Some(key) = ps2::read_key() {
                    unsafe { core::ptr::write_volatile(buf as *mut u8, key) };
                    return 1;
                }
                match scheduler::block_on_keyboard() {
                    Ok(()) => {}
                    Err(scheduler::SchedulerError::KeyboardQueueFull) => {
                        return abi::errno(abi::EAGAIN)
                    }
                    Err(_) => return abi::errno(abi::EIO),
                }
            }
        }
        FdEntry::Console => abi::errno(abi::EINVAL),
        FdEntry::File(file) => {
            if len == 0 {
                return 0;
            }
            let len = len as usize;
            if len > MAX_IO {
                return abi::errno(abi::EINVAL);
            }
            if !uaccess::stack_valid(buf as usize, len) {
                return abi::errno(abi::EFAULT);
            }
            let buffer = unsafe { core::slice::from_raw_parts_mut(buf as *mut u8, len) };
            let count = match fs::read_node(file.node, file.offset, buffer) {
                Ok(count) => count,
                Err(error) => return vfs_errno(error),
            };
            if count > 0 {
                if let Err(error) =
                    scheduler::current_set_fd_offset(fd as u32, file.offset + count as u64)
                {
                    return fd_errno(error);
                }
            }
            count as u64
        }
    }
}

fn sys_write(fd: u64, buf: u64, len: u64) -> u64 {
    let entry = match scheduler::current_fd_entry(fd as u32) {
        Ok(entry) => entry,
        Err(error) => return fd_errno(error),
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
            if !uaccess::range_valid(buf as usize, len) {
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
    let file = match fs::open(&path) {
        Ok(file) => file,
        Err(error) => return vfs_errno(error),
    };
    match scheduler::current_open_fd(file.node) {
        Ok(fd) => fd as u64,
        Err(error) => fd_errno(error),
    }
}

fn sys_close(fd: u64) -> u64 {
    match scheduler::current_close_fd(fd as u32) {
        Ok(()) => 0,
        Err(error) => fd_errno(error),
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
    if !uaccess::stack_valid(buffer as usize, size_of::<UserStat>()) {
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
    let mut file = match scheduler::current_fd_entry(fd as u32) {
        Ok(FdEntry::File(file)) => file,
        Ok(_) => return abi::errno(abi::EINVAL),
        Err(error) => return fd_errno(error),
    };
    let new_offset = match fs::seek(&mut file, offset as i64, whence as u8) {
        Ok(offset) => offset,
        Err(error) => return vfs_errno(error),
    };
    match scheduler::current_set_fd_offset(fd as u32, new_offset) {
        Ok(()) => new_offset,
        Err(error) => fd_errno(error),
    }
}

fn sys_fork(frame: &SyscallFrame) -> u64 {
    match scheduler::fork_current(frame) {
        Ok(pid) => {
            crate::kprintln!(
                "fork: parent {} child {}",
                scheduler::current_pid().unwrap_or(0),
                pid
            );
            pid as u64
        }
        Err(error) => fork_errno(error),
    }
}

fn sys_execve(frame: &mut SyscallFrame, path: u64, argv: u64, envp: u64) -> u64 {
    let path = match user_path(path) {
        Ok(path) => path,
        Err(error) => return error,
    };
    let argv = match user_strings(argv) {
        Ok(argv) => argv,
        Err(error) => return error,
    };
    let envp = match user_strings(envp) {
        Ok(envp) => envp,
        Err(error) => return error,
    };
    let argv_refs: Vec<&str> = argv.iter().map(String::as_str).collect();
    let envp_refs: Vec<&str> = envp.iter().map(String::as_str).collect();
    let image = match binfmt_elf::load(&path, &argv_refs, &envp_refs) {
        Ok(image) => image,
        Err(error) => return load_errno(error),
    };
    let new_root = image.address_space.root();
    let entry = image.entry;
    let stack = image.stack_top;
    let old = match scheduler::replace_address_space(image.address_space) {
        Ok(old) => old,
        Err(_) => return abi::errno(abi::EIO),
    };
    cpu::write_cr3(new_root);
    drop(old);
    frame.r15 = 0;
    frame.r14 = 0;
    frame.r13 = 0;
    frame.r12 = 0;
    frame.r11 = 0;
    frame.r10 = 0;
    frame.r9 = 0;
    frame.r8 = 0;
    frame.rbp = 0;
    frame.rdi = 0;
    frame.rsi = 0;
    frame.rdx = 0;
    frame.rcx = 0;
    frame.rbx = 0;
    frame.rax = 0;
    frame.user_rip = entry;
    frame.user_rsp = stack;
    frame.user_rflags = 0x202;
    crate::kprintln!(
        "execve: pid {} {}",
        scheduler::current_pid().unwrap_or(0),
        path
    );
    0
}

fn sys_waitpid(pid: u64, status: u64, options: u64) -> u64 {
    if options != 0 {
        return abi::errno(abi::EINVAL);
    }
    if !uaccess::range_valid(status as usize, 4) {
        return abi::errno(abi::EFAULT);
    }
    let target = match pid as i32 {
        -1 => WaitTarget::Any,
        value if value > 0 => WaitTarget::Pid(value as u32),
        _ => return abi::errno(abi::EINVAL),
    };
    loop {
        match scheduler::try_wait(target) {
            WaitResult::Reaped(child, code) => {
                let wait_status = (code & 0xFF) << 8;
                unsafe {
                    core::ptr::write_unaligned(status as *mut i32, wait_status);
                }
                crate::kprintln!("waitpid: reaped pid {}", child);
                return child as u64;
            }
            WaitResult::NoChild => return abi::errno(abi::ECHILD),
            WaitResult::Block => {
                if scheduler::block_on_child(target).is_err() {
                    return abi::errno(abi::EIO);
                }
            }
        }
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
        if !uaccess::range_valid(address, 1) {
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

fn user_strings(pointer: u64) -> Result<Vec<String>, u64> {
    if pointer == 0 {
        return Ok(Vec::new());
    }
    let mut strings = Vec::new();
    let mut total = 0usize;
    for index in 0..=elf::MAX_ARGS {
        let address = (pointer as usize)
            .checked_add(index * 8)
            .ok_or_else(|| abi::errno(abi::EFAULT))?;
        if !uaccess::range_valid(address, 8) {
            return Err(abi::errno(abi::EFAULT));
        }
        let string_pointer = unsafe { core::ptr::read_unaligned(address as *const u64) };
        if string_pointer == 0 {
            return Ok(strings);
        }
        let string = user_string(string_pointer)?;
        total = total
            .checked_add(string.len() + 1)
            .ok_or_else(|| abi::errno(abi::E2BIG))?;
        if total > elf::MAX_ARG_BYTES {
            return Err(abi::errno(abi::E2BIG));
        }
        strings.push(string);
    }
    Err(abi::errno(abi::E2BIG))
}

fn user_string(pointer: u64) -> Result<String, u64> {
    let mut bytes = Vec::new();
    for offset in 0..=elf::MAX_STRING {
        let address = (pointer as usize)
            .checked_add(offset)
            .ok_or_else(|| abi::errno(abi::EFAULT))?;
        if !uaccess::range_valid(address, 1) {
            return Err(abi::errno(abi::EFAULT));
        }
        let byte = unsafe { core::ptr::read_volatile(address as *const u8) };
        if byte == 0 {
            return core::str::from_utf8(&bytes)
                .map(String::from)
                .map_err(|_| abi::errno(abi::EINVAL));
        }
        if offset == elf::MAX_STRING {
            return Err(abi::errno(abi::E2BIG));
        }
        bytes.push(byte);
    }
    Err(abi::errno(abi::E2BIG))
}

fn fd_errno(error: FdError) -> u64 {
    match error {
        FdError::InvalidFd | FdError::NoProcess => abi::errno(abi::EBADF),
        FdError::NoFreeFd => abi::errno(abi::EMFILE),
    }
}

fn fork_errno(error: ForkError) -> u64 {
    match error {
        ForkError::NoCurrent => abi::errno(abi::ESRCH),
        ForkError::NoFreeSlot | ForkError::PidExhausted | ForkError::QueueFull => {
            abi::errno(abi::EAGAIN)
        }
        ForkError::OutOfMemory => abi::errno(abi::ENOMEM),
    }
}

fn load_errno(error: LoadError) -> u64 {
    abi::errno(error.errno())
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
