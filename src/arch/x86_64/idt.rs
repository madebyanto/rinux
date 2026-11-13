use super::cpu;
use super::pic;
use crate::sync::SpinLock;
use core::arch::asm;
use core::cell::UnsafeCell;
use core::mem::size_of;

core::arch::global_asm!(include_str!("interrupt.S"));

pub const EXCEPTION_COUNT: usize = 32;
pub const IRQ_SLOTS: usize = 16;

const STUB_COUNT: usize = EXCEPTION_COUNT + IRQ_SLOTS;
const STUB_TABLE_LEN: usize = STUB_COUNT + 1;
const IDT_ENTRY_COUNT: usize = 256;
const KERNEL_CODE_SELECTOR: u16 = 0x08;
const INTERRUPT_GATE: u8 = 0x8E;
const PAGE_FAULT: u64 = 14;

extern "C" {
    #[link_name = "isr_stub_table"]
    static ISR_STUB_TABLE: [u64; STUB_TABLE_LEN];
}

pub type IrqHandler = fn();

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IrqError {
    InvalidIrq(u8),
    AlreadyRegistered(u8),
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct TrapFrame {
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
    pub vector: u64,
    pub error_code: u64,
    pub rip: u64,
    pub cs: u64,
    pub rflags: u64,
    pub rsp: u64,
    pub ss: u64,
}

impl TrapFrame {
    fn dump(&self) {
        crate::kprintln!(
            "RIP: {:#018x} CS: {:#06x} RFLAGS: {:#018x}",
            self.rip,
            self.cs,
            self.rflags
        );
        crate::kprintln!("RSP: {:#018x} SS: {:#06x}", self.rsp, self.ss);
        crate::kprintln!(
            "RAX: {:#018x} RBX: {:#018x} RCX: {:#018x}",
            self.rax,
            self.rbx,
            self.rcx
        );
        crate::kprintln!(
            "RDX: {:#018x} RSI: {:#018x} RDI: {:#018x}",
            self.rdx,
            self.rsi,
            self.rdi
        );
        crate::kprintln!(
            "RBP: {:#018x} R8:  {:#018x} R9:  {:#018x}",
            self.rbp,
            self.r8,
            self.r9
        );
        crate::kprintln!(
            "R10: {:#018x} R11: {:#018x} R12: {:#018x}",
            self.r10,
            self.r11,
            self.r12
        );
        crate::kprintln!(
            "R13: {:#018x} R14: {:#018x} R15: {:#018x}",
            self.r13,
            self.r14,
            self.r15
        );
    }
}

#[derive(Clone, Copy)]
#[repr(C)]
struct IdtEntry {
    offset_low: u16,
    selector: u16,
    ist: u8,
    type_attr: u8,
    offset_mid: u16,
    offset_high: u32,
    reserved: u32,
}

impl IdtEntry {
    const fn absent() -> Self {
        Self {
            offset_low: 0,
            selector: 0,
            ist: 0,
            type_attr: 0,
            offset_mid: 0,
            offset_high: 0,
            reserved: 0,
        }
    }

    fn new(handler: u64) -> Self {
        Self {
            offset_low: handler as u16,
            selector: KERNEL_CODE_SELECTOR,
            ist: 0,
            type_attr: INTERRUPT_GATE,
            offset_mid: (handler >> 16) as u16,
            offset_high: (handler >> 32) as u32,
            reserved: 0,
        }
    }
}

struct Idt {
    entries: [IdtEntry; IDT_ENTRY_COUNT],
}

const EMPTY_IDT: Idt = Idt {
    entries: [IdtEntry::absent(); IDT_ENTRY_COUNT],
};

struct IdtStorage(UnsafeCell<Idt>);

unsafe impl Sync for IdtStorage {}

static IDT: IdtStorage = IdtStorage(UnsafeCell::new(EMPTY_IDT));

static IRQ_HANDLERS: SpinLock<[Option<IrqHandler>; IRQ_SLOTS]> = SpinLock::new([None; IRQ_SLOTS]);

#[repr(C, packed)]
struct DescriptorTablePointer {
    limit: u16,
    base: u64,
}

pub fn init() {
    let idt = unsafe { &mut *IDT.0.get() };
    for (vector, entry) in idt.entries.iter_mut().enumerate() {
        let handler = unsafe {
            if vector < STUB_COUNT {
                ISR_STUB_TABLE[vector]
            } else {
                ISR_STUB_TABLE[STUB_COUNT]
            }
        };
        *entry = IdtEntry::new(handler);
    }
    let descriptor = DescriptorTablePointer {
        limit: (size_of::<Idt>() - 1) as u16,
        base: idt as *const Idt as u64,
    };
    unsafe {
        asm!("lidt [{}]", in(reg) &descriptor, options(nostack, preserves_flags));
    }
}

pub fn register_irq(irq: u8, handler: IrqHandler) -> Result<(), IrqError> {
    if irq as usize >= IRQ_SLOTS {
        return Err(IrqError::InvalidIrq(irq));
    }
    cpu::without_interrupts(|| {
        let mut handlers = IRQ_HANDLERS.lock();
        if handlers[irq as usize].is_some() {
            return Err(IrqError::AlreadyRegistered(irq));
        }
        handlers[irq as usize] = Some(handler);
        Ok(())
    })
}

#[no_mangle]
pub extern "C" fn interrupt_dispatch(frame: *mut TrapFrame) {
    let frame = unsafe { &mut *frame };
    match frame.vector {
        0..=31 => handle_exception(frame),
        32..=47 => handle_irq((frame.vector - pic::VECTOR_BASE as u64) as u8),
        _ => handle_unexpected(frame),
    }
}

fn handle_exception(frame: &TrapFrame) -> ! {
    cpu::disable_interrupts();
    let vector = frame.vector as u8;
    crate::kprintln!();
    crate::kprintln!(
        "EXCEPTION: {} ({}) vector={}",
        exception_name(vector),
        exception_mnemonic(vector),
        vector
    );
    if has_error_code(vector) {
        crate::kprintln!("error code: {:#018x}", frame.error_code);
    }
    if frame.vector == PAGE_FAULT {
        crate::kprintln!("CR2: {:#018x}", cpu::read_cr2());
    }
    frame.dump();
    crate::kprintln!("system halted");
    cpu::halt_forever()
}

fn handle_unexpected(frame: &TrapFrame) -> ! {
    cpu::disable_interrupts();
    crate::kprintln!();
    crate::kprintln!("UNEXPECTED INTERRUPT vector={}", frame.vector);
    frame.dump();
    crate::kprintln!("system halted");
    cpu::halt_forever()
}

fn handle_irq(irq: u8) {
    let handler = IRQ_HANDLERS.lock()[irq as usize];
    if let Some(handler) = handler {
        handler();
    }
    let _ = pic::eoi(irq);
}

pub fn has_error_code(vector: u8) -> bool {
    matches!(vector, 8 | 10 | 11 | 12 | 13 | 14 | 17 | 21 | 29 | 30)
}

pub fn exception_name(vector: u8) -> &'static str {
    match vector {
        0 => "DIVIDE ERROR",
        1 => "DEBUG",
        2 => "NON-MASKABLE INTERRUPT",
        3 => "BREAKPOINT",
        4 => "OVERFLOW",
        5 => "BOUND RANGE EXCEEDED",
        6 => "INVALID OPCODE",
        7 => "DEVICE NOT AVAILABLE",
        8 => "DOUBLE FAULT",
        9 => "COPROCESSOR SEGMENT OVERRUN",
        10 => "INVALID TSS",
        11 => "SEGMENT NOT PRESENT",
        12 => "STACK-SEGMENT FAULT",
        13 => "GENERAL PROTECTION FAULT",
        14 => "PAGE FAULT",
        16 => "x87 FLOATING-POINT EXCEPTION",
        17 => "ALIGNMENT CHECK",
        18 => "MACHINE CHECK",
        19 => "SIMD FLOATING-POINT EXCEPTION",
        20 => "VIRTUALIZATION EXCEPTION",
        21 => "CONTROL PROTECTION EXCEPTION",
        28 => "HYPERVISOR INJECTION EXCEPTION",
        29 => "VMM COMMUNICATION EXCEPTION",
        30 => "SECURITY EXCEPTION",
        _ => "RESERVED",
    }
}

pub fn exception_mnemonic(vector: u8) -> &'static str {
    match vector {
        0 => "#DE",
        1 => "#DB",
        2 => "#NMI",
        3 => "#BP",
        4 => "#OF",
        5 => "#BR",
        6 => "#UD",
        7 => "#NM",
        8 => "#DF",
        9 => "#CSO",
        10 => "#TS",
        11 => "#NP",
        12 => "#SS",
        13 => "#GP",
        14 => "#PF",
        16 => "#MF",
        17 => "#AC",
        18 => "#MC",
        19 => "#XM",
        20 => "#VE",
        21 => "#CP",
        28 => "#HV",
        29 => "#VC",
        30 => "#SX",
        _ => "#??",
    }
}
