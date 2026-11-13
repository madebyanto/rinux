use super::process::TaskEntry;
use core::arch::global_asm;

global_asm!(include_str!("../arch/x86_64/context.S"));

pub const KERNEL_STACK_SIZE: usize = 16 * 1024;
pub const INITIAL_RFLAGS: u64 = 0x002;
pub const USER_CS: u64 = 0x33;
pub const USER_SS: u64 = 0x2B;
pub const USER_RFLAGS: u64 = 0x202;

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
    fn user_task_start();
}

pub unsafe fn switch(old: *mut Context, new: *const Context) {
    context_switch(old, new);
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

pub fn prepare_user_stack(stack: &mut [u8], user_entry: u64, user_stack_top: u64) -> Context {
    Context {
        rsp: prepare_slot(stack),
        rflags: INITIAL_RFLAGS,
        rbx: user_task_start as *const () as usize as u64,
        rbp: USER_RFLAGS,
        r12: user_entry,
        r13: user_stack_top,
        r14: USER_CS,
        r15: USER_SS,
    }
}

#[no_mangle]
pub extern "C" fn task_finished() -> ! {
    super::scheduler::exit_current(0)
}
