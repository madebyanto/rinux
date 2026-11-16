//! Scheduler-free Unix signal core.
//!
//! This module owns the kernel's signal vocabulary and the layout of the
//! frame that is stretched onto the user stack when a handler runs. It knows
//! nothing about the scheduler, address spaces or user pointers: callers
//! validate the stack range and hand in raw addresses, which keeps every rule
//! here testable on the host.
//!
//! The stateful half (pending/blocked sets, dispositions, delivery and
//! `sigreturn`) lives in `scheduler.rs` and is wired up by the signal
//! syscalls in later steps.
#![allow(dead_code)] // consumed incrementally by the scheduler/syscall wiring

use core::mem::size_of;

/// Number of supported signals. Rinux uses the Linux x86-64 numbers 1..=31.
pub const NSIG: usize = 31;

/// Bit mask with one bit per valid signal (`bit = signo - 1`).
pub const VALID_SIGNAL_BITS: u64 = (1u64 << NSIG) - 1;

/// Signal number encoded by the sigreturn trampoline.
pub const SIGRETURN_NUMBER: u64 = 15;

/// Machine code of the trampoline: `mov rax, 15; syscall; ud2`.
///
/// The handler returns with `ret`, which pops the address the kernel pushed as
/// its return slot, so the trampoline runs with the frame base one word below
/// `rsp`. `sys_sigreturn` locates the frame from there. The page itself lives
/// at `mm::uaccess::TRAMPOLINE_BASE` and is mapped by the ELF loader.
pub const TRAMPOLINE_CODE: [u8; 11] = [
    0x48,
    0xC7,
    0xC0,
    SIGRETURN_NUMBER as u8,
    0x00,
    0x00,
    0x00,
    0x0F,
    0x05,
    0x0F,
    0x0B,
];

/// Maximum number of nested handler frames on the 16 KiB user stack.
///
/// Reaching the limit forces the pending signal's default action instead of
/// recursing further.
pub const MAX_SIGNAL_DEPTH: u8 = 8;

/// SysV ABI red zone preserved below the interrupted `rsp` by delivery.
pub const FRAME_RED_ZONE: u64 = 128;

/// Minimum distance kept between the frame and `STACK_BASE`, so a handler has
/// room for a small prologue even at the bottom of a nearly exhausted stack.
pub const FRAME_STACK_MARGIN: u64 = 64;

/// A real signal number, always in `1..=NSIG`.
#[repr(transparent)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Signal(u8);

impl Signal {
    pub const HUP: Self = Self(1);
    pub const INT: Self = Self(2);
    pub const QUIT: Self = Self(3);
    pub const ILL: Self = Self(4);
    pub const TRAP: Self = Self(5);
    pub const ABRT: Self = Self(6);
    pub const BUS: Self = Self(7);
    pub const FPE: Self = Self(8);
    pub const KILL: Self = Self(9);
    pub const USR1: Self = Self(10);
    pub const SEGV: Self = Self(11);
    pub const USR2: Self = Self(12);
    pub const PIPE: Self = Self(13);
    pub const ALRM: Self = Self(14);
    pub const TERM: Self = Self(15);
    pub const STKFLT: Self = Self(16);
    pub const CHLD: Self = Self(17);
    pub const CONT: Self = Self(18);
    pub const STOP: Self = Self(19);
    pub const TSTP: Self = Self(20);
    pub const TTIN: Self = Self(21);
    pub const TTOU: Self = Self(22);
    pub const URG: Self = Self(23);
    pub const XCPU: Self = Self(24);
    pub const XFSZ: Self = Self(25);
    pub const VTALRM: Self = Self(26);
    pub const PROF: Self = Self(27);
    pub const WINCH: Self = Self(28);
    pub const IO: Self = Self(29);
    pub const PWR: Self = Self(30);
    pub const SYS: Self = Self(31);

    /// Every supported signal in ascending order.
    pub const ALL: [Self; NSIG] = [
        Self::HUP,
        Self::INT,
        Self::QUIT,
        Self::ILL,
        Self::TRAP,
        Self::ABRT,
        Self::BUS,
        Self::FPE,
        Self::KILL,
        Self::USR1,
        Self::SEGV,
        Self::USR2,
        Self::PIPE,
        Self::ALRM,
        Self::TERM,
        Self::STKFLT,
        Self::CHLD,
        Self::CONT,
        Self::STOP,
        Self::TSTP,
        Self::TTIN,
        Self::TTOU,
        Self::URG,
        Self::XCPU,
        Self::XFSZ,
        Self::VTALRM,
        Self::PROF,
        Self::WINCH,
        Self::IO,
        Self::PWR,
        Self::SYS,
    ];

    pub const fn from_u8(raw: u8) -> Option<Self> {
        if raw >= 1 && raw as usize <= NSIG {
            Some(Self(raw))
        } else {
            None
        }
    }

    pub const fn number(self) -> u8 {
        self.0
    }

    /// Index into the per-process disposition table.
    pub const fn index(self) -> usize {
        self.0 as usize - 1
    }

    /// `SIGKILL` and `SIGSTOP` can be neither caught, ignored nor blocked.
    pub const fn is_uncatchable(self) -> bool {
        self.0 == Self::KILL.0 || self.0 == Self::STOP.0
    }

    pub const fn name(self) -> &'static str {
        match self.0 {
            1 => "SIGHUP",
            2 => "SIGINT",
            3 => "SIGQUIT",
            4 => "SIGILL",
            5 => "SIGTRAP",
            6 => "SIGABRT",
            7 => "SIGBUS",
            8 => "SIGFPE",
            9 => "SIGKILL",
            10 => "SIGUSR1",
            11 => "SIGSEGV",
            12 => "SIGUSR2",
            13 => "SIGPIPE",
            14 => "SIGALRM",
            15 => "SIGTERM",
            16 => "SIGSTKFLT",
            17 => "SIGCHLD",
            18 => "SIGCONT",
            19 => "SIGSTOP",
            20 => "SIGTSTP",
            21 => "SIGTTIN",
            22 => "SIGTTOU",
            23 => "SIGURG",
            24 => "SIGXCPU",
            25 => "SIGXFSZ",
            26 => "SIGVTALRM",
            27 => "SIGPROF",
            28 => "SIGWINCH",
            29 => "SIGIO",
            30 => "SIGPWR",
            31 => "SIGSYS",
            _ => "SIG?",
        }
    }

    const fn bit(self) -> u64 {
        1u64 << (self.0 - 1)
    }
}

/// A signal set (`sigset_t`), laid out like Linux's: bit `signo - 1`.
///
/// The kernel set is one `u64`, so `rt_sigaction`/`rt_sigprocmask` accept a
/// `sigsetsize` of eight bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SigSet(u64);

impl SigSet {
    pub const EMPTY: Self = Self(0);

    /// Builds a set from raw bits, discarding anything outside signals 1..=31.
    pub const fn from_bits(bits: u64) -> Self {
        Self(bits & VALID_SIGNAL_BITS)
    }

    pub const fn bits(self) -> u64 {
        self.0
    }

    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }

    pub const fn singleton(signal: Signal) -> Self {
        Self(signal.bit())
    }

    pub const fn contains(self, signal: Signal) -> bool {
        self.0 & signal.bit() != 0
    }

    pub const fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }

    pub const fn intersection(self, other: Self) -> Self {
        Self(self.0 & other.0)
    }

    pub const fn difference(self, other: Self) -> Self {
        Self(self.0 & !other.0)
    }

    pub fn insert(&mut self, signal: Signal) {
        self.0 |= signal.bit();
    }

    pub fn remove(&mut self, signal: Signal) {
        self.0 &= !signal.bit();
    }

    pub fn insert_all(&mut self, other: Self) {
        self.0 |= other.0;
    }

    /// Lowest signal in the set, if any.
    pub fn lowest(self) -> Option<Signal> {
        if self.0 == 0 {
            return None;
        }
        Signal::from_u8(self.0.trailing_zeros() as u8 + 1)
    }

    /// The set without `SIGKILL`/`SIGSTOP`, which can never be blocked.
    pub const fn without_uncatchable(self) -> Self {
        Self(self.0 & !(Signal::KILL.bit() | Signal::STOP.bit()))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DefaultAction {
    Terminate,
    /// Terminate and report the core-dump bit; Rinux has no writable
    /// filesystem, so nothing is written.
    TerminateCore,
    Ignore,
    /// No job control yet, so phase 1 treats stop as ignore.
    Stop,
}

/// POSIX default disposition for every supported signal.
pub const fn default_action(signal: Signal) -> DefaultAction {
    match signal.0 {
        1 | 2 | 9 | 10 | 12 | 13 | 14 | 15 | 16 | 26 | 27 | 29 | 30 => DefaultAction::Terminate,
        3 | 4 | 5 | 6 | 7 | 8 | 11 | 24 | 25 | 31 => DefaultAction::TerminateCore,
        17 | 18 | 23 | 28 => DefaultAction::Ignore,
        19 | 20 | 21 | 22 => DefaultAction::Stop,
        _ => DefaultAction::Terminate,
    }
}

/// Synchronous CPU faults that the architecture layer turns into signals.
///
/// Kept architecture-neutral so the mapping can be unit-tested on the host;
/// `idt.rs` translates x86 vectors into these variants.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HardwareFault {
    DivideError,
    Debug,
    Breakpoint,
    InvalidOpcode,
    AlignmentCheck,
    /// Page fault: `present` is the x86 P bit and `address` is CR2.
    PageFault { present: bool, address: u64 },
}

/// POSIX signal and `siginfo_t` for a synchronous hardware fault.
///
/// `instruction` is the interrupted RIP, used as `si_addr` for faults that do
/// not report a memory address.
pub fn hardware_fault_signal(fault: HardwareFault, instruction: u64) -> (Signal, Siginfo) {
    let (signal, code, addr) = match fault {
        HardwareFault::DivideError => (Signal::FPE, FPE_INTDIV, 0),
        HardwareFault::Debug => (Signal::TRAP, TRAP_TRACE, instruction),
        HardwareFault::Breakpoint => (Signal::TRAP, TRAP_BRKPT, instruction),
        HardwareFault::InvalidOpcode => (Signal::ILL, ILL_ILLOPC, instruction),
        HardwareFault::AlignmentCheck => (Signal::BUS, BUS_ADRALN, 0),
        HardwareFault::PageFault { present, address } => (
            Signal::SEGV,
            if present { SEGV_ACCERR } else { SEGV_MAPERR },
            address,
        ),
    };
    (signal, Siginfo::kernel(signal, code, addr))
}

/// Wait-status word for a child that `signal` terminated.
///
/// The low seven bits are the signal number; bit 7 is the core-dump flag,
/// which mirrors the signal's default disposition (`TerminateCore`). User
/// handlers run before this is reached, so a caught signal never reports a
/// core dump just because its default action would have produced one.
pub const fn signal_wait_status(signal: Signal) -> i32 {
    let core = matches!(default_action(signal), DefaultAction::TerminateCore);
    signal.number() as i32 | if core { 0x80 } else { 0 }
}

pub const SIG_DFL: u64 = 0;
pub const SIG_IGN: u64 = 1;

// Linux x86-64 values.
pub const SA_NOCLDSTOP: u64 = 0x0000_0001;
pub const SA_NOCLDWAIT: u64 = 0x0000_0002;
pub const SA_SIGINFO: u64 = 0x0000_0004;
pub const SA_RESTORER: u64 = 0x0400_0000;
pub const SA_ONSTACK: u64 = 0x0800_0000;
pub const SA_RESTART: u64 = 0x1000_0000;
pub const SA_NODEFER: u64 = 0x4000_0000;
pub const SA_RESETHAND: u64 = 0x8000_0000;

/// Every flag the kernel understands; `sys_sigaction` rejects anything else.
pub const SA_SUPPORTED: u64 = SA_NOCLDSTOP
    | SA_NOCLDWAIT
    | SA_SIGINFO
    | SA_RESTORER
    | SA_ONSTACK
    | SA_RESTART
    | SA_NODEFER
    | SA_RESETHAND;

/// One entry of the per-process disposition table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SigAction {
    /// `SIG_DFL`, `SIG_IGN`, or a user virtual address in the program window.
    pub handler: u64,
    pub flags: u64,
    /// Only meaningful with `SA_RESTORER`; otherwise the kernel trampoline is
    /// used.
    pub restorer: u64,
    /// Extra signals blocked while the handler runs.
    pub mask: SigSet,
}

impl SigAction {
    pub const DEFAULT: Self = Self {
        handler: SIG_DFL,
        flags: 0,
        restorer: 0,
        mask: SigSet::EMPTY,
    };

    pub const IGNORE: Self = Self {
        handler: SIG_IGN,
        flags: 0,
        restorer: 0,
        mask: SigSet::EMPTY,
    };

    pub const fn with_handler(handler: u64) -> Self {
        Self {
            handler,
            flags: 0,
            restorer: 0,
            mask: SigSet::EMPTY,
        }
    }

    pub const fn is_default(self) -> bool {
        self.handler == SIG_DFL
    }

    pub const fn is_ignored(self) -> bool {
        self.handler == SIG_IGN
    }

    /// A real user handler (anything above the two sentinel values).
    pub const fn is_caught(self) -> bool {
        self.handler > SIG_IGN
    }

    pub const fn flags_supported(self) -> bool {
        self.flags & !SA_SUPPORTED == 0
    }

    pub const fn restarts(self) -> bool {
        self.flags & SA_RESTART != 0
    }

    pub const fn reports_info(self) -> bool {
        self.flags & SA_SIGINFO != 0
    }

    pub const fn uses_altstack(self) -> bool {
        self.flags & SA_ONSTACK != 0
    }

    pub const fn resets_on_entry(self) -> bool {
        self.flags & SA_RESETHAND != 0
    }

    /// Without `SA_NODEFER`, the delivered signal stays blocked during its own
    /// handler.
    pub const fn nodefers(self) -> bool {
        self.flags & SA_NODEFER != 0
    }

    pub const fn uses_restorer(self) -> bool {
        self.flags & SA_RESTORER != 0
    }
}

/// Mask in force while `signal`'s handler runs.
pub fn handler_mask(blocked: SigSet, action: SigAction, signal: Signal) -> SigSet {
    let mut mask = blocked.union(action.mask);
    if !action.nodefers() {
        mask.insert(signal);
    }
    mask.without_uncatchable()
}

/// Picks the next signal that requires work and discards ignored ones.
///
/// `pending` is mutated: signals whose disposition ignores them are removed, as
/// Linux does when it dequeues a signal. The returned signal is left pending;
/// the delivery loop clears it once the frame is set up (or the process exits).
/// `SIGKILL` and `SIGSTOP` win over everything and are never blocked.
pub fn next_deliverable(
    pending: &mut SigSet,
    blocked: SigSet,
    actions: &[SigAction; NSIG],
) -> Option<Signal> {
    if pending.contains(Signal::KILL) {
        return Some(Signal::KILL);
    }
    if pending.contains(Signal::STOP) {
        return Some(Signal::STOP);
    }
    loop {
        let signal = pending.difference(blocked).lowest()?;
        let action = actions[signal.index()];
        let ignored = action.is_ignored()
            || (action.is_default()
                && matches!(
                    default_action(signal),
                    DefaultAction::Ignore | DefaultAction::Stop
                ));
        if ignored {
            pending.remove(signal);
            continue;
        }
        return Some(signal);
    }
}

/// Non-mutating variant used by interruptible blocking syscalls.
pub fn next_interrupting(
    pending: SigSet,
    blocked: SigSet,
    actions: &[SigAction; NSIG],
) -> Option<Signal> {
    let mut copy = pending;
    next_deliverable(&mut copy, blocked, actions)
}

/// `siginfo_t` subset passed to `SA_SIGINFO` handlers.
///
/// `addr` is a union: the fault address for hardware signals, or
/// `pid | (status << 32)` for `SIGCHLD`.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Siginfo {
    pub signo: i32,
    pub errno: i32,
    pub code: i32,
    pub _pad: i32,
    pub addr: u64,
}

pub const SI_USER: i32 = 0;
pub const SI_KERNEL: i32 = 0x80;
pub const SI_QUEUE: i32 = -1;
pub const SI_TKILL: i32 = -6;
pub const SEGV_MAPERR: i32 = 1;
pub const SEGV_ACCERR: i32 = 2;
pub const FPE_INTDIV: i32 = 1;
pub const TRAP_BRKPT: i32 = 1;
pub const TRAP_TRACE: i32 = 2;
pub const ILL_ILLOPC: i32 = 2;
pub const BUS_ADRALN: i32 = 1;
pub const CLD_EXITED: i32 = 1;
pub const CLD_KILLED: i32 = 2;
pub const CLD_DUMPED: i32 = 3;

impl Siginfo {
    /// Zeroed info, used as the slot value for signals that are not pending.
    pub const EMPTY: Self = Self {
        signo: 0,
        errno: 0,
        code: 0,
        _pad: 0,
        addr: 0,
    };

    const fn new(signal: Signal, code: i32, addr: u64) -> Self {
        Self {
            signo: signal.number() as i32,
            errno: 0,
            code,
            _pad: 0,
            addr,
        }
    }

    /// `kill(2)`-style delivery.
    pub const fn user(signal: Signal) -> Self {
        Self::new(signal, SI_USER, 0)
    }

    /// Kernel-generated signal (faults, termination defaults).
    pub const fn kernel(signal: Signal, code: i32, addr: u64) -> Self {
        Self::new(signal, code, addr)
    }

    /// `SIGCHLD` payload: child pid and wait status in one 64-bit word.
    pub const fn child(pid: u32, code: i32, status: i32) -> Self {
        Self::new(
            Signal::CHLD,
            code,
            pid as u64 | ((status as u32 as u64) << 32),
        )
    }

    pub const fn child_pid(self) -> u32 {
        self.addr as u32
    }

    pub const fn child_status(self) -> i32 {
        (self.addr >> 32) as i32
    }
}

/// Saved user register state (the Rinux equivalent of `ucontext`'s
/// `mcontext`).
///
/// The field order mirrors `SyscallFrame`'s first twenty fields exactly
/// (`r15..rax`, then `rip`, `cs`, `rflags`, `rsp`, `ss`) and `TrapFrame`'s
/// shared prefix, so step 3 can copy contexts without reordering. Rinux is
/// soft-float only, so there is no FP/XSAVE state.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SigContext {
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

impl SigContext {
    pub const ZERO: Self = Self {
        r15: 0,
        r14: 0,
        r13: 0,
        r12: 0,
        r11: 0,
        r10: 0,
        r9: 0,
        r8: 0,
        rbp: 0,
        rdi: 0,
        rsi: 0,
        rdx: 0,
        rcx: 0,
        rbx: 0,
        rax: 0,
        rip: 0,
        cs: 0,
        rflags: 0,
        rsp: 0,
        ss: 0,
    };
}

/// Frame stretched onto the user stack before entering a handler.
///
/// `[frame + 0]` is the handler's return address; a `ret` pops it, so the
/// trampoline starts with `rsp == frame + 8`. `sys_sigreturn` therefore finds
/// the frame at `user_rsp - 8`.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SignalFrame {
    pub restorer: u64,
    pub context: SigContext,
    pub saved_mask: u64,
    pub signo: u64,
    pub info: Siginfo,
    pub magic: u64,
}

/// `"SIGNALFR"`, lets `sigreturn` reject a frame it did not write.
pub const SIGNAL_FRAME_MAGIC: u64 = 0x5349_474E_414C_4652;

pub const SIGNAL_FRAME_SIZE: usize = size_of::<SignalFrame>();

/// Offsets the delivery path passes to a `SA_SIGINFO` handler (`rsi`/`rdx`).
pub const SIGNAL_FRAME_CONTEXT_OFFSET: usize = core::mem::offset_of!(SignalFrame, context);
pub const SIGNAL_FRAME_INFO_OFFSET: usize = core::mem::offset_of!(SignalFrame, info);

const _: () = assert!(SIGNAL_FRAME_SIZE == 216);
const _: () = assert!(size_of::<SigContext>() == 160);
const _: () = assert!(size_of::<Siginfo>() == 24);
const _: () = assert!(size_of::<SigSet>() == 8);
const _: () = assert!(SIGNAL_FRAME_CONTEXT_OFFSET == 8);
const _: () = assert!(SIGNAL_FRAME_INFO_OFFSET == 184);

impl SignalFrame {
    pub const fn new(
        restorer: u64,
        context: SigContext,
        saved_mask: SigSet,
        signal: Signal,
        info: Siginfo,
    ) -> Self {
        Self {
            restorer,
            context,
            saved_mask: saved_mask.bits(),
            signo: signal.number() as u64,
            info,
            magic: SIGNAL_FRAME_MAGIC,
        }
    }

    /// Structural validation only: the frame lives in writable user memory, so
    /// a handler may legitimately edit it. `sigreturn` can only ever restore
    /// user state.
    pub const fn is_valid(&self) -> bool {
        self.magic == SIGNAL_FRAME_MAGIC && self.signo >= 1 && self.signo <= NSIG as u64
    }

    pub fn signal(&self) -> Option<Signal> {
        if self.magic != SIGNAL_FRAME_MAGIC {
            return None;
        }
        Signal::from_u8(self.signo as u8)
    }

    /// # Safety
    ///
    /// `address` must be a writable, mapped user address with room for
    /// `SIGNAL_FRAME_SIZE` bytes; callers use [`frame_fits`] first.
    pub unsafe fn write_to(self, address: u64) {
        core::ptr::write(address as *mut Self, self);
    }

    /// # Safety
    ///
    /// `address` must be a readable, mapped user address with room for
    /// `SIGNAL_FRAME_SIZE` bytes; callers use [`locate_frame`] first.
    pub unsafe fn read_from(address: u64) -> Self {
        core::ptr::read(address as *const Self)
    }
}

/// Bounds of the user stack window a frame may live in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StackRange {
    pub base: u64,
    pub top: u64,
}

/// Whether a `SignalFrame` at `address` lies entirely inside `stack`.
pub fn frame_fits(address: u64, stack: StackRange) -> bool {
    if address % 8 != 0 {
        return false;
    }
    match address.checked_add(SIGNAL_FRAME_SIZE as u64) {
        Some(end) => address >= stack.base && end <= stack.top,
        None => false,
    }
}

/// New handler `rsp` for an interruption that happened at `old_rsp`.
///
/// Leaves the ABI red zone untouched and returns an address with
/// `rsp % 16 == 8`, the alignment a handler sees after a normal `call`.
/// Returns `None` when the frame does not fit, which callers turn into the
/// signal's default action.
pub fn place_frame(old_rsp: u64, stack: StackRange) -> Option<u64> {
    if stack.base >= stack.top {
        return None;
    }
    let base = old_rsp
        .checked_sub(FRAME_RED_ZONE + SIGNAL_FRAME_SIZE as u64)?
        & !0xF;
    let new_rsp = base.checked_sub(size_of::<u64>() as u64)?;
    let lowest = stack.base.checked_add(FRAME_STACK_MARGIN)?;
    if new_rsp < lowest {
        return None;
    }
    if !frame_fits(new_rsp, stack) {
        return None;
    }
    Some(new_rsp)
}

/// Frame address implied by the `rsp` seen by `sigreturn`.
///
/// The trampoline runs with `rsp == frame + 8` because the handler's `ret`
/// popped the restorer, so `sigreturn` subtracts one word. The `% 16 == 8`
/// check enforces the [`place_frame`] invariant.
pub fn locate_frame(user_rsp: u64, stack: StackRange) -> Option<u64> {
    let address = user_rsp.checked_sub(size_of::<u64>() as u64)?;
    if address % 16 != 8 {
        return None;
    }
    if !frame_fits(address, stack) {
        return None;
    }
    Some(address)
}

/// RFLAGS bits a user context is allowed to control: CF, PF, AF, ZF, SF, TF,
/// DF and OF.
pub const USER_RFLAGS_MASK: u64 = 0x0DD5;
/// Bit 1 is architecturally fixed at one.
pub const RFLAGS_RESERVED: u64 = 1 << 1;
pub const RFLAGS_INTERRUPT: u64 = 1 << 9;

/// Cleans a context's flags before `sysretq`/`iretq`.
///
/// IOPL, NT, RF, VM and AC are dropped; the reserved bit and IF are forced so a
/// crafted `sigreturn` frame cannot run a task with interrupts disabled forever
/// or with ring-0 privileges.
pub fn sanitize_rflags(flags: u64) -> u64 {
    (flags & USER_RFLAGS_MASK) | RFLAGS_RESERVED | RFLAGS_INTERRUPT
}
