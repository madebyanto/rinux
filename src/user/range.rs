pub const PROGRAM_BASE: usize = 0x0000_0080_0000_0000;
pub const PROGRAM_PAGES: usize = 16;
pub const PROGRAM_SIZE: usize = PROGRAM_PAGES * 4096;

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

pub fn stack_valid(address: usize, length: usize) -> bool {
    let Some(end) = address.checked_add(length) else {
        return false;
    };
    address >= STACK_BASE && end <= STACK_TOP
}

pub fn range_valid(address: usize, length: usize) -> bool {
    program_valid(address, length) || stack_valid(address, length)
}

pub const PML4_SHIFT: usize = 39;
pub const USER_PML4_INDEX: usize = 1;

pub const fn pml4_index(address: usize) -> usize {
    address >> PML4_SHIFT
}

const _: () = assert!(pml4_index(PROGRAM_BASE) == USER_PML4_INDEX);
const _: () = assert!(pml4_index(STACK_TOP - 1) == USER_PML4_INDEX);
