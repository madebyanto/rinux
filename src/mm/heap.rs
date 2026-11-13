use core::alloc::Layout;
use core::mem::size_of;
use core::ptr;

pub const PAGE_SIZE: usize = 4096;
const MIN_BLOCK: usize = 32;
const MAX_BLOCK: usize = 2048;
const CLASS_COUNT: usize = 7;
const WORD: usize = size_of::<usize>();

pub struct Heap {
    end: usize,
    next_page: usize,
    classes: [usize; CLASS_COUNT],
    free_runs: usize,
}

impl Heap {
    pub const fn new() -> Self {
        Self {
            end: 0,
            next_page: 0,
            classes: [0; CLASS_COUNT],
            free_runs: 0,
        }
    }

    pub unsafe fn init(&mut self, start: usize, size: usize) {
        assert!(start % PAGE_SIZE == 0 && size % PAGE_SIZE == 0 && size > 0);
        self.end = start + size;
        self.next_page = start;
        self.classes = [0; CLASS_COUNT];
        self.free_runs = 0;
    }

    pub unsafe fn alloc(&mut self, layout: Layout) -> *mut u8 {
        match size_class(layout) {
            Some(index) => self.alloc_block(index),
            None => self.alloc_large(layout),
        }
    }

    pub unsafe fn dealloc(&mut self, ptr: *mut u8, layout: Layout) {
        let address = ptr as usize;
        match size_class(layout) {
            Some(index) => self.push_block(index, address),
            None => self.push_run(address, pages_for(layout.size())),
        }
    }

    unsafe fn alloc_block(&mut self, index: usize) -> *mut u8 {
        if self.classes[index] == 0 {
            let page = match self.take_pages(1) {
                Some(page) => page,
                None => return ptr::null_mut(),
            };
            let size = block_size(index);
            let mut offset = PAGE_SIZE;
            while offset >= size {
                offset -= size;
                self.push_block(index, page + offset);
            }
        }
        let block = self.classes[index];
        self.classes[index] = read_word(block);
        block as *mut u8
    }

    unsafe fn alloc_large(&mut self, layout: Layout) -> *mut u8 {
        if layout.align() > PAGE_SIZE {
            return ptr::null_mut();
        }
        match self.take_pages(pages_for(layout.size())) {
            Some(address) => address as *mut u8,
            None => ptr::null_mut(),
        }
    }

    unsafe fn push_block(&mut self, index: usize, address: usize) {
        write_word(address, self.classes[index]);
        self.classes[index] = address;
    }

    unsafe fn push_run(&mut self, address: usize, pages: usize) {
        if address + pages * PAGE_SIZE == self.next_page {
            self.next_page = address;
            return;
        }
        let mut previous: Option<usize> = None;
        let mut successor = self.free_runs;
        while successor != 0 && successor < address {
            previous = Some(successor);
            successor = read_word(successor);
        }
        let start = address;
        let mut count = pages;
        if successor != 0 && start + count * PAGE_SIZE == successor {
            count += read_word(successor + WORD);
            successor = read_word(successor);
        }
        match previous {
            Some(link) => {
                let link_pages = read_word(link + WORD);
                if link + link_pages * PAGE_SIZE == start {
                    write_word(link, successor);
                    write_word(link + WORD, link_pages + count);
                } else {
                    write_word(start, successor);
                    write_word(start + WORD, count);
                    write_word(link, start);
                }
            }
            None => {
                write_word(start, successor);
                write_word(start + WORD, count);
                self.free_runs = start;
            }
        }
    }

    unsafe fn take_pages(&mut self, count: usize) -> Option<usize> {
        let mut previous: Option<usize> = None;
        let mut run = self.free_runs;
        while run != 0 {
            let next = read_word(run);
            let available = read_word(run + WORD);
            if available >= count {
                let remaining = available - count;
                let replacement = if remaining == 0 {
                    next
                } else {
                    let rest = run + count * PAGE_SIZE;
                    write_word(rest, next);
                    write_word(rest + WORD, remaining);
                    rest
                };
                match previous {
                    None => self.free_runs = replacement,
                    Some(link) => write_word(link, replacement),
                }
                return Some(run);
            }
            previous = Some(run);
            run = next;
        }
        let bytes = count * PAGE_SIZE;
        let page = self.next_page;
        let end = page.checked_add(bytes)?;
        if end > self.end {
            return None;
        }
        self.next_page = end;
        Some(page)
    }
}

fn size_class(layout: Layout) -> Option<usize> {
    let block = layout.size().max(layout.align()).max(MIN_BLOCK);
    if block > MAX_BLOCK {
        return None;
    }
    Some(block.next_power_of_two().trailing_zeros() as usize - MIN_BLOCK.trailing_zeros() as usize)
}

fn block_size(index: usize) -> usize {
    MIN_BLOCK << index
}

fn pages_for(size: usize) -> usize {
    size.div_ceil(PAGE_SIZE).max(1)
}

unsafe fn read_word(address: usize) -> usize {
    (address as *const usize).read()
}

unsafe fn write_word(address: usize, value: usize) {
    (address as *mut usize).write(value)
}
