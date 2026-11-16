//! Fixed user virtual address layout and range validation.
//!
//! The user half currently uses a single PML4 slot: a 64 KiB program window,
//! a sigreturn trampoline page one MiB above it, and a 16 KiB stack below
//! `0x0000_0080_0020_0000`.

pub const PROGRAM_BASE: usize = 0x0000_0080_0000_0000;
pub const PROGRAM_PAGES: usize = 16;
pub const PROGRAM_SIZE: usize = PROGRAM_PAGES * 4096;

/// Kernel-provided `rt_sigreturn` trampoline.
///
/// `binfmt_elf::build_image` maps this page read-only/executable into every
/// user address space. Fork inherits it through the normal user-mapping clone;
/// execve recreates it because it builds a fresh address space.
pub const TRAMPOLINE_BASE: usize = 0x0000_0080_0010_0000;
pub const TRAMPOLINE_PAGES: usize = 1;
pub const TRAMPOLINE_SIZE: usize = TRAMPOLINE_PAGES * 4096;
pub const TRAMPOLINE_END: usize = TRAMPOLINE_BASE + TRAMPOLINE_SIZE;

pub const STACK_TOP: usize = 0x0000_0080_0020_0000;
pub const STACK_PAGES: usize = 4;
pub const STACK_SIZE: usize = STACK_PAGES * 4096;
pub const STACK_BASE: usize = STACK_TOP - STACK_SIZE;

pub fn program_valid(address: usize, length: usize) -> bool {
    let Some(end) = address.checked_add(length) else {
        return false;
    };
    address >= PROGRAM_BASE && end <= PROGRAM_BASE + PROGRAM_SIZE
}

pub fn trampoline_valid(address: usize, length: usize) -> bool {
    let Some(end) = address.checked_add(length) else {
        return false;
    };
    address >= TRAMPOLINE_BASE && end <= TRAMPOLINE_END
}

pub fn stack_valid(address: usize, length: usize) -> bool {
    let Some(end) = address.checked_add(length) else {
        return false;
    };
    address >= STACK_BASE && end <= STACK_TOP
}

/// User buffers may only name the program data window or the stack; the
/// trampoline is executable by ring 3 but is never a syscall buffer.
pub fn range_valid(address: usize, length: usize) -> bool {
    program_valid(address, length) || stack_valid(address, length)
}

pub const PML4_SHIFT: usize = 39;
pub const USER_PML4_INDEX: usize = 1;

pub const fn pml4_index(address: usize) -> usize {
    address >> PML4_SHIFT
}

const _: () = assert!(pml4_index(PROGRAM_BASE) == USER_PML4_INDEX);
const _: () = assert!(pml4_index(TRAMPOLINE_BASE) == USER_PML4_INDEX);
const _: () = assert!(pml4_index(STACK_TOP - 1) == USER_PML4_INDEX);
const _: () = assert!(TRAMPOLINE_SIZE == 4096);
const _: () = assert!(TRAMPOLINE_BASE >= PROGRAM_BASE + PROGRAM_SIZE);
const _: () = assert!(TRAMPOLINE_END <= STACK_BASE);
