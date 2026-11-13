use super::frame::FRAME_SIZE;
use super::{allocate_frame, deallocate_frame};
use crate::arch::x86_64::cpu;
use core::ptr::{copy_nonoverlapping, read_volatile, write_bytes, write_volatile};
use core::sync::atomic::{AtomicUsize, Ordering};

pub const PRESENT: u64 = 1 << 0;
pub const WRITABLE: u64 = 1 << 1;
pub const USER: u64 = 1 << 2;
pub const NO_EXECUTE: u64 = 1 << 63;
pub const USER_PML4_INDEX: usize = 1;
const HUGE_PAGE: u64 = 1 << 7;
const ADDRESS_MASK: u64 = 0x000F_FFFF_FFFF_F000;
const ENTRY_COUNT: usize = 512;
const ENTRY_SIZE: usize = 8;

static KERNEL_ROOT: AtomicUsize = AtomicUsize::new(0);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MapError {
    Misaligned,
    AlreadyMapped,
    NotMapped,
    HugePageConflict,
    OutOfFrames,
    NotInitialized,
}

pub fn init_kernel_root() {
    KERNEL_ROOT.store(active_root(), Ordering::SeqCst);
}

pub fn kernel_root() -> usize {
    KERNEL_ROOT.load(Ordering::SeqCst)
}

pub fn active_root() -> usize {
    (cpu::read_cr3() & ADDRESS_MASK) as usize
}

pub fn map(virt: usize, phys: usize, flags: u64) -> Result<(), MapError> {
    map_in(active_root(), virt, phys, flags)
}

/// Maps a 4 KiB page in the given P4 root.
///
/// The caller must ensure the root is not being mutated concurrently.
pub fn map_in(root: usize, virt: usize, phys: usize, flags: u64) -> Result<(), MapError> {
    if virt % FRAME_SIZE != 0 || phys % FRAME_SIZE != 0 {
        return Err(MapError::Misaligned);
    }
    let mut table = root;
    for level in [4u32, 3, 2] {
        let slot = entry_address(table, table_index(virt, level));
        let entry = read_entry(slot);
        table = if (entry & PRESENT) == 0 {
            let child = new_table().ok_or(MapError::OutOfFrames)?;
            write_entry(slot, child as u64 | PRESENT | WRITABLE | (flags & USER));
            child
        } else if (entry & HUGE_PAGE) != 0 {
            return Err(MapError::HugePageConflict);
        } else {
            (entry & ADDRESS_MASK) as usize
        };
    }
    let slot = entry_address(table, table_index(virt, 1));
    if (read_entry(slot) & PRESENT) != 0 {
        return Err(MapError::AlreadyMapped);
    }
    write_entry(slot, phys as u64 | flags | PRESENT);
    if root == active_root() {
        cpu::invlpg(virt);
    }
    Ok(())
}

pub fn unmap(virt: usize) -> Result<usize, MapError> {
    unmap_in(active_root(), virt)
}

/// Removes a 4 KiB page mapping from the given P4 root.
pub fn unmap_in(root: usize, virt: usize) -> Result<usize, MapError> {
    let slot = leaf_slot_in(root, virt)?;
    let entry = read_entry(slot);
    if (entry & PRESENT) == 0 {
        return Err(MapError::NotMapped);
    }
    write_entry(slot, 0);
    if root == active_root() {
        cpu::invlpg(virt);
    }
    Ok((entry & ADDRESS_MASK) as usize)
}

pub fn translate_page(virt: usize) -> Option<usize> {
    translate_in(active_root(), virt)
}

pub fn translate_in(root: usize, virt: usize) -> Option<usize> {
    let slot = leaf_slot_in(root, virt).ok()?;
    let entry = read_entry(slot);
    if (entry & PRESENT) == 0 {
        return None;
    }
    Some((entry & ADDRESS_MASK) as usize + virt % FRAME_SIZE)
}

fn leaf_slot_in(root: usize, virt: usize) -> Result<usize, MapError> {
    let mut table = root;
    for level in [4u32, 3, 2] {
        let entry = read_entry(entry_address(table, table_index(virt, level)));
        if (entry & PRESENT) == 0 {
            return Err(MapError::NotMapped);
        }
        if (entry & HUGE_PAGE) != 0 {
            return Err(MapError::HugePageConflict);
        }
        table = (entry & ADDRESS_MASK) as usize;
    }
    Ok(entry_address(table, table_index(virt, 1)))
}

/// Creates a P4 root by copying every kernel PML4 entry except the user entry.
///
/// This is the only supported root-creation path; `copy_kernel_entries` is
/// private so the kernel template cannot diverge.
pub(super) fn create_kernel_root() -> Result<usize, MapError> {
    let kernel = kernel_root();
    if kernel == 0 {
        return Err(MapError::NotInitialized);
    }
    let root = new_table().ok_or(MapError::OutOfFrames)?;
    copy_kernel_entries(root);
    Ok(root)
}

fn copy_kernel_entries(child: usize) {
    let kernel = kernel_root();
    for index in 0..ENTRY_COUNT {
        if index == USER_PML4_INDEX {
            continue;
        }
        let value = read_entry(entry_address(kernel, index));
        write_entry(entry_address(child, index), value);
    }
}

/// Frees the user PML4 subtree and its leaf frames, leaving kernel entries alone.
pub(super) fn free_user_subtree(root: usize) {
    let user_entry = read_entry(entry_address(root, USER_PML4_INDEX));
    if (user_entry & PRESENT) == 0 {
        return;
    }
    let p3 = (user_entry & ADDRESS_MASK) as usize;
    for p3_index in 0..ENTRY_COUNT {
        let p3_entry = read_entry(entry_address(p3, p3_index));
        if (p3_entry & PRESENT) == 0 || (p3_entry & HUGE_PAGE) != 0 {
            continue;
        }
        let p2 = (p3_entry & ADDRESS_MASK) as usize;
        for p2_index in 0..ENTRY_COUNT {
            let p2_entry = read_entry(entry_address(p2, p2_index));
            if (p2_entry & PRESENT) == 0 || (p2_entry & HUGE_PAGE) != 0 {
                continue;
            }
            let p1 = (p2_entry & ADDRESS_MASK) as usize;
            for p1_index in 0..ENTRY_COUNT {
                let leaf = read_entry(entry_address(p1, p1_index));
                if (leaf & PRESENT) != 0 {
                    let _ = deallocate_frame((leaf & ADDRESS_MASK) as usize);
                }
            }
            let _ = deallocate_frame(p1);
        }
        let _ = deallocate_frame(p2);
    }
    let _ = deallocate_frame(p3);
    write_entry(entry_address(root, USER_PML4_INDEX), 0);
}

pub fn assert_no_nx_bits() {
    let root = kernel_root();
    assert!(root != 0);
    for p4_index in 0..ENTRY_COUNT {
        let p4_entry = read_entry(entry_address(root, p4_index));
        if (p4_entry & PRESENT) == 0 {
            continue;
        }
        assert!((p4_entry & NO_EXECUTE) == 0);
        assert!((p4_entry & HUGE_PAGE) == 0);
        let p3 = (p4_entry & ADDRESS_MASK) as usize;
        for p3_index in 0..ENTRY_COUNT {
            let p3_entry = read_entry(entry_address(p3, p3_index));
            if (p3_entry & PRESENT) == 0 {
                continue;
            }
            assert!((p3_entry & NO_EXECUTE) == 0);
            if (p3_entry & HUGE_PAGE) != 0 {
                continue;
            }
            let p2 = (p3_entry & ADDRESS_MASK) as usize;
            for p2_index in 0..ENTRY_COUNT {
                let p2_entry = read_entry(entry_address(p2, p2_index));
                if (p2_entry & PRESENT) == 0 {
                    continue;
                }
                assert!((p2_entry & NO_EXECUTE) == 0);
                if (p2_entry & HUGE_PAGE) != 0 {
                    continue;
                }
                let p1 = (p2_entry & ADDRESS_MASK) as usize;
                for p1_index in 0..ENTRY_COUNT {
                    let leaf = read_entry(entry_address(p1, p1_index));
                    if (leaf & PRESENT) != 0 {
                        assert!((leaf & NO_EXECUTE) == 0);
                    }
                }
            }
        }
    }
}

pub fn clone_user_mappings(parent: usize, child: usize) -> Result<(), MapError> {
    let p4_entry = read_entry(entry_address(parent, USER_PML4_INDEX));
    if (p4_entry & PRESENT) == 0 {
        return Ok(());
    }
    if (p4_entry & HUGE_PAGE) != 0 {
        return Err(MapError::HugePageConflict);
    }
    let p3 = (p4_entry & ADDRESS_MASK) as usize;
    for p3_index in 0..ENTRY_COUNT {
        let p3_entry = read_entry(entry_address(p3, p3_index));
        if (p3_entry & PRESENT) == 0 {
            continue;
        }
        if (p3_entry & HUGE_PAGE) != 0 {
            return Err(MapError::HugePageConflict);
        }
        let p2 = (p3_entry & ADDRESS_MASK) as usize;
        for p2_index in 0..ENTRY_COUNT {
            let p2_entry = read_entry(entry_address(p2, p2_index));
            if (p2_entry & PRESENT) == 0 {
                continue;
            }
            if (p2_entry & HUGE_PAGE) != 0 {
                return Err(MapError::HugePageConflict);
            }
            let p1 = (p2_entry & ADDRESS_MASK) as usize;
            for p1_index in 0..ENTRY_COUNT {
                let leaf = read_entry(entry_address(p1, p1_index));
                if (leaf & PRESENT) == 0 {
                    continue;
                }
                let parent_phys = (leaf & ADDRESS_MASK) as usize;
                let child_phys = allocate_frame().ok_or(MapError::OutOfFrames)?;
                unsafe {
                    copy_nonoverlapping(
                        parent_phys as *const u8,
                        child_phys as *mut u8,
                        FRAME_SIZE,
                    );
                }
                let virt = (USER_PML4_INDEX << 39)
                    | (p3_index << 30)
                    | (p2_index << 21)
                    | (p1_index << 12);
                let flags = (leaf & (WRITABLE | NO_EXECUTE)) | USER;
                if let Err(error) = map_in(child, virt, child_phys, flags) {
                    let _ = deallocate_frame(child_phys);
                    return Err(error);
                }
            }
        }
    }
    Ok(())
}

fn table_index(virt: usize, level: u32) -> usize {
    (virt >> (12 + 9 * (level - 1))) & (ENTRY_COUNT - 1)
}

fn entry_address(table: usize, index: usize) -> usize {
    table + index * ENTRY_SIZE
}

fn new_table() -> Option<usize> {
    let frame = allocate_frame()?;
    unsafe { write_bytes(frame as *mut u64, 0, ENTRY_COUNT) };
    Some(frame)
}

fn read_entry(address: usize) -> u64 {
    unsafe { read_volatile(address as *const u64) }
}

fn write_entry(address: usize, value: u64) {
    unsafe { write_volatile(address as *mut u64, value) }
}
