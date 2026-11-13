pub const FRAME_SIZE: usize = 4096;
pub const MANAGED_LIMIT: usize = 1 << 30;
pub const FRAME_COUNT: usize = MANAGED_LIMIT / FRAME_SIZE;
const WORD_BITS: usize = 64;
const WORD_COUNT: usize = FRAME_COUNT / WORD_BITS;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameError {
    Misaligned,
    OutOfRange,
    NotAllocated,
}

pub struct FrameAllocator {
    bits: [u64; WORD_COUNT],
    free: usize,
    hint: usize,
}

impl FrameAllocator {
    pub const fn new() -> Self {
        Self {
            bits: [u64::MAX; WORD_COUNT],
            free: 0,
            hint: 0,
        }
    }

    pub fn add_usable_range(&mut self, start: usize, end: usize) {
        let first = start.div_ceil(FRAME_SIZE).min(FRAME_COUNT);
        let last = (end / FRAME_SIZE).min(FRAME_COUNT);
        for frame in first..last {
            self.set_used(frame, false);
        }
    }

    pub fn reserve_range(&mut self, start: usize, end: usize) {
        let first = (start / FRAME_SIZE).min(FRAME_COUNT);
        let last = end.div_ceil(FRAME_SIZE).min(FRAME_COUNT);
        for frame in first..last {
            self.set_used(frame, true);
        }
    }

    pub fn allocate(&mut self) -> Option<usize> {
        for offset in 0..WORD_COUNT {
            let word_index = (self.hint + offset) % WORD_COUNT;
            let word = self.bits[word_index];
            if word != u64::MAX {
                let bit = (!word).trailing_zeros() as usize;
                self.bits[word_index] |= 1u64 << bit;
                self.free -= 1;
                self.hint = word_index;
                return Some((word_index * WORD_BITS + bit) * FRAME_SIZE);
            }
        }
        None
    }

    pub fn deallocate(&mut self, address: usize) -> Result<(), FrameError> {
        if address % FRAME_SIZE != 0 {
            return Err(FrameError::Misaligned);
        }
        let frame = address / FRAME_SIZE;
        if frame >= FRAME_COUNT {
            return Err(FrameError::OutOfRange);
        }
        if !self.is_used(frame) {
            return Err(FrameError::NotAllocated);
        }
        self.set_used(frame, false);
        self.hint = frame / WORD_BITS;
        Ok(())
    }

    pub fn free_frames(&self) -> usize {
        self.free
    }

    fn is_used(&self, frame: usize) -> bool {
        self.bits[frame / WORD_BITS] & (1u64 << (frame % WORD_BITS)) != 0
    }

    fn set_used(&mut self, frame: usize, used: bool) {
        let word = frame / WORD_BITS;
        let mask = 1u64 << (frame % WORD_BITS);
        let was_used = self.bits[word] & mask != 0;
        if used && !was_used {
            self.bits[word] |= mask;
            self.free -= 1;
        } else if !used && was_used {
            self.bits[word] &= !mask;
            self.free += 1;
        }
    }
}
