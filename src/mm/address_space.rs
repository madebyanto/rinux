use super::deallocate_frame;
use super::paging;
use crate::arch::x86_64::cpu;

const PML4_SHIFT: usize = 39;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AddressSpaceError {
    OutOfMemory,
    UserRange,
    MapFailed,
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
        let _ = deallocate_frame(self.root);
    }
}
