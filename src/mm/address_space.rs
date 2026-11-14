use super::frame::{FrameError, FRAME_SIZE};
use super::pte::{frame_address, make_writable, validate_cow_leaf, with_frame, CowLeafError};
use super::{allocate_frame, frame_refcount, paging, release_frame};
use crate::arch::x86_64::cpu;
use core::ptr::copy_nonoverlapping;

const PML4_SHIFT: usize = 39;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AddressSpaceError {
    OutOfMemory,
    UserRange,
    MapFailed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CowError {
    Leaf(CowLeafError),
    Frame(FrameError),
    OutOfMemory,
}

pub struct AddressSpace {
    root: usize,
}

impl AddressSpace {
    pub fn new() -> Result<Self, AddressSpaceError> {
        let root = paging::create_kernel_root().map_err(|_| AddressSpaceError::OutOfMemory)?;
        Ok(Self { root })
    }

    pub fn root(&self) -> usize {
        self.root
    }

    pub fn clone_user(&self) -> Result<Self, AddressSpaceError> {
        let root = paging::create_kernel_root().map_err(|_| AddressSpaceError::OutOfMemory)?;
        let child = Self { root };
        paging::clone_user_mappings(self.root, child.root)
            .map_err(|_| AddressSpaceError::MapFailed)?;
        Ok(child)
    }

    /// Resolves a write fault on a copy-on-write leaf in this address space.
    ///
    /// A last remaining reference is simply promoted to writable. A shared
    /// frame is copied into a fresh frame, the old reference is dropped, and
    /// the leaf is rewritten to point at the private copy.
    pub fn handle_cow_fault(&mut self, virt: usize) -> Result<(), CowError> {
        let slot = paging::leaf_slot_in(self.root, virt)
            .map_err(|_| CowError::Leaf(CowLeafError::NotPresent))?;
        let entry = paging::read_entry(slot);
        validate_cow_leaf(entry).map_err(CowError::Leaf)?;
        let old_phys = frame_address(entry);
        let refcount = frame_refcount(old_phys).map_err(CowError::Frame)?;
        let new_entry = if refcount > 1 {
            let new_phys = allocate_frame().ok_or(CowError::OutOfMemory)?;
            unsafe {
                copy_nonoverlapping(old_phys as *const u8, new_phys as *mut u8, FRAME_SIZE);
            }
            if let Err(error) = release_frame(old_phys) {
                let _ = release_frame(new_phys);
                return Err(CowError::Frame(error));
            }
            make_writable(with_frame(entry, new_phys))
        } else {
            make_writable(entry)
        };
        paging::write_entry(slot, new_entry);
        if self.root == paging::active_root() {
            cpu::invlpg(virt);
        }
        Ok(())
    }

    pub fn map_user(
        &mut self,
        virt: usize,
        phys: usize,
        flags: u64,
    ) -> Result<(), AddressSpaceError> {
        if virt >> PML4_SHIFT != paging::USER_PML4_INDEX {
            return Err(AddressSpaceError::UserRange);
        }
        paging::map_in(self.root, virt, phys, flags | paging::USER)
            .map_err(|_| AddressSpaceError::MapFailed)
    }
}

impl Drop for AddressSpace {
    fn drop(&mut self) {
        if paging::active_root() == self.root {
            cpu::write_cr3(paging::kernel_root());
        }
        paging::free_user_subtree(self.root);
        let _ = release_frame(self.root);
    }
}
