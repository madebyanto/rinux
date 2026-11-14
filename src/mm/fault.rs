// Consumed by the page-fault handler in Step 3.
#![allow(dead_code)]

pub const FAULT_PRESENT: u64 = 1 << 0;
pub const FAULT_WRITE: u64 = 1 << 1;
pub const FAULT_USER: u64 = 1 << 2;
pub const FAULT_RESERVED: u64 = 1 << 3;
pub const FAULT_INSTRUCTION: u64 = 1 << 4;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PageFaultAccess {
    Read,
    Write,
    InstructionFetch,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PageFaultOrigin {
    User,
    Kernel,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PageFaultError {
    bits: u64,
}

impl PageFaultError {
    pub const fn new(bits: u64) -> Self {
        Self { bits }
    }

    pub const fn bits(self) -> u64 {
        self.bits
    }

    pub const fn present(self) -> bool {
        self.bits & FAULT_PRESENT != 0
    }

    pub const fn write(self) -> bool {
        self.bits & FAULT_WRITE != 0
    }

    pub const fn user(self) -> bool {
        self.bits & FAULT_USER != 0
    }

    pub const fn reserved(self) -> bool {
        self.bits & FAULT_RESERVED != 0
    }

    pub const fn instruction_fetch(self) -> bool {
        self.bits & FAULT_INSTRUCTION != 0
    }

    pub const fn access(self) -> PageFaultAccess {
        if self.instruction_fetch() {
            PageFaultAccess::InstructionFetch
        } else if self.write() {
            PageFaultAccess::Write
        } else {
            PageFaultAccess::Read
        }
    }

    pub const fn origin(self) -> PageFaultOrigin {
        if self.user() {
            PageFaultOrigin::User
        } else {
            PageFaultOrigin::Kernel
        }
    }

    pub const fn is_cow_candidate(self) -> bool {
        self.present()
            && self.write()
            && !self.reserved()
            && !self.instruction_fetch()
    }
}
