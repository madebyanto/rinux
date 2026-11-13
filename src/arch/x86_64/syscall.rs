use core::arch::asm;
use core::mem::size_of;

core::arch::global_asm!(include_str!("syscall.S"));

const IA32_EFER: u32 = 0xC000_0080;
const IA32_STAR: u32 = 0xC000_0081;
const IA32_LSTAR: u32 = 0xC000_0082;
const IA32_FMASK: u32 = 0xC000_0084;

const EFER_SCE: u64 = 1;
const STAR_VALUE: u64 = 0x0023_0008_0000_0000;
const FMASK_VALUE: u64 = 0x700;

#[repr(C)]
pub struct SyscallFrame {
    pub r15: u64,
    pub r14: u64,
    pub r13: u64,
    pub r12: u64,
    pub r11: u64,
    pub r10: u64,
    pub r9: u64,
    pub r8: u64,
    pub rbp: u64,
    pub rdi: u64,
    pub rsi: u64,
    pub rdx: u64,
    pub rcx: u64,
    pub rbx: u64,
    pub rax: u64,
    pub user_rip: u64,
    pub user_cs: u64,
    pub user_rflags: u64,
    pub user_rsp: u64,
    pub user_ss: u64,
}

const _: () = assert!(size_of::<SyscallFrame>() == 160);

extern "C" {
    fn syscall_entry();
}

pub fn init() {
    unsafe {
        write_msr(IA32_STAR, STAR_VALUE);
        write_msr(IA32_LSTAR, syscall_entry as *const () as usize as u64);
        write_msr(IA32_FMASK, FMASK_VALUE);
        let efer = read_msr(IA32_EFER);
        write_msr(IA32_EFER, efer | EFER_SCE);
    }
}

pub fn star_value() -> u64 {
    STAR_VALUE
}

pub fn fmask_value() -> u64 {
    FMASK_VALUE
}

pub fn lstar() -> u64 {
    syscall_entry as *const () as usize as u64
}

unsafe fn read_msr(msr: u32) -> u64 {
    let low: u32;
    let high: u32;
    asm!(
        "rdmsr",
        in("ecx") msr,
        out("eax") low,
        out("edx") high,
        options(nomem, nostack, preserves_flags)
    );
    ((high as u64) << 32) | low as u64
}

unsafe fn write_msr(msr: u32, value: u64) {
    let low = value as u32;
    let high = (value >> 32) as u32;
    asm!(
        "wrmsr",
        in("ecx") msr,
        in("eax") low,
        in("edx") high,
        options(nomem, nostack, preserves_flags)
    );
}
