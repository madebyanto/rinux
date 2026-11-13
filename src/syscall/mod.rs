pub mod abi;

use crate::arch::x86_64::syscall::SyscallFrame;
use crate::console;
use crate::drivers::ps2;
use crate::proc::scheduler;
use crate::user;

const MAX_WRITE: usize = 4096;

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
        Some(abi::Syscall::GetPid) => sys_getpid(),
        Some(abi::Syscall::Exit) => sys_exit(arg0),
        None => abi::errno(abi::ENOSYS),
    }
}

fn sys_read(fd: u64, buf: u64, len: u64) -> u64 {
    if fd != 0 {
        return abi::errno(abi::EBADF);
    }
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

fn sys_write(fd: u64, buf: u64, len: u64) -> u64 {
    if fd != 1 {
        return abi::errno(abi::EBADF);
    }
    if len == 0 {
        return 0;
    }
    let len = len as usize;
    if len > MAX_WRITE {
        return abi::errno(abi::EINVAL);
    }
    if !user::range_valid(buf as usize, len) {
        return abi::errno(abi::EFAULT);
    }
    let bytes = unsafe { core::slice::from_raw_parts(buf as *const u8, len) };
    for &byte in bytes {
        console::write_byte(byte);
    }
    len as u64
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
