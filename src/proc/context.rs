use super::process::TaskEntry;
use crate::arch::x86_64::syscall::SyscallFrame;
use core::arch::global_asm;

global_asm!(include_str!("../arch/x86_64/context.S"));

pub const KERNEL_STACK_SIZE: usize = 16 * 1024;
pub const INITIAL_RFLAGS: u64 = 0x002;

#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct Context {
    pub rsp: u64,
    pub rflags: u64,
    pub rbx: u64,
    pub rbp: u64,
    pub r12: u64,
    pub r13: u64,
    pub r14: u64,
    pub r15: u64,
}

const _: () = {
    assert!(core::mem::size_of::<Context>() == 64);
    assert!(core::mem::offset_of!(Context, rsp) == 0);
    assert!(core::mem::offset_of!(Context, rflags) == 8);
    assert!(core::mem::offset_of!(Context, rbx) == 16);
    assert!(core::mem::offset_of!(Context, rbp) == 24);
    assert!(core::mem::offset_of!(Context, r12) == 32);
    assert!(core::mem::offset_of!(Context, r13) == 40);
    assert!(core::mem::offset_of!(Context, r14) == 48);
    assert!(core::mem::offset_of!(Context, r15) == 56);
};

extern "C" {
    fn context_switch(old: *mut Context, new: *const Context);
    fn task_start();
    fn fork_child_start();
    fn enter_user_mode(entry: u64, stack_top: u64) -> !;
}

pub unsafe fn switch(old: *mut Context, new: *const Context) {
    context_switch(old, new);
}

/// Leaves the kernel and starts a ring-3 program on a freshly built stack.
///
/// This is used by `kernel_execve`, where no syscall return frame exists.
pub fn enter_user(entry: u64, stack_top: u64) -> ! {
    unsafe { enter_user_mode(entry, stack_top) }
}

fn prepare_slot(stack: &mut [u8]) -> u64 {
    let top = (stack.as_mut_ptr() as usize + stack.len()) & !0xF;
    let return_slot = (top - core::mem::size_of::<u64>()) as *mut u64;
    unsafe { return_slot.write(task_start as *const () as usize as u64) };
    return_slot as u64
}

pub fn prepare_stack(stack: &mut [u8], entry: TaskEntry) -> Context {
    Context {
        rsp: prepare_slot(stack),
        rflags: INITIAL_RFLAGS,
        rbx: entry as usize as u64,
        ..Context::default()
    }
}

pub fn prepare_fork_stack(stack: &mut [u8], frame: &SyscallFrame) -> Context {
    let top = (stack.as_mut_ptr() as usize + stack.len()) & !0xF;
    let frame_ptr = (top - 256) as *mut SyscallFrame;
    unsafe {
        core::ptr::copy_nonoverlapping(frame, frame_ptr, 1);
        (*frame_ptr).rax = 0;
    }
    Context {
        rsp: prepare_slot(stack),
        rflags: INITIAL_RFLAGS,
        rbx: fork_child_start as *const () as usize as u64,
        r12: frame_ptr as u64,
        ..Context::default()
    }
}

#[no_mangle]
pub extern "C" fn task_finished() -> ! {
    super::scheduler::exit_current(0)
}
