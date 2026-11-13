pub mod range;

use crate::mm;
use crate::mm::address_space::AddressSpace;
use core::ptr::{copy_nonoverlapping, write_bytes};

pub use range::{range_valid, stack_valid, PROGRAM_BASE, PROGRAM_SIZE, STACK_BASE, STACK_TOP};

static BLOB: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/hello.bin"));

const PAGE_SIZE: usize = 4096;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UserError {
    ProgramTooLarge,
    OutOfMemory,
    MapFailed,
}

pub struct UserImage {
    pub entry: u64,
    pub stack_top: u64,
    pub address_space: AddressSpace,
}

pub fn blob_len() -> usize {
    BLOB.len()
}

pub fn init() -> Result<UserImage, UserError> {
    if BLOB.len() > PROGRAM_SIZE {
        return Err(UserError::ProgramTooLarge);
    }
    let mut address_space = AddressSpace::new().map_err(|_| UserError::OutOfMemory)?;
    let mut copied = 0;
    for page in 0..range::PROGRAM_PAGES {
        let virt = PROGRAM_BASE + page * PAGE_SIZE;
        let phys = mm::allocate_frame().ok_or(UserError::OutOfMemory)?;
        unsafe { write_bytes(phys as *mut u8, 0, PAGE_SIZE) };
        if copied < BLOB.len() {
            let count = (BLOB.len() - copied).min(PAGE_SIZE);
            unsafe {
                copy_nonoverlapping(BLOB.as_ptr().add(copied), phys as *mut u8, count);
            }
            copied += count;
        }
        if address_space
            .map_user(virt, phys, mm::paging::USER)
            .is_err()
        {
            let _ = mm::deallocate_frame(phys);
            return Err(UserError::MapFailed);
        }
    }
    for page in 0..range::STACK_PAGES {
        let virt = STACK_BASE + page * PAGE_SIZE;
        let phys = mm::allocate_frame().ok_or(UserError::OutOfMemory)?;
        unsafe { write_bytes(phys as *mut u8, 0, PAGE_SIZE) };
        if address_space
            .map_user(virt, phys, mm::paging::USER | mm::paging::WRITABLE)
            .is_err()
        {
            let _ = mm::deallocate_frame(phys);
            return Err(UserError::MapFailed);
        }
    }
    Ok(UserImage {
        entry: PROGRAM_BASE as u64,
        stack_top: (STACK_TOP - 16) as u64,
        address_space,
    })
}
