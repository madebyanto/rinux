#![allow(dead_code)]

use super::structs::u32_at;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlockPath {
    Direct(usize),
    Singly(u64),
    Doubly(u64, u64),
    Triply(u64, u64, u64),
    OutOfRange,
}

pub fn block_path(logical: u64, pointers: u64) -> BlockPath {
    let singly_end = 12 + pointers;
    let doubly_end = singly_end + pointers * pointers;
    let triply_end = doubly_end + pointers * pointers * pointers;
    if logical < 12 {
        BlockPath::Direct(logical as usize)
    } else if logical < singly_end {
        BlockPath::Singly(logical - 12)
    } else if logical < doubly_end {
        let index = logical - singly_end;
        BlockPath::Doubly(index / pointers, index % pointers)
    } else if logical < triply_end {
        let index = logical - doubly_end;
        BlockPath::Triply(
            index / (pointers * pointers),
            (index / pointers) % pointers,
            index % pointers,
        )
    } else {
        BlockPath::OutOfRange
    }
}

pub(crate) fn read_pointer(data: &[u8], index: usize) -> u32 {
    u32_at(data, index * 4)
}

pub(crate) fn write_pointer(data: &mut [u8], index: usize, block: u32) {
    data[index * 4..index * 4 + 4].copy_from_slice(&block.to_le_bytes());
}
