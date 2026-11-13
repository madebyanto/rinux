pub const KERNEL_CODE: u64 = 0x00AF_9A00_0000_FFFF;
pub const KERNEL_DATA: u64 = 0x00CF_9200_0000_FFFF;
pub const USER_DATA: u64 = 0x00CF_F200_0000_FFFF;
pub const USER_CODE: u64 = 0x00AF_FA00_0000_FFFF;

pub fn tss_descriptor(base: u64, limit: u32) -> [u64; 2] {
    let limit = limit as u64;
    let access = 0x89u64;
    let low = (limit & 0xFFFF)
        | ((base & 0xFFFF) << 16)
        | (((base >> 16) & 0xFF) << 32)
        | (access << 40)
        | (((limit >> 16) & 0x0F) << 48)
        | (((base >> 24) & 0xFF) << 56);
    let high = base >> 32;
    [low, high]
}
