#![allow(dead_code)]

use alloc::vec;
use alloc::vec::Vec;

/// An ext2 block or inode bitmap.
///
/// Ext2 stores bitmaps little-endian at the bit level: bit `n` lives in
/// `data[n / 8]` at `1 << (n % 8)`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Bitmap {
    data: Vec<u8>,
}

impl Bitmap {
    pub fn new(bytes: usize) -> Self {
        Self {
            data: vec![0u8; bytes],
        }
    }

    pub fn from_bytes(data: Vec<u8>) -> Self {
        Self { data }
    }

    pub fn as_bytes(&self) -> &[u8] {
        &self.data
    }

    pub fn as_mut_bytes(&mut self) -> &mut [u8] {
        &mut self.data
    }

    pub fn len_bits(&self) -> u32 {
        (self.data.len() * 8) as u32
    }

    pub fn is_set(&self, bit: u32) -> bool {
        let index = bit as usize / 8;
        match self.data.get(index) {
            Some(byte) => byte & (1u8 << (bit % 8)) != 0,
            None => false,
        }
    }

    pub fn set(&mut self, bit: u32) {
        if let Some(byte) = self.data.get_mut(bit as usize / 8) {
            *byte |= 1u8 << (bit % 8);
        }
    }

    pub fn clear(&mut self, bit: u32) {
        if let Some(byte) = self.data.get_mut(bit as usize / 8) {
            *byte &= !(1u8 << (bit % 8));
        }
    }

    /// First clear bit in `min_bit..valid_bits`.
    pub fn first_free(&self, valid_bits: u32, min_bit: u32) -> Option<u32> {
        let start = min_bit.min(valid_bits);
        (start..valid_bits).find(|bit| !self.is_set(*bit))
    }

    pub fn count_free(&self, valid_bits: u32) -> u32 {
        (0..valid_bits).filter(|bit| !self.is_set(*bit)).count() as u32
    }
}
