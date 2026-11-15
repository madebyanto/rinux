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

/// Distance between the top of a kernel stack and the saved syscall frame.
///
/// Interrupts enter ring 0 with `TSS.rsp0` pointing at the same top and build a
/// trap frame of roughly 176 bytes there, so the copy must stay below that
/// window. `fork_child_start` also runs with interrupts disabled until
/// `sysretq`, which keeps the copy intact while it is being consumed.
const FORK_FRAME_OFFSET: usize = 256;

/// Builds the `ret` target slot for a fresh kernel stack.
///
/// `context_switch` ends with `ret`, so the stack top holds the address the
/// task starts at. `entry` must not return: both `task_start` and
/// `fork_child_start` finish by leaving the kernel for good.
fn prepare_slot(stack: &mut [u8], entry: usize) -> u64 {
    let top = (stack.as_mut_ptr() as usize + stack.len()) & !0xF;
    let return_slot = (top - core::mem::size_of::<u64>()) as *mut u64;
    unsafe { return_slot.write(entry as u64) };
    return_slot as u64
}

pub fn prepare_stack(stack: &mut [u8], entry: TaskEntry) -> Context {
    Context {
        rsp: prepare_slot(stack, task_start as *const () as usize),
        rflags: INITIAL_RFLAGS,
        rbx: entry as usize as u64,
        ..Context::default()
    }
}

/// Prepares a forked child to resume the parent's syscall return path.
///
/// The child starts directly in `fork_child_start`, not in `task_start`:
/// `task_start` enables interrupts before calling the entry point, and an IRQ
/// taken there would push a trap frame on top of the frame copy below.
pub fn prepare_fork_stack(stack: &mut [u8], frame: &SyscallFrame) -> Context {
    let top = (stack.as_mut_ptr() as usize + stack.len()) & !0xF;
    let frame_ptr = (top - FORK_FRAME_OFFSET) as *mut SyscallFrame;
    unsafe {
        core::ptr::copy_nonoverlapping(frame, frame_ptr, 1);
        // The child returns 0 from `fork`; every other register is inherited.
        (*frame_ptr).rax = 0;
    }
    Context {
        rsp: prepare_slot(stack, fork_child_start as *const () as usize),
        rflags: INITIAL_RFLAGS,
        r12: frame_ptr as u64,
        ..Context::default()
    }
}

#[no_mangle]
pub extern "C" fn task_finished() -> ! {
    super::scheduler::exit_current(0)
}
