pub mod abi;

use crate::arch::x86_64::cpu;
use crate::arch::x86_64::frame::UserContext;
use crate::arch::x86_64::gdt;
use crate::arch::x86_64::syscall::SyscallFrame;
use crate::console;
use crate::drivers::ps2;
use crate::fs;
use crate::fs::binfmt_elf;
use crate::fs::binfmt_elf::LoadError;
use crate::fs::elf;
use crate::fs::fd::{FdEntry, FdError};
use crate::fs::pipe::{self, PipeDirection};
use crate::fs::vfs::{FileType, VfsError};
use crate::mm::uaccess;
use crate::proc::scheduler::{self, ForkError, SchedulerError, SignalError, WaitResult};
use crate::proc::signal::{
    self, SigAction, SigSet, Signal, SignalFrame, StackRange,
};
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
    let result = dispatch(frame);
    // The assembly stub stores this into the saved `rax`, so publish the
    // syscall result before a signal handler may overwrite it.
    frame.rax = result;
    // Signals are delivered on the way out to ring 3, exactly where Linux
    // checks `TIF_SIGPENDING`: before the interrupted instruction resumes.
    let mut context = UserContext::from_syscall_frame(frame);
    scheduler::deliver_pending_signals(&mut context);
    context.write_to_syscall_frame(frame);
    frame.rax
}

fn dispatch(frame: &mut SyscallFrame) -> u64 {
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
        Some(abi::Syscall::RtSigaction) => sys_sigaction(frame, arg0, arg1, arg2),
        Some(abi::Syscall::RtSigprocmask) => sys_sigprocmask(frame, arg0, arg1, arg2),
        Some(abi::Syscall::RtSigreturn) => sys_sigreturn(frame),
        Some(abi::Syscall::Pipe) => sys_pipe(arg0),
        Some(abi::Syscall::GetPid) => sys_getpid(),
        Some(abi::Syscall::Fork) => sys_fork(frame),
        Some(abi::Syscall::Execve) => sys_execve(frame, arg0, arg1, arg2),
        Some(abi::Syscall::WaitPid) => sys_waitpid(arg0, arg1, arg2),
        Some(abi::Syscall::Kill) => sys_kill(arg0, arg1),
        Some(abi::Syscall::Truncate) => sys_truncate(arg0, arg1),
        Some(abi::Syscall::Ftruncate) => sys_ftruncate(arg0, arg1),
        Some(abi::Syscall::Sync) => sys_sync(),
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
                // Retry the keyboard first, then let an interrupting signal win.
                if scheduler::pending_interrupting().is_some() {
                    scheduler::note_interrupted_syscall(abi::SYS_READ, true);
                    return abi::errno(abi::EINTR);
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
            if file.flags & abi::O_ACCMODE == abi::O_WRONLY {
                return abi::errno(abi::EBADF);
            }
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
        FdEntry::Pipe(end) => {
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
            let buffer = unsafe { core::slice::from_raw_parts_mut(buf as *mut u8, len) };
            loop {
                match end.try_read(buffer) {
                    pipe::ReadOutcome::Data(count) => return count as u64,
                    pipe::ReadOutcome::EndOfFile => return 0,
                    pipe::ReadOutcome::Empty => {
                        // A wakeup may have been a signal rather than new data:
                        // the loop retries the pipe first and only then reports
                        // `EINTR` if an interrupting signal is still pending.
                        if scheduler::pending_interrupting().is_some() {
                            scheduler::note_interrupted_syscall(abi::SYS_READ, true);
                            return abi::errno(abi::EINTR);
                        }
                        if let Err(error) =
                            scheduler::block_on_pipe(end.id(), PipeDirection::Read, || end.readable())
                        {
                            return scheduler_errno(error);
                        }
                    }
                    pipe::ReadOutcome::WrongDirection => return abi::errno(abi::EBADF),
                }
            }
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
        FdEntry::File(file) => {
            if file.flags & abi::O_ACCMODE == abi::O_RDONLY {
                return abi::errno(abi::EBADF);
            }
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
            if file.flags & abi::O_APPEND != 0 {
                let (count, end) = match fs::append_node(file.node, bytes) {
                    Ok(result) => result,
                    Err(error) => return vfs_errno(error),
                };
                if let Err(error) = scheduler::current_set_fd_offset(fd as u32, end) {
                    return fd_errno(error);
                }
                count as u64
            } else {
                let count = match fs::write_node(file.node, file.offset, bytes) {
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
        FdEntry::Pipe(end) => {
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
            loop {
                match end.try_write(bytes) {
                    pipe::WriteOutcome::Written(count) => return count as u64,
                    pipe::WriteOutcome::Full => {
                        if scheduler::pending_interrupting().is_some() {
                            scheduler::note_interrupted_syscall(abi::SYS_WRITE, true);
                            return abi::errno(abi::EINTR);
                        }
                        if let Err(error) = scheduler::block_on_pipe(
                            end.id(),
                            PipeDirection::Write,
                            || end.writable(),
                        ) {
                            return scheduler_errno(error);
                        }
                    }
                    pipe::WriteOutcome::BrokenPipe => {
                        // As on Linux, the write reports `EPIPE` to the caller
                        // and the pending `SIGPIPE` is delivered on the way
                        // back to ring 3 (terminating the task by default).
                        let info =
                            signal::Siginfo::kernel(Signal::PIPE, signal::SI_KERNEL, 0);
                        let _ = scheduler::raise_current_signal(Signal::PIPE, info);
                        return abi::errno(abi::EPIPE);
                    }
                    pipe::WriteOutcome::WrongDirection => return abi::errno(abi::EBADF),
                }
            }
        }
    }
}

fn sys_open(path: u64, flags: u64, mode: u64) -> u64 {
    let flags = flags as u32;
    let access = match abi::validate_open_flags(flags) {
        Some(access) => access,
        None => return abi::errno(abi::EINVAL),
    };
    if flags & abi::O_TRUNC != 0 && !access.writable() {
        return abi::errno(abi::EACCES);
    }
    let path = match user_path(path) {
        Ok(path) => path,
        Err(error) => return error,
    };
    let mode = if mode == 0 {
        0o644
    } else {
        (mode as u16) & 0o7777
    };
    let file = match fs::open_with_flags(&path, flags, mode) {
        Ok(file) => file,
        Err(error) => return vfs_errno(error),
    };
    match scheduler::current_open_fd(file.node, flags) {
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

fn sys_truncate(path: u64, length: u64) -> u64 {
    if (length as i64) < 0 {
        return abi::errno(abi::EINVAL);
    }
    let path = match user_path(path) {
        Ok(path) => path,
        Err(error) => return error,
    };
    match fs::truncate_path(&path, length) {
        Ok(()) => 0,
        Err(error) => vfs_errno(error),
    }
}

fn sys_ftruncate(fd: u64, length: u64) -> u64 {
    if (length as i64) < 0 {
        return abi::errno(abi::EINVAL);
    }
    let file = match scheduler::current_fd_entry(fd as u32) {
        Ok(FdEntry::File(file)) => file,
        Ok(_) => return abi::errno(abi::EBADF),
        Err(error) => return fd_errno(error),
    };
    if file.flags & abi::O_ACCMODE == abi::O_RDONLY {
        return abi::errno(abi::EINVAL);
    }
    match fs::truncate_node(file.node, length) {
        Ok(()) => 0,
        Err(error) => vfs_errno(error),
    }
}

fn sys_sync() -> u64 {
    match fs::sync() {
        Ok(()) => 0,
        Err(error) => vfs_errno(error),
    }
}

fn sys_pipe(fds: u64) -> u64 {
    if !uaccess::range_valid(fds as usize, size_of::<[i32; 2]>()) {
        return abi::errno(abi::EFAULT);
    }
    let id = match scheduler::allocate_pipe() {
        Ok(id) => id,
        Err(error) => return scheduler_errno(error),
    };
    let (read_end, write_end) = match pipe::create(id) {
        Ok(ends) => ends,
        Err(_) => return abi::errno(abi::ENOMEM),
    };
    let (read_fd, write_fd) = match scheduler::current_install_pipe(&read_end, &write_end) {
        Ok(pair) => pair,
        Err(error) => return fd_errno(error),
    };
    unsafe {
        core::ptr::write_unaligned(fds as *mut [i32; 2], [read_fd as i32, write_fd as i32]);
    }
    0
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
    // The image is committed; there is no error path left. Exec keeps the
    // blocked mask but resets caught dispositions and discards pending signals.
    let _ = scheduler::reset_signals_for_exec();
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
    if options & !abi::WNOHANG != 0 {
        return abi::errno(abi::EINVAL);
    }
    let no_hang = options & abi::WNOHANG != 0;
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
            WaitResult::Reaped(child, exit_status) => {
                let wait_status = exit_status.wait_status();
                unsafe {
                    core::ptr::write_unaligned(status as *mut i32, wait_status);
                }
                crate::kprintln!("waitpid: reaped pid {}", child);
                return child as u64;
            }
            WaitResult::NoChild => return abi::errno(abi::ECHILD),
            WaitResult::Block => {
                if no_hang {
                    return 0;
                }
                // Reaping is retried at the top of the loop before `EINTR` is
                // reported, so a child that exited while blocked is collected
                // even when a `SIGCHLD`/other signal is pending.
                if scheduler::pending_interrupting().is_some() {
                    scheduler::note_interrupted_syscall(abi::SYS_WAITPID, true);
                    return abi::errno(abi::EINTR);
                }
                if let Err(error) = scheduler::block_on_child(target) {
                    return scheduler_errno(error);
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

fn sys_kill(pid: u64, signal: u64) -> u64 {
    // The signal argument is an `int`; the ABI leaves the high half
    // unspecified, so inspect only the low 32 bits.
    let raw_signal = signal as i32;
    if raw_signal < 0 {
        return abi::errno(abi::EINVAL);
    }
    let signal = if raw_signal == 0 {
        None
    } else {
        match u8::try_from(raw_signal).ok().and_then(Signal::from_u8) {
            Some(signal) => Some(signal),
            None => return abi::errno(abi::EINVAL),
        }
    };

    let result = match pid as i32 {
        target if target > 0 => match signal {
            Some(signal) => scheduler::send_signal(target as u32, signal),
            None => scheduler::check_signal_target(target as u32),
        },
        // No process groups: pid 0 means the calling process.
        0 => match scheduler::current_pid() {
            Some(current) => match signal {
                Some(signal) => scheduler::send_signal(current, signal),
                None => Ok(()),
            },
            None => Err(SignalError::NoCurrentProcess),
        },
        // Broadcast to every process this one may signal, except init and self.
        -1 => scheduler::send_signal_all(signal),
        _ => return abi::errno(abi::EINVAL),
    };
    match result {
        Ok(()) => 0,
        Err(error) => signal_errno(error),
    }
}

fn sys_sigaction(frame: &SyscallFrame, sig: u64, act: u64, oldact: u64) -> u64 {
    if frame.r10 != abi::SIGSET_SIZE as u64 {
        return abi::errno(abi::EINVAL);
    }
    let signal = match u8::try_from(sig).ok().and_then(Signal::from_u8) {
        Some(signal) => signal,
        None => return abi::errno(abi::EINVAL),
    };
    // SIGKILL/SIGSTOP have no queryable disposition, as on Linux.
    if signal.is_uncatchable() {
        return abi::errno(abi::EINVAL);
    }
    let action_size = size_of::<abi::KernelSigAction>();
    if act != 0 && !uaccess::range_valid(act as usize, action_size) {
        return abi::errno(abi::EFAULT);
    }
    if oldact != 0 && !uaccess::range_valid(oldact as usize, action_size) {
        return abi::errno(abi::EFAULT);
    }

    let previous = if act != 0 {
        let raw = unsafe { core::ptr::read_unaligned(act as *const abi::KernelSigAction) };
        match scheduler::install_signal_action(signal, action_from_user(raw)) {
            Ok(previous) => previous,
            Err(error) => return signal_errno(error),
        }
    } else {
        match scheduler::signal_action(signal) {
            Ok(action) => action,
            Err(error) => return signal_errno(error),
        }
    };

    if oldact != 0 {
        let outgoing = action_to_user(previous);
        unsafe { core::ptr::write_unaligned(oldact as *mut abi::KernelSigAction, outgoing) };
    }
    0
}

fn sys_sigprocmask(frame: &SyscallFrame, how: u64, set: u64, oldset: u64) -> u64 {
    if frame.r10 != abi::SIGSET_SIZE as u64 {
        return abi::errno(abi::EINVAL);
    }
    if how > scheduler::SIG_SETMASK {
        return abi::errno(abi::EINVAL);
    }
    if set != 0 && !uaccess::range_valid(set as usize, abi::SIGSET_SIZE) {
        return abi::errno(abi::EFAULT);
    }
    if oldset != 0 && !uaccess::range_valid(oldset as usize, abi::SIGSET_SIZE) {
        return abi::errno(abi::EFAULT);
    }

    let previous = if set != 0 {
        let bits = unsafe { core::ptr::read_unaligned(set as *const u64) };
        match scheduler::set_blocked_signals(how, SigSet::from_bits(bits)) {
            Ok(previous) => previous,
            Err(error) => return signal_errno(error),
        }
    } else {
        match scheduler::blocked_signals() {
            Ok(previous) => previous,
            Err(error) => return signal_errno(error),
        }
    };

    if oldset != 0 {
        unsafe { core::ptr::write_unaligned(oldset as *mut u64, previous.bits()) };
    }
    0
}

/// `rt_sigreturn`: restores the context saved by the last delivery.
///
/// The trampoline runs one word into the frame (`rsp == frame + 8`), so
/// [`signal::locate_frame`] walks back to the base. Everything read here is
/// user-writable, so the magic, stack range, selectors and resume addresses
/// are all revalidated before any register is trusted.
fn sys_sigreturn(frame: &mut SyscallFrame) -> u64 {
    let Some(signal_frame) = read_signal_frame(frame.user_rsp) else {
        // Linux turns a corrupt `rt_sigreturn` into SIGSEGV; Rinux has no
        // recoverable kernel frame to return to, so terminate instead.
        scheduler::exit_current_signaled(Signal::SEGV);
    };
    let saved = signal_frame.context;
    if saved.cs != gdt::USER_CODE_RPL3 as u64
        || saved.ss != gdt::USER_DATA_RPL3 as u64
        || !user_executable(saved.rip)
        || !uaccess::stack_valid(saved.rsp as usize, 0)
    {
        scheduler::exit_current_signaled(Signal::SEGV);
    }
    if scheduler::finish_sigreturn(SigSet::from_bits(signal_frame.saved_mask)).is_err() {
        scheduler::exit_current_signaled(Signal::SEGV);
    }
    let mut context = UserContext::from_sig_context(saved);
    context.rflags = signal::sanitize_rflags(saved.rflags);
    context.write_to_syscall_frame(frame);
    // Returning `rax` here makes `syscall.S` store the restored value, not a
    // syscall return code, into the interrupted register.
    context.rax
}

/// Validates and copies the frame the trampoline pointed at.
fn read_signal_frame(user_rsp: u64) -> Option<SignalFrame> {
    let stack = StackRange {
        base: uaccess::STACK_BASE as u64,
        top: uaccess::STACK_TOP as u64,
    };
    let address = signal::locate_frame(user_rsp, stack)?;
    let frame = unsafe { SignalFrame::read_from(address) };
    if frame.signal().is_none() {
        return None;
    }
    Some(frame)
}

/// Whether `rip` may be resumed: the program window or the trampoline page.
fn user_executable(address: u64) -> bool {
    let address = address as usize;
    uaccess::program_valid(address, 1) || uaccess::trampoline_valid(address, 1)
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

fn action_from_user(raw: abi::KernelSigAction) -> SigAction {
    SigAction {
        handler: raw.handler,
        flags: raw.flags,
        restorer: raw.restorer,
        mask: SigSet::from_bits(raw.mask),
    }
}

fn action_to_user(action: SigAction) -> abi::KernelSigAction {
    abi::KernelSigAction {
        handler: action.handler,
        flags: action.flags,
        restorer: action.restorer,
        mask: action.mask.bits(),
    }
}

fn signal_errno(error: SignalError) -> u64 {
    match error {
        SignalError::NotInitialized | SignalError::NoCurrentProcess => abi::errno(abi::ESRCH),
        SignalError::NoSuchProcess => abi::errno(abi::ESRCH),
        SignalError::PermissionDenied => abi::errno(abi::EPERM),
        SignalError::Uncatchable
        | SignalError::UnsupportedFlags
        | SignalError::InvalidHandler
        | SignalError::InvalidHow => abi::errno(abi::EINVAL),
    }
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

fn scheduler_errno(error: SchedulerError) -> u64 {
    match error {
        SchedulerError::NotInitialized | SchedulerError::NoCurrentProcess => {
            abi::errno(abi::ESRCH)
        }
        SchedulerError::KeyboardQueueFull => abi::errno(abi::EAGAIN),
        SchedulerError::PipeTableFull => abi::errno(abi::ENFILE),
        SchedulerError::InvalidPipe => abi::errno(abi::EBADF),
        SchedulerError::AlreadyInitialized => abi::errno(abi::EIO),
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
        VfsError::AlreadyExists => abi::errno(abi::EEXIST),
        VfsError::InvalidPath => abi::errno(abi::EINVAL),
        VfsError::InvalidOffset => abi::errno(abi::EINVAL),
        VfsError::Corrupt => abi::errno(abi::EIO),
        VfsError::Device => abi::errno(abi::EIO),
        VfsError::Unsupported => abi::errno(abi::EOPNOTSUPP),
        VfsError::ReadOnly => abi::errno(abi::EROFS),
        VfsError::NoSpace => abi::errno(abi::ENOSPC),
        VfsError::FileTooLarge => abi::errno(abi::EFBIG),
    }
}
