use core::mem::{offset_of, size_of};

use hosttest::signal::{
    default_action, frame_fits, handler_mask, hardware_fault_signal, locate_frame, next_deliverable,
    next_interrupting, place_frame, sanitize_rflags, signal_wait_status, DefaultAction,
    HardwareFault, SigAction, SigContext, SigSet, Siginfo, Signal, SignalFrame, StackRange,
    BUS_ADRALN, CLD_EXITED, CLD_KILLED, FPE_INTDIV, FRAME_RED_ZONE, FRAME_STACK_MARGIN,
    ILL_ILLOPC, NSIG, SA_NODEFER, SA_ONSTACK, SA_RESTART, SA_RESETHAND, SA_RESTORER, SA_SIGINFO,
    SEGV_ACCERR, SEGV_MAPERR, SIG_DFL, SIG_IGN, SIGNAL_FRAME_CONTEXT_OFFSET,
    SIGNAL_FRAME_INFO_OFFSET, SIGNAL_FRAME_MAGIC, SIGNAL_FRAME_SIZE, SI_USER, TRAMPOLINE_CODE,
    TRAP_BRKPT, TRAP_TRACE, USER_RFLAGS_MASK, VALID_SIGNAL_BITS,
};
use hosttest::uaccess::{
    trampoline_valid, PROGRAM_BASE, PROGRAM_SIZE, STACK_BASE, TRAMPOLINE_BASE, TRAMPOLINE_END,
    TRAMPOLINE_SIZE,
};

fn set(signals: &[Signal]) -> SigSet {
    let mut result = SigSet::EMPTY;
    for signal in signals {
        result.insert(*signal);
    }
    result
}

fn empty_actions() -> [SigAction; NSIG] {
    [SigAction::DEFAULT; NSIG]
}

#[test]
fn signal_numbers_and_names_match_linux() {
    let expected: [(u8, &str); NSIG] = [
        (1, "SIGHUP"),
        (2, "SIGINT"),
        (3, "SIGQUIT"),
        (4, "SIGILL"),
        (5, "SIGTRAP"),
        (6, "SIGABRT"),
        (7, "SIGBUS"),
        (8, "SIGFPE"),
        (9, "SIGKILL"),
        (10, "SIGUSR1"),
        (11, "SIGSEGV"),
        (12, "SIGUSR2"),
        (13, "SIGPIPE"),
        (14, "SIGALRM"),
        (15, "SIGTERM"),
        (16, "SIGSTKFLT"),
        (17, "SIGCHLD"),
        (18, "SIGCONT"),
        (19, "SIGSTOP"),
        (20, "SIGTSTP"),
        (21, "SIGTTIN"),
        (22, "SIGTTOU"),
        (23, "SIGURG"),
        (24, "SIGXCPU"),
        (25, "SIGXFSZ"),
        (26, "SIGVTALRM"),
        (27, "SIGPROF"),
        (28, "SIGWINCH"),
        (29, "SIGIO"),
        (30, "SIGPWR"),
        (31, "SIGSYS"),
    ];

    for (index, (number, name)) in expected.iter().enumerate() {
        let signal = Signal::from_u8(*number).expect("valid signal");
        assert_eq!(signal.number(), *number);
        assert_eq!(signal.name(), *name);
        assert_eq!(signal.index(), index);
        assert_eq!(Signal::ALL[index], signal);
    }

    assert_eq!(Signal::ALL.len(), NSIG);
    assert!(Signal::from_u8(0).is_none());
    assert!(Signal::from_u8(32).is_none());
    assert!(Signal::from_u8(u8::MAX).is_none());
}

#[test]
fn uncatchable_signals_are_only_kill_and_stop() {
    assert!(Signal::KILL.is_uncatchable());
    assert!(Signal::STOP.is_uncatchable());
    for signal in Signal::ALL {
        if signal == Signal::KILL || signal == Signal::STOP {
            continue;
        }
        assert!(!signal.is_uncatchable(), "{}", signal.name());
    }
}

#[test]
fn signal_set_uses_linux_bit_order() {
    assert_eq!(SigSet::singleton(Signal::HUP).bits(), 1);
    assert_eq!(SigSet::singleton(Signal::KILL).bits(), 1 << 8);
    assert_eq!(SigSet::singleton(Signal::SYS).bits(), 1 << 30);
    assert_eq!(VALID_SIGNAL_BITS, (1u64 << 31) - 1);

    // Bits outside 1..=31 are not signals and must be dropped.
    assert_eq!(SigSet::from_bits(1 << 31).bits(), 0);
    assert_eq!(SigSet::from_bits(u64::MAX).bits(), VALID_SIGNAL_BITS);
    assert_eq!(SigSet::from_bits((1 << 0) | (1 << 31)).bits(), 1);

    assert_eq!(SigSet::default(), SigSet::EMPTY);
    assert!(SigSet::EMPTY.is_empty());
}

#[test]
fn signal_set_supports_the_core_set_algebra() {
    let mut signals = SigSet::EMPTY;
    signals.insert(Signal::HUP);
    signals.insert(Signal::TERM);
    assert!(signals.contains(Signal::HUP));
    assert!(signals.contains(Signal::TERM));
    assert!(!signals.contains(Signal::INT));
    assert_eq!(signals.lowest(), Some(Signal::HUP));

    let other = set(&[Signal::INT, Signal::TERM]);
    assert_eq!(
        signals.union(other),
        set(&[Signal::HUP, Signal::INT, Signal::TERM])
    );
    assert_eq!(signals.intersection(other), SigSet::singleton(Signal::TERM));
    assert_eq!(
        signals.difference(other),
        SigSet::singleton(Signal::HUP)
    );

    signals.remove(Signal::HUP);
    assert_eq!(signals.lowest(), Some(Signal::TERM));
    signals.insert_all(other);
    assert_eq!(signals, set(&[Signal::INT, Signal::TERM]));
}

#[test]
fn uncatchable_signals_are_removed_from_blocked_masks() {
    let blocked = set(&[Signal::INT, Signal::KILL, Signal::STOP]);
    let sanitized = blocked.without_uncatchable();
    assert!(sanitized.contains(Signal::INT));
    assert!(!sanitized.contains(Signal::KILL));
    assert!(!sanitized.contains(Signal::STOP));
}

#[test]
fn default_actions_match_posix() {
    let core = [
        Signal::QUIT,
        Signal::ILL,
        Signal::TRAP,
        Signal::ABRT,
        Signal::BUS,
        Signal::FPE,
        Signal::SEGV,
        Signal::XCPU,
        Signal::XFSZ,
        Signal::SYS,
    ];
    let ignored = [Signal::CHLD, Signal::CONT, Signal::URG, Signal::WINCH];
    let stopped = [Signal::STOP, Signal::TSTP, Signal::TTIN, Signal::TTOU];

    for signal in Signal::ALL {
        let expected = if core.contains(&signal) {
            DefaultAction::TerminateCore
        } else if ignored.contains(&signal) {
            DefaultAction::Ignore
        } else if stopped.contains(&signal) {
            DefaultAction::Stop
        } else {
            DefaultAction::Terminate
        };
        assert_eq!(default_action(signal), expected, "{}", signal.name());
    }
}

#[test]
fn sigaction_predicates_decode_flags() {
    assert_eq!(SIG_DFL, 0);
    assert_eq!(SIG_IGN, 1);
    assert!(SigAction::DEFAULT.is_default());
    assert!(!SigAction::DEFAULT.is_ignored());
    assert!(!SigAction::DEFAULT.is_caught());
    assert!(SigAction::IGNORE.is_ignored());
    assert!(!SigAction::IGNORE.is_default());
    assert!(!SigAction::IGNORE.is_caught());

    let handler = SigAction {
        handler: 0x0000_0080_0000_1000,
        flags: SA_SIGINFO
            | SA_RESTART
            | SA_ONSTACK
            | SA_NODEFER
            | SA_RESETHAND
            | SA_RESTORER,
        restorer: 0x0000_0080_0000_2000,
        mask: SigSet::singleton(Signal::USR1),
    };
    assert!(handler.is_caught());
    assert!(!handler.is_default());
    assert!(!handler.is_ignored());
    assert!(handler.reports_info());
    assert!(handler.restarts());
    assert!(handler.uses_altstack());
    assert!(handler.nodefers());
    assert!(handler.resets_on_entry());
    assert!(handler.uses_restorer());
    assert!(handler.flags_supported());

    let unknown = SigAction {
        flags: 1 << 40,
        ..handler
    };
    assert!(!unknown.flags_supported());

    let plain = SigAction::with_handler(0x0000_0080_0000_3000);
    assert!(plain.is_caught());
    assert!(!plain.restarts());
    assert!(!plain.uses_restorer());
}

#[test]
fn next_deliverable_prefers_kill_then_stop() {
    let actions = empty_actions();

    // SIGKILL wins over any other pending signal, and blocking cannot hide it.
    let mut pending = set(&[Signal::USR1, Signal::KILL]);
    assert_eq!(
        next_deliverable(&mut pending, SigSet::EMPTY, &actions),
        Some(Signal::KILL)
    );
    assert_eq!(
        next_deliverable(&mut pending, set(&[Signal::KILL]), &actions),
        Some(Signal::KILL)
    );
    assert!(pending.contains(Signal::KILL));
    assert!(pending.contains(Signal::USR1));

    // SIGSTOP is second, also unblockable.
    let mut pending = set(&[Signal::USR1, Signal::STOP]);
    assert_eq!(
        next_deliverable(&mut pending, SigSet::EMPTY, &actions),
        Some(Signal::STOP)
    );
    assert_eq!(
        next_deliverable(&mut pending, set(&[Signal::STOP]), &actions),
        Some(Signal::STOP)
    );
}

#[test]
fn next_deliverable_skips_blocked_and_prunes_ignored() {
    let mut actions = empty_actions();
    actions[Signal::USR2.index()] = SigAction::IGNORE;

    let mut pending = set(&[Signal::USR1, Signal::USR2, Signal::TERM]);
    assert_eq!(
        next_deliverable(&mut pending, set(&[Signal::USR1]), &actions),
        Some(Signal::TERM)
    );

    // The ignored signal is discarded, the blocked one and the chosen one stay.
    assert!(!pending.contains(Signal::USR2));
    assert!(pending.contains(Signal::USR1));
    assert!(pending.contains(Signal::TERM));
}

#[test]
fn next_deliverable_prunes_default_ignored_signals() {
    let actions = empty_actions();
    let mut pending = set(&[Signal::CHLD, Signal::CONT, Signal::WINCH]);
    assert_eq!(
        next_deliverable(&mut pending, SigSet::EMPTY, &actions),
        None
    );
    assert!(pending.is_empty());

    // A default-ignored signal must not mask a real one waiting behind it.
    let mut pending = set(&[Signal::CHLD, Signal::SYS]);
    assert_eq!(
        next_deliverable(&mut pending, SigSet::EMPTY, &actions),
        Some(Signal::SYS)
    );
    assert_eq!(pending.bits(), SigSet::singleton(Signal::SYS).bits());
}

#[test]
fn next_deliverable_returns_none_when_all_are_blocked() {
    let actions = empty_actions();
    let mut pending = set(&[Signal::USR1, Signal::TERM]);
    let before = pending;
    assert_eq!(
        next_deliverable(&mut pending, set(&[Signal::USR1, Signal::TERM]), &actions),
        None
    );
    // Blocked signals remain pending even when their default action is ignore.
    assert_eq!(pending, before);
}

#[test]
fn next_interrupting_does_not_mutate_the_pending_set() {
    let actions = empty_actions();
    let pending = set(&[Signal::CHLD, Signal::USR1]);
    assert_eq!(
        next_interrupting(pending, SigSet::EMPTY, &actions),
        Some(Signal::USR1)
    );
    assert_eq!(pending.bits(), set(&[Signal::CHLD, Signal::USR1]).bits());

    // Ignored-only sets do not interrupt a blocking syscall.
    let pending = set(&[Signal::CHLD, Signal::WINCH]);
    assert_eq!(next_interrupting(pending, SigSet::EMPTY, &actions), None);
}

#[test]
fn handler_mask_composes_blocked_sa_mask_and_the_signal() {
    let blocked = set(&[Signal::INT, Signal::TERM]);
    let action = SigAction {
        mask: set(&[Signal::USR1, Signal::KILL]),
        ..SigAction::with_handler(0x0000_0080_0000_1000)
    };

    let mask = handler_mask(blocked, action, Signal::TERM);
    assert!(mask.contains(Signal::INT));
    assert!(mask.contains(Signal::TERM));
    assert!(mask.contains(Signal::USR1));
    assert!(!mask.contains(Signal::KILL));

    let nodefer = SigAction {
        flags: SA_NODEFER,
        ..action
    };
    let mask = handler_mask(SigSet::EMPTY, nodefer, Signal::TERM);
    assert!(!mask.contains(Signal::TERM));
    assert!(mask.contains(Signal::USR1));
}

#[test]
fn siginfo_constructors_encode_the_payload() {
    let user = Siginfo::user(Signal::USR1);
    assert_eq!(user.signo, 10);
    assert_eq!(user.errno, 0);
    assert_eq!(user.code, SI_USER);
    assert_eq!(user.addr, 0);

    let fault = Siginfo::kernel(Signal::SEGV, SEGV_MAPERR, 0xDEAD_BEEF);
    assert_eq!(fault.signo, 11);
    assert_eq!(fault.code, SEGV_MAPERR);
    assert_eq!(fault.addr, 0xDEAD_BEEF);

    let exited = Siginfo::child(42, CLD_EXITED, 7);
    assert_eq!(exited.signo, 17);
    assert_eq!(exited.code, CLD_EXITED);
    assert_eq!(exited.child_pid(), 42);
    assert_eq!(exited.child_status(), 7);

    let killed = Siginfo::child(7, CLD_KILLED, 9);
    assert_eq!(killed.child_pid(), 7);
    assert_eq!(killed.child_status(), 9);

    assert_eq!(Siginfo::EMPTY.signo, 0);
    assert_eq!(Siginfo::EMPTY.code, 0);
    assert_eq!(Siginfo::EMPTY.addr, 0);
}

#[test]
fn hardware_faults_map_to_posix_signals_and_si_codes() {
    let (signal, info) = hardware_fault_signal(HardwareFault::DivideError, 0x1000);
    assert_eq!(signal, Signal::FPE);
    assert_eq!(info.signo, 8);
    assert_eq!(info.code, FPE_INTDIV);
    assert_eq!(info.addr, 0);

    let (signal, info) = hardware_fault_signal(HardwareFault::Debug, 0x2000);
    assert_eq!(signal, Signal::TRAP);
    assert_eq!(info.code, TRAP_TRACE);
    assert_eq!(info.addr, 0x2000);

    let (signal, info) = hardware_fault_signal(HardwareFault::Breakpoint, 0x3000);
    assert_eq!(signal, Signal::TRAP);
    assert_eq!(info.code, TRAP_BRKPT);
    assert_eq!(info.addr, 0x3000);

    let (signal, info) = hardware_fault_signal(HardwareFault::InvalidOpcode, 0x4000);
    assert_eq!(signal, Signal::ILL);
    assert_eq!(info.code, ILL_ILLOPC);
    assert_eq!(info.addr, 0x4000);

    let (signal, info) = hardware_fault_signal(HardwareFault::AlignmentCheck, 0x5000);
    assert_eq!(signal, Signal::BUS);
    assert_eq!(info.code, BUS_ADRALN);
    assert_eq!(info.addr, 0);

    let (signal, info) = hardware_fault_signal(
        HardwareFault::PageFault {
            present: false,
            address: 0xDEAD_0000,
        },
        0x6000,
    );
    assert_eq!(signal, Signal::SEGV);
    assert_eq!(info.signo, 11);
    assert_eq!(info.code, SEGV_MAPERR);
    assert_eq!(info.addr, 0xDEAD_0000);

    let (signal, info) = hardware_fault_signal(
        HardwareFault::PageFault {
            present: true,
            address: 0xBEEF_0000,
        },
        0x7000,
    );
    assert_eq!(signal, Signal::SEGV);
    assert_eq!(info.code, SEGV_ACCERR);
    assert_eq!(info.addr, 0xBEEF_0000);
}

#[test]
fn wait_status_encodes_termination_signal_and_core_flag() {
    // Normal exits keep the low byte clear.
    assert_eq!(signal_wait_status(Signal::TERM), 15);
    // Signals whose default action terminates without a core dump.
    for signal in [Signal::HUP, Signal::INT, Signal::KILL, Signal::PIPE] {
        assert_eq!(signal_wait_status(signal), signal.number() as i32);
    }
    // TerminateCore signals set bit 7 (WCOREDUMP).
    for signal in [
        Signal::QUIT,
        Signal::ILL,
        Signal::TRAP,
        Signal::ABRT,
        Signal::BUS,
        Signal::FPE,
        Signal::SEGV,
    ] {
        assert_eq!(
            signal_wait_status(signal),
            signal.number() as i32 | 0x80
        );
    }
}

#[test]
fn next_interrupting_honours_dispositions_and_the_blocked_mask() {
    let mut actions = empty_actions();
    actions[Signal::USR1.index()] = SigAction::with_handler(0x0000_0080_0000_1000);
    actions[Signal::USR2.index()] = SigAction::IGNORE;

    // Default-ignored `SIGCHLD` and an explicitly ignored `SIGUSR2` never
    // interrupt a blocking syscall; the caught `SIGUSR1` does.
    let pending = set(&[Signal::CHLD, Signal::USR2, Signal::USR1]);
    assert_eq!(
        next_interrupting(pending, SigSet::EMPTY, &actions),
        Some(Signal::USR1)
    );

    // A blocked caught signal is skipped and only ignored signals remain.
    let pending = set(&[Signal::CHLD, Signal::USR1]);
    assert_eq!(
        next_interrupting(pending, set(&[Signal::USR1]), &actions),
        None
    );
}

#[test]
fn sigcontext_layout_matches_the_syscall_frame() {
    assert_eq!(size_of::<SigContext>(), 160);
    assert_eq!(offset_of!(SigContext, r15), 0);
    assert_eq!(offset_of!(SigContext, r14), 8);
    assert_eq!(offset_of!(SigContext, rbp), 64);
    assert_eq!(offset_of!(SigContext, rdi), 72);
    assert_eq!(offset_of!(SigContext, rax), 112);
    assert_eq!(offset_of!(SigContext, rip), 120);
    assert_eq!(offset_of!(SigContext, cs), 128);
    assert_eq!(offset_of!(SigContext, rflags), 136);
    assert_eq!(offset_of!(SigContext, rsp), 144);
    assert_eq!(offset_of!(SigContext, ss), 152);
}

#[test]
fn signal_frame_layout_is_stable() {
    assert_eq!(size_of::<SignalFrame>(), 216);
    assert_eq!(SIGNAL_FRAME_SIZE, 216);
    assert_eq!(core::mem::align_of::<SignalFrame>(), 8);
    assert_eq!(size_of::<Siginfo>(), 24);
    assert_eq!(size_of::<SigSet>(), 8);

    assert_eq!(offset_of!(SignalFrame, restorer), 0);
    assert_eq!(offset_of!(SignalFrame, context), 8);
    assert_eq!(offset_of!(SignalFrame, saved_mask), 168);
    assert_eq!(offset_of!(SignalFrame, signo), 176);
    assert_eq!(offset_of!(SignalFrame, info), 184);
    assert_eq!(offset_of!(SignalFrame, magic), 208);
    assert_eq!(SIGNAL_FRAME_CONTEXT_OFFSET, 8);
    assert_eq!(SIGNAL_FRAME_INFO_OFFSET, 184);
}

#[test]
fn signal_frame_round_trips_through_memory() {
    let mut context = SigContext::ZERO;
    context.r15 = 0x1111;
    context.rdi = 0x2222;
    context.rax = 0x3333;
    context.rip = 0x0000_0080_0000_1234;
    context.cs = 0x33;
    context.rflags = 0x202;
    context.rsp = 0x0000_0080_001F_FF00;
    context.ss = 0x2B;

    let mask = set(&[Signal::INT, Signal::TERM]);
    let info = Siginfo::user(Signal::USR1);
    let frame = SignalFrame::new(
        TRAMPOLINE_BASE as u64,
        context,
        mask,
        Signal::USR1,
        info,
    );
    assert!(frame.is_valid());
    assert_eq!(frame.signal(), Some(Signal::USR1));

    let mut memory = [0u64; SIGNAL_FRAME_SIZE / size_of::<u64>()];
    unsafe { frame.write_to(memory.as_mut_ptr() as u64) };
    let read = unsafe { SignalFrame::read_from(memory.as_ptr() as u64) };

    assert_eq!(read, frame);
    assert_eq!(read.saved_mask, mask.bits());
    assert_eq!(read.context.rip, context.rip);
    assert_eq!(read.info, info);
    assert_eq!(read.magic, SIGNAL_FRAME_MAGIC);

    let mut broken = frame;
    broken.magic ^= 1;
    assert!(!broken.is_valid());
    assert_eq!(broken.signal(), None);
}

#[test]
fn place_frame_keeps_alignment_red_zone_and_bounds() {
    let stack = StackRange {
        base: 0x0000_0080_001C_0000,
        top: 0x0000_0080_0020_0000,
    };

    for delta in [0, 1, 8, 16, 0x40, 0x400, 0x2000] {
        let old_rsp = stack.top - delta;
        let new_rsp = place_frame(old_rsp, stack).expect("frame fits");
        assert_eq!(new_rsp % 16, 8, "handler entry alignment");
        assert!(new_rsp >= stack.base + FRAME_STACK_MARGIN);
        assert!(frame_fits(new_rsp, stack));
        assert!(new_rsp + SIGNAL_FRAME_SIZE as u64 <= old_rsp - FRAME_RED_ZONE);
        assert_eq!(locate_frame(new_rsp + 8, stack), Some(new_rsp));
    }
}

#[test]
fn place_frame_rejects_exhausted_or_inverted_stacks() {
    let tiny = StackRange {
        base: 0,
        top: 0x100,
    };
    assert_eq!(place_frame(0x100, tiny), None);
    assert_eq!(place_frame(0, tiny), None);
    assert_eq!(
        place_frame(0x2000, StackRange { base: 0x3000, top: 0x2000 }),
        None
    );

    // Exactly at the margin: the aligned frame would start below base + 64.
    let tight = StackRange {
        base: 0x1000,
        top: 0x1000 + FRAME_RED_ZONE + SIGNAL_FRAME_SIZE as u64 + FRAME_STACK_MARGIN,
    };
    assert_eq!(place_frame(tight.top, tight), None);

    // A roomier stack with the same base still fits.
    let roomy = StackRange {
        base: 0x1000,
        top: tight.top + 0x100,
    };
    assert!(place_frame(roomy.top, roomy).is_some());
}

#[test]
fn locate_frame_requires_the_trampoline_stack_pointer() {
    let stack = StackRange {
        base: 0x0000_0080_001C_0000,
        top: 0x0000_0080_0020_0000,
    };
    let new_rsp = place_frame(stack.top, stack).expect("frame fits");

    assert_eq!(locate_frame(new_rsp + 8, stack), Some(new_rsp));
    // The handler's ret popped one word, so user_rsp-8 must be 16-aligned + 8.
    assert_eq!(locate_frame(new_rsp + 16, stack), None);
    assert_eq!(locate_frame(new_rsp + 9, stack), None);
    assert_eq!(locate_frame(0, stack), None);
    assert_eq!(locate_frame(stack.top + 8, stack), None);

    let elsewhere = StackRange {
        base: 0,
        top: 0x1000,
    };
    assert_eq!(locate_frame(new_rsp + 8, elsewhere), None);
}

#[test]
fn sanitize_rflags_drops_privileged_bits() {
    use hosttest::signal::{RFLAGS_INTERRUPT, RFLAGS_RESERVED};

    // Nothing set still yields the reserved bit and IF.
    assert_eq!(
        sanitize_rflags(0),
        RFLAGS_RESERVED | RFLAGS_INTERRUPT
    );
    assert_eq!(
        sanitize_rflags(0),
        0x202,
        "sysretq target flags for a fresh handler"
    );

    // Arithmetic/string flags survive; IOPL, NT, RF, VM and AC do not.
    let privileged = (3 << 12) | (1 << 14) | (1 << 16) | (1 << 17) | (1 << 18);
    assert_eq!(sanitize_rflags(privileged), 0x202);
    assert_eq!(
        sanitize_rflags(USER_RFLAGS_MASK),
        USER_RFLAGS_MASK | 0x202
    );
    assert_eq!(sanitize_rflags(u64::MAX), 0xFD7);
    assert_eq!(sanitize_rflags(1 << 8), 0x302); // TF preserved

    let sanitized = sanitize_rflags(1 << 9);
    assert_eq!(sanitized & (1 << 9), 1 << 9);
    assert_eq!(sanitized & (3 << 12), 0);
    assert_eq!(sanitized & (1 << 14), 0);
    assert_eq!(sanitized & (1 << 18), 0);
    assert_eq!(sanitized & 0x2, 0x2);
}

#[test]
fn trampoline_page_is_well_formed_and_isolated() {
    // mov rax, 15
    assert_eq!(&TRAMPOLINE_CODE[..3], &[0x48, 0xC7, 0xC0]);
    let immediate = u32::from_le_bytes([
        TRAMPOLINE_CODE[3],
        TRAMPOLINE_CODE[4],
        TRAMPOLINE_CODE[5],
        TRAMPOLINE_CODE[6],
    ]);
    assert_eq!(immediate, 15, "rt_sigreturn syscall number");
    // syscall
    assert_eq!(&TRAMPOLINE_CODE[7..9], &[0x0F, 0x05]);
    // ud2: a bug, never a fall-through
    assert_eq!(&TRAMPOLINE_CODE[9..11], &[0x0F, 0x0B]);

    assert_eq!(TRAMPOLINE_BASE % 4096, 0, "page aligned");
    assert_eq!(TRAMPOLINE_BASE >> 39, 1, "user PML4 slot");
    assert_eq!(TRAMPOLINE_SIZE, 4096);
    assert_eq!(TRAMPOLINE_END, TRAMPOLINE_BASE + TRAMPOLINE_SIZE);
    assert!(TRAMPOLINE_SIZE >= TRAMPOLINE_CODE.len());
    assert!(trampoline_valid(TRAMPOLINE_BASE, TRAMPOLINE_CODE.len()));
    assert!(!trampoline_valid(TRAMPOLINE_END, 1));
    assert!(!trampoline_valid(TRAMPOLINE_BASE - 1, 1));
    let program_end = (PROGRAM_BASE + PROGRAM_SIZE) as u64;
    assert!(TRAMPOLINE_BASE as u64 >= program_end);
    assert!(TRAMPOLINE_END as u64 <= STACK_BASE as u64);
    assert_ne!(TRAMPOLINE_BASE, PROGRAM_BASE);
}
