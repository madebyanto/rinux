pub const FRAME_SIZE: usize = 4096;
pub const MANAGED_LIMIT: usize = 1 << 30;
pub const FRAME_COUNT: usize = MANAGED_LIMIT / FRAME_SIZE;
pub const REFCOUNT_MAX: u16 = u16::MAX;
const WORD_BITS: usize = 64;
const WORD_COUNT: usize = FRAME_COUNT / WORD_BITS;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameError {
    Misaligned,
    OutOfRange,
    NotAllocated,
    RefcountOverflow,
}

pub struct FrameAllocator {
    bits: [u64; WORD_COUNT],
    refs: [u16; FRAME_COUNT],
    free: usize,
    hint: usize,
}

impl FrameAllocator {
    pub const fn new() -> Self {
        Self {
            bits: [u64::MAX; WORD_COUNT],
            refs: [0; FRAME_COUNT],
            free: 0,
            hint: 0,
        }
    }

    pub fn add_usable_range(&mut self, start: usize, end: usize) {
        let first = start.div_ceil(FRAME_SIZE).min(FRAME_COUNT);
        let last = (end / FRAME_SIZE).min(FRAME_COUNT);
        for frame in first..last {
            debug_assert_eq!(self.refs[frame], 0, "usable frame is still referenced");
            self.set_used(frame, false);
        }
    }

    pub fn reserve_range(&mut self, start: usize, end: usize) {
        let first = (start / FRAME_SIZE).min(FRAME_COUNT);
        let last = end.div_ceil(FRAME_SIZE).min(FRAME_COUNT);
        for frame in first..last {
            debug_assert_eq!(self.refs[frame], 0, "reserved frame is still referenced");
            self.set_used(frame, true);
        }
    }

    pub fn allocate(&mut self) -> Option<usize> {
        for offset in 0..WORD_COUNT {
            let word_index = (self.hint + offset) % WORD_COUNT;
            let word = self.bits[word_index];
            if word != u64::MAX {
                let bit = (!word).trailing_zeros() as usize;
                let frame = word_index * WORD_BITS + bit;
                debug_assert_eq!(self.refs[frame], 0, "free frame has references");
                self.set_used(frame, true);
                self.refs[frame] = 1;
                self.hint = word_index;
                return Some(frame * FRAME_SIZE);
            }
        }
        None
    }

    pub fn retain(&mut self, address: usize) -> Result<u16, FrameError> {
        let frame = self.frame_index(address)?;
        if !self.is_used(frame) || self.refs[frame] == 0 {
            return Err(FrameError::NotAllocated);
        }
        if self.refs[frame] == REFCOUNT_MAX {
            return Err(FrameError::RefcountOverflow);
        }
        self.refs[frame] += 1;
        Ok(self.refs[frame])
    }

    pub fn release(&mut self, address: usize) -> Result<bool, FrameError> {
        let frame = self.frame_index(address)?;
        if !self.is_used(frame) || self.refs[frame] == 0 {
            return Err(FrameError::NotAllocated);
        }
        self.refs[frame] -= 1;
        if self.refs[frame] != 0 {
            return Ok(false);
        }
        self.set_used(frame, false);
        self.hint = frame / WORD_BITS;
        Ok(true)
    }

    pub fn refcount(&self, address: usize) -> Result<u16, FrameError> {
        let frame = self.frame_index(address)?;
        if !self.is_used(frame) || self.refs[frame] == 0 {
            return Err(FrameError::NotAllocated);
        }
        Ok(self.refs[frame])
    }

    pub fn free_frames(&self) -> usize {
        self.free
    }

    fn frame_index(&self, address: usize) -> Result<usize, FrameError> {
        if address % FRAME_SIZE != 0 {
            return Err(FrameError::Misaligned);
        }
        let frame = address / FRAME_SIZE;
        if frame >= FRAME_COUNT {
            return Err(FrameError::OutOfRange);
        }
        Ok(frame)
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
