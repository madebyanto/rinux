//! Unified user register state shared by the signal machinery.
//!
//! Syscalls and interrupts save ring-3 state in two different frames: the
//! `syscall` path builds a [`SyscallFrame`] and the IDT stubs build a
//! [`TrapFrame`]. Signal delivery and `rt_sigreturn` only care about the twenty
//! architectural registers, so both frames convert into [`UserContext`] and
//! back. The layout matches [`SigContext`] exactly, which lets a frame written
//! to the user stack round-trip through `sigreturn` without reordering.

use super::idt::TrapFrame;
use super::syscall::SyscallFrame;
use crate::proc::signal::SigContext;
use core::mem::size_of;

/// General-purpose registers plus the ring-3 `iretq`/`sysretq` tail.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct UserContext {
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
    pub rip: u64,
    pub cs: u64,
    pub rflags: u64,
    pub rsp: u64,
    pub ss: u64,
}

const _: () = assert!(size_of::<UserContext>() == 160);
const _: () = assert!(size_of::<UserContext>() == size_of::<SigContext>());
const _: () = {
    assert!(core::mem::offset_of!(UserContext, r15) == 0);
    assert!(core::mem::offset_of!(UserContext, rbp) == 64);
    assert!(core::mem::offset_of!(UserContext, rdi) == 72);
    assert!(core::mem::offset_of!(UserContext, rax) == 112);
    assert!(core::mem::offset_of!(UserContext, rip) == 120);
    assert!(core::mem::offset_of!(UserContext, cs) == 128);
    assert!(core::mem::offset_of!(UserContext, rflags) == 136);
    assert!(core::mem::offset_of!(UserContext, rsp) == 144);
    assert!(core::mem::offset_of!(UserContext, ss) == 152);
};

impl UserContext {
    /// Captures the ring-3 state a `syscall` instruction saved.
    pub fn from_syscall_frame(frame: &SyscallFrame) -> Self {
        Self {
            r15: frame.r15,
            r14: frame.r14,
            r13: frame.r13,
            r12: frame.r12,
            r11: frame.r11,
            r10: frame.r10,
            r9: frame.r9,
            r8: frame.r8,
            rbp: frame.rbp,
            rdi: frame.rdi,
            rsi: frame.rsi,
            rdx: frame.rdx,
            rcx: frame.rcx,
            rbx: frame.rbx,
            rax: frame.rax,
            rip: frame.user_rip,
            cs: frame.user_cs,
            rflags: frame.user_rflags,
            rsp: frame.user_rsp,
            ss: frame.user_ss,
        }
    }

    /// Writes the context back into the frame `syscall.S` consumes on return.
    pub fn write_to_syscall_frame(self, frame: &mut SyscallFrame) {
        frame.r15 = self.r15;
        frame.r14 = self.r14;
        frame.r13 = self.r13;
        frame.r12 = self.r12;
        frame.r11 = self.r11;
        frame.r10 = self.r10;
        frame.r9 = self.r9;
        frame.r8 = self.r8;
        frame.rbp = self.rbp;
        frame.rdi = self.rdi;
        frame.rsi = self.rsi;
        frame.rdx = self.rdx;
        frame.rcx = self.rcx;
        frame.rbx = self.rbx;
        frame.rax = self.rax;
        frame.user_rip = self.rip;
        frame.user_cs = self.cs;
        frame.user_rflags = self.rflags;
        frame.user_rsp = self.rsp;
        frame.user_ss = self.ss;
    }

    /// Captures the ring-3 state an IDT stub saved, ignoring `vector` and
    /// `error_code`.
    pub fn from_trap_frame(frame: &TrapFrame) -> Self {
        Self {
            r15: frame.r15,
            r14: frame.r14,
            r13: frame.r13,
            r12: frame.r12,
            r11: frame.r11,
            r10: frame.r10,
            r9: frame.r9,
            r8: frame.r8,
            rbp: frame.rbp,
            rdi: frame.rdi,
            rsi: frame.rsi,
            rdx: frame.rdx,
            rcx: frame.rcx,
            rbx: frame.rbx,
            rax: frame.rax,
            rip: frame.rip,
            cs: frame.cs,
            rflags: frame.rflags,
            rsp: frame.rsp,
            ss: frame.ss,
        }
    }

    /// Writes the context back into a trap frame, leaving `vector` and
    /// `error_code` untouched.
    pub fn write_to_trap_frame(self, frame: &mut TrapFrame) {
        frame.r15 = self.r15;
        frame.r14 = self.r14;
        frame.r13 = self.r13;
        frame.r12 = self.r12;
        frame.r11 = self.r11;
        frame.r10 = self.r10;
        frame.r9 = self.r9;
        frame.r8 = self.r8;
        frame.rbp = self.rbp;
        frame.rdi = self.rdi;
        frame.rsi = self.rsi;
        frame.rdx = self.rdx;
        frame.rcx = self.rcx;
        frame.rbx = self.rbx;
        frame.rax = self.rax;
        frame.rip = self.rip;
        frame.cs = self.cs;
        frame.rflags = self.rflags;
        frame.rsp = self.rsp;
        frame.ss = self.ss;
    }

    /// The `mcontext` saved inside a `SignalFrame`.
    pub fn to_sig_context(self) -> SigContext {
        SigContext {
            r15: self.r15,
            r14: self.r14,
            r13: self.r13,
            r12: self.r12,
            r11: self.r11,
            r10: self.r10,
            r9: self.r9,
            r8: self.r8,
            rbp: self.rbp,
            rdi: self.rdi,
            rsi: self.rsi,
            rdx: self.rdx,
            rcx: self.rcx,
            rbx: self.rbx,
            rax: self.rax,
            rip: self.rip,
            cs: self.cs,
            rflags: self.rflags,
            rsp: self.rsp,
            ss: self.ss,
        }
    }

    /// Rebuilds a context from a (possibly user-edited) `mcontext`.
    pub fn from_sig_context(context: SigContext) -> Self {
        Self {
            r15: context.r15,
            r14: context.r14,
            r13: context.r13,
            r12: context.r12,
            r11: context.r11,
            r10: context.r10,
            r9: context.r9,
            r8: context.r8,
            rbp: context.rbp,
            rdi: context.rdi,
            rsi: context.rsi,
            rdx: context.rdx,
            rcx: context.rcx,
            rbx: context.rbx,
            rax: context.rax,
            rip: context.rip,
            cs: context.cs,
            rflags: context.rflags,
            rsp: context.rsp,
            ss: context.ss,
        }
    }
}
