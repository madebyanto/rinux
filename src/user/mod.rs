pub mod range;

use crate::mm;
use crate::proc::process::{free_user_pages, UserPage};
use alloc::vec::Vec;
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
    pub pages: Vec<UserPage>,
}

pub fn blob_len() -> usize {
    BLOB.len()
}

pub fn init() -> Result<UserImage, UserError> {
    if BLOB.len() > PROGRAM_SIZE {
        return Err(UserError::ProgramTooLarge);
    }

    let mut pages = Vec::new();
    for page in 0..range::PROGRAM_PAGES {
        let virt = PROGRAM_BASE + page * PAGE_SIZE;
        match map_page(virt, mm::paging::USER) {
            Ok(user_page) => pages.push(user_page),
            Err(error) => {
                free_user_pages(&mut pages);
                return Err(error);
            }
        }
    }

    let mut copied = 0;
    for page in &pages {
        if copied >= BLOB.len() {
            break;
        }
        let count = (BLOB.len() - copied).min(PAGE_SIZE);
        unsafe {
            copy_nonoverlapping(BLOB.as_ptr().add(copied), page.phys as *mut u8, count);
        }
        copied += count;
    }

    for page in 0..range::STACK_PAGES {
        let virt = STACK_BASE + page * PAGE_SIZE;
        match map_page(virt, mm::paging::USER | mm::paging::WRITABLE) {
            Ok(user_page) => pages.push(user_page),
            Err(error) => {
                free_user_pages(&mut pages);
                return Err(error);
            }
        }
    }

    Ok(UserImage {
        entry: PROGRAM_BASE as u64,
        stack_top: (STACK_TOP - 16) as u64,
        pages,
    })
}

fn map_page(virt: usize, flags: u64) -> Result<UserPage, UserError> {
    let phys = mm::allocate_frame().ok_or(UserError::OutOfMemory)?;
    unsafe { write_bytes(phys as *mut u8, 0, PAGE_SIZE) };
    if mm::paging::map(virt, phys, flags).is_err() {
        let _ = mm::deallocate_frame(phys);
        return Err(UserError::MapFailed);
    }
    Ok(UserPage { virt, phys })
}
