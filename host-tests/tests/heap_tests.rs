use hosttest::heap::{Heap, PAGE_SIZE};
use std::alloc::{alloc_zeroed, dealloc, Layout};

const HEAP_SIZE: usize = 4 * 1024 * 1024;

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
}

fn make_heap() -> (Heap, usize, Layout) {
    let layout = Layout::from_size_align(HEAP_SIZE, PAGE_SIZE).unwrap();
    let base = unsafe { alloc_zeroed(layout) } as usize;
    assert_ne!(base, 0);
    let mut heap = Heap::new();
    unsafe { heap.init(base, HEAP_SIZE) };
    (heap, base, layout)
}

#[test]
fn alignment_bounds_and_reuse() {
    let (mut heap, base, layout) = make_heap();
    let cases = [
        (1usize, 1usize),
        (8, 8),
        (24, 8),
        (33, 16),
        (100, 64),
        (500, 512),
        (2048, 2048),
        (2049, 8),
        (4096, 4096),
        (10_000, 8),
        (1 << 20, 4096),
    ];
    for (size, align) in cases {
        let block = Layout::from_size_align(size, align).unwrap();
        let pointer = unsafe { heap.alloc(block) };
        assert!(!pointer.is_null(), "size {size} align {align}");
        assert_eq!(pointer as usize % align, 0, "size {size} align {align}");
        let address = pointer as usize;
        assert!(address >= base && address + size <= base + HEAP_SIZE);
        unsafe {
            std::ptr::write_bytes(pointer, 0xAB, size);
            heap.dealloc(pointer, block);
        }
    }
    let small = Layout::from_size_align(64, 8).unwrap();
    let first = unsafe { heap.alloc(small) };
    unsafe { heap.dealloc(first, small) };
    let again = unsafe { heap.alloc(small) };
    assert_eq!(first, again, "freed block should be reused");
    unsafe { dealloc(base as *mut u8, layout) };
}

#[test]
fn exhaustion_returns_null() {
    let (mut heap, base, layout) = make_heap();
    let block = Layout::from_size_align(4096, 8).unwrap();
    let mut count = 0usize;
    loop {
        let pointer = unsafe { heap.alloc(block) };
        if pointer.is_null() {
            break;
        }
        count += 1;
    }
    assert!(count > 500, "only {count} pages before exhaustion");
    unsafe { dealloc(base as *mut u8, layout) };
}

#[test]
fn random_stress_keeps_contents_intact() {
    let (mut heap, base, layout) = make_heap();
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    let mut live: Vec<(*mut u8, Layout, u8)> = Vec::new();
    let mut failures = 0usize;
    let mut attempts = 0usize;
    for step in 0..20_000u32 {
        let roll = rng.next() % 100;
        if (roll < 55 && live.len() < 300) || live.is_empty() {
            let size = match rng.next() % 4 {
                0 => 1 + (rng.next() as usize) % 64,
                1 => 1 + (rng.next() as usize) % 512,
                2 => 1 + (rng.next() as usize) % 2048,
                _ => 1 + (rng.next() as usize) % 16384,
            };
            let align = 1usize << (rng.next() % 7);
            let block = Layout::from_size_align(size, align).unwrap();
            attempts += 1;
            let pointer = unsafe { heap.alloc(block) };
            if pointer.is_null() {
                failures += 1;
                continue;
            }
            assert_eq!(pointer as usize % align, 0);
            let pattern = (step % 251) as u8 + 1;
            unsafe { std::ptr::write_bytes(pointer, pattern, size) };
            live.push((pointer, block, pattern));
        } else {
            let index = (rng.next() as usize) % live.len();
            let (pointer, block, pattern) = live.swap_remove(index);
            let bytes = unsafe { std::slice::from_raw_parts(pointer, block.size()) };
            assert!(bytes.iter().all(|&b| b == pattern), "corruption at step {step}");
            unsafe { heap.dealloc(pointer, block) };
        }
    }
    for (pointer, block, pattern) in live.drain(..) {
        let bytes = unsafe { std::slice::from_raw_parts(pointer, block.size()) };
        assert!(bytes.iter().all(|&b| b == pattern));
        unsafe { heap.dealloc(pointer, block) };
    }
    println!("stress: {failures} failures out of {attempts} allocations");
    assert!(failures * 50 < attempts.max(1), "too many failures: {failures}/{attempts}");
    unsafe { dealloc(base as *mut u8, layout) };
}
