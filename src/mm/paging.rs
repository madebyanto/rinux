use super::allocate_frame;
use super::frame::FRAME_SIZE;
use crate::arch::x86_64::cpu;
use core::ptr::{read_volatile, write_bytes, write_volatile};

pub const PRESENT: u64 = 1 << 0;
pub const WRITABLE: u64 = 1 << 1;
const HUGE_PAGE: u64 = 1 << 7;
const ADDRESS_MASK: u64 = 0x000F_FFFF_FFFF_F000;
const ENTRY_COUNT: usize = 512;
const ENTRY_SIZE: usize = 8;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MapError {
    Misaligned,
    AlreadyMapped,
    NotMapped,
    HugePageConflict,
    OutOfFrames,
}

pub fn map(virt: usize, phys: usize, flags: u64) -> Result<(), MapError> {
    if virt % FRAME_SIZE != 0 || phys % FRAME_SIZE != 0 {
        return Err(MapError::Misaligned);
    }
    let mut table = active_root();
    for level in [4u32, 3, 2] {
        let slot = entry_address(table, table_index(virt, level));
        let entry = read_entry(slot);
        table = if (entry & PRESENT) == 0 {
            let child = new_table().ok_or(MapError::OutOfFrames)?;
            write_entry(slot, child as u64 | PRESENT | WRITABLE);
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
    cpu::invlpg(virt);
    Ok(())
}

pub fn unmap(virt: usize) -> Result<usize, MapError> {
    let slot = leaf_slot(virt)?;
    let entry = read_entry(slot);
    if (entry & PRESENT) == 0 {
        return Err(MapError::NotMapped);
    }
    write_entry(slot, 0);
    cpu::invlpg(virt);
    Ok((entry & ADDRESS_MASK) as usize)
}

pub fn translate_page(virt: usize) -> Option<usize> {
    let slot = leaf_slot(virt).ok()?;
    let entry = read_entry(slot);
    if (entry & PRESENT) == 0 {
        return None;
    }
    Some((entry & ADDRESS_MASK) as usize + virt % FRAME_SIZE)
}

fn leaf_slot(virt: usize) -> Result<usize, MapError> {
    let mut table = active_root();
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

fn active_root() -> usize {
    (cpu::read_cr3() & ADDRESS_MASK) as usize
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
