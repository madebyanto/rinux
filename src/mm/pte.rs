#![allow(dead_code)]

pub const PRESENT: u64 = 1 << 0;
pub const WRITABLE: u64 = 1 << 1;
pub const USER: u64 = 1 << 2;
pub const COW_BIT: u64 = 9;
pub const COW: u64 = 1 << COW_BIT;
pub const NO_EXECUTE: u64 = 1 << 63;
pub const ADDRESS_MASK: u64 = 0x000F_FFFF_FFFF_F000;

pub const fn is_present(entry: u64) -> bool {
    entry & PRESENT != 0
}

pub const fn is_writable(entry: u64) -> bool {
    entry & WRITABLE != 0
}

pub const fn is_user(entry: u64) -> bool {
    entry & USER != 0
}

pub const fn is_cow(entry: u64) -> bool {
    entry & COW != 0
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CowLeafError {
    NotPresent,
    NotUser,
    NotCow,
    Writable,
}

/// Checks whether a leaf entry is a page that a COW write fault can resolve.
pub const fn validate_cow_leaf(entry: u64) -> Result<(), CowLeafError> {
    if !is_present(entry) {
        Err(CowLeafError::NotPresent)
    } else if !is_user(entry) {
        Err(CowLeafError::NotUser)
    } else if !is_cow(entry) {
        Err(CowLeafError::NotCow)
    } else if is_writable(entry) {
        Err(CowLeafError::Writable)
    } else {
        Ok(())
    }
}

pub const fn make_cow(entry: u64) -> u64 {
    (entry & !WRITABLE) | COW
}

pub const fn make_writable(entry: u64) -> u64 {
    (entry | WRITABLE) & !COW
}

pub const fn child_mapping_flags(entry: u64) -> u64 {
    let mut flags = entry & (USER | NO_EXECUTE);
    if is_writable(entry) || is_cow(entry) {
        flags |= COW;
    }
    flags
}

pub const fn frame_address(entry: u64) -> usize {
    (entry & ADDRESS_MASK) as usize
}

pub const fn with_frame(entry: u64, address: usize) -> u64 {
    (entry & !ADDRESS_MASK) | (address as u64 & ADDRESS_MASK)
}
