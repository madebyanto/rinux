use super::descriptor;
use core::mem::size_of;
use core::ptr;

core::arch::global_asm!(include_str!("gdt.S"));

pub const KERNEL_CODE: u16 = 0x08;
pub const KERNEL_DATA: u16 = 0x10;
pub const TSS_SELECTOR: u16 = 0x18;
pub const USER_DATA: u16 = 0x28;
pub const USER_CODE: u16 = 0x30;
pub const USER_DATA_RPL3: u16 = USER_DATA | 3;
pub const USER_CODE_RPL3: u16 = USER_CODE | 3;

const GDT_ENTRIES: usize = 7;

#[repr(C, packed)]
pub struct TaskStateSegment {
    reserved0: u32,
    rsp0: u64,
    rsp1: u64,
    rsp2: u64,
    reserved1: u64,
    ist: [u64; 7],
    reserved2: u64,
    reserved3: u16,
    iomap_base: u16,
}

impl TaskStateSegment {
    const fn new() -> Self {
        Self {
            reserved0: 0,
            rsp0: 0,
            rsp1: 0,
            rsp2: 0,
            reserved1: 0,
            ist: [0; 7],
            reserved2: 0,
            reserved3: 0,
            iomap_base: 0x68,
        }
    }
}

#[no_mangle]
static mut TSS: TaskStateSegment = TaskStateSegment::new();

static mut GDT: [u64; GDT_ENTRIES] = [
    0,
    descriptor::KERNEL_CODE,
    descriptor::KERNEL_DATA,
    0,
    0,
    descriptor::USER_DATA,
    descriptor::USER_CODE,
];

#[repr(C, packed)]
struct GdtPointer {
    limit: u16,
    base: u64,
}

extern "C" {
    fn gdt_load(pointer: *const GdtPointer);
    #[link_name = "boot_stack_top"]
    static BOOT_STACK_TOP: u8;
}

pub fn init() {
    let base = ptr::addr_of!(TSS) as u64;
    let limit = (size_of::<TaskStateSegment>() - 1) as u32;
    let [tss_low, tss_high] = descriptor::tss_descriptor(base, limit);
    unsafe {
        let gdt = ptr::addr_of_mut!(GDT) as *mut u64;
        ptr::write(gdt.add(3), tss_low);
        ptr::write(gdt.add(4), tss_high);
    }
    let pointer = GdtPointer {
        limit: (size_of::<[u64; GDT_ENTRIES]>() - 1) as u16,
        base: ptr::addr_of!(GDT) as u64,
    };
    unsafe { gdt_load(&pointer) };
    set_rsp0(boot_stack_top());
}

pub fn set_rsp0(top: u64) {
    unsafe {
        ptr::addr_of_mut!(TSS.rsp0).write_unaligned(top);
    }
}

pub fn boot_stack_top() -> u64 {
    ptr::addr_of!(BOOT_STACK_TOP) as u64
}
