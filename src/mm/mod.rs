pub mod address_space;
pub mod frame;
pub mod heap;
pub mod paging;

use crate::arch::x86_64::multiboot2::BootInfo;
use crate::sync::SpinLock;
use alloc::boxed::Box;
use alloc::vec;
use alloc::vec::Vec;
use core::alloc::{GlobalAlloc, Layout};
use core::ptr::{addr_of, read_volatile, write_volatile};
use frame::{FrameAllocator, FrameError, FRAME_COUNT, FRAME_SIZE};
use heap::Heap;

extern "C" {
    static __kernel_start: u8;
    static __kernel_end: u8;
}

pub const HEAP_START: usize = 0x4444_0000_0000;
pub const HEAP_SIZE: usize = 4 * 1024 * 1024;
const LOW_MEMORY_END: usize = 0x10_0000;
const SELF_TEST_PAGE: usize = 0x5555_0000_0000;

static FRAMES: SpinLock<FrameAllocator> = SpinLock::new(FrameAllocator::new());
static HEAP: SpinLock<Heap> = SpinLock::new(Heap::new());

struct KernelAllocator;

unsafe impl GlobalAlloc for KernelAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        HEAP.lock().alloc(layout)
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        HEAP.lock().dealloc(ptr, layout)
    }
}

#[global_allocator]
static KERNEL_ALLOCATOR: KernelAllocator = KernelAllocator;

pub fn init(boot: &BootInfo) {
    paging::init_kernel_root();
    let kernel_start = addr_of!(__kernel_start) as usize;
    let kernel_end = addr_of!(__kernel_end) as usize;
    let (info_start, info_end) = boot.info_range();
    {
        let mut frames = FRAMES.lock();
        for region in boot.usable_regions() {
            let start = region.start.max(LOW_MEMORY_END);
            if start < region.end {
                frames.add_usable_range(start, region.end);
            }
        }
        frames.reserve_range(kernel_start, kernel_end);
        frames.reserve_range(info_start, info_end);
        kprintln!(
            "frames: {} free of {} managed (first 1 GiB)",
            frames.free_frames(),
            FRAME_COUNT
        );
    }
    for page in 0..HEAP_SIZE / FRAME_SIZE {
        let phys = allocate_frame().expect("no frame available for the kernel heap");
        paging::map(HEAP_START + page * FRAME_SIZE, phys, paging::WRITABLE)
            .expect("kernel heap mapping failed");
    }
    unsafe { HEAP.lock().init(HEAP_START, HEAP_SIZE) };
    kprintln!("kernel heap: {} KiB at {:#x}", HEAP_SIZE / 1024, HEAP_START);
}

pub fn allocate_frame() -> Option<usize> {
    FRAMES.lock().allocate()
}

pub fn deallocate_frame(address: usize) -> Result<(), FrameError> {
    FRAMES.lock().deallocate(address)
}

pub fn self_test() {
    let first = allocate_frame().expect("frame allocation failed");
    let second = allocate_frame().expect("frame allocation failed");
    assert!(first != second && first % FRAME_SIZE == 0);
    deallocate_frame(first).expect("frame release failed");
    deallocate_frame(second).expect("frame release failed");
    assert!(deallocate_frame(second) == Err(FrameError::NotAllocated));

    let boxed = Box::new(0x5249_4E55_5820_u64);
    assert!(*boxed == 0x5249_4E55_5820);

    let mut values: Vec<u32> = Vec::new();
    for value in 0..1000 {
        values.push(value);
    }
    let sum: u64 = values.iter().map(|&value| value as u64).sum();
    assert!(sum == 499_500);

    let large = vec![0xA5u8; 256 * 1024];
    assert!(large[large.len() - 1] == 0xA5);
    drop(large);
    drop(values);
    drop(boxed);
    kprintln!("heap self-test: ok");

    let phys = allocate_frame().expect("frame allocation failed");
    paging::map(SELF_TEST_PAGE, phys, paging::WRITABLE).expect("self-test mapping failed");
    unsafe { write_volatile(SELF_TEST_PAGE as *mut u64, 0x1234_5678_9ABC_DEF0) };
    let readback = unsafe { read_volatile(SELF_TEST_PAGE as *const u64) };
    assert!(readback == 0x1234_5678_9ABC_DEF0);
    assert!(paging::translate_page(SELF_TEST_PAGE) == Some(phys));
    assert!(paging::unmap(SELF_TEST_PAGE) == Ok(phys));
    assert!(paging::translate_page(SELF_TEST_PAGE).is_none());
    deallocate_frame(phys).expect("frame release failed");
    kprintln!("paging self-test: ok");
}
