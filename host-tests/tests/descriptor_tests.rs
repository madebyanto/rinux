use hosttest::descriptor::{
    tss_descriptor, KERNEL_CODE, KERNEL_DATA, USER_CODE, USER_DATA,
};

fn access(entry: u64) -> u64 {
    (entry >> 40) & 0xFF
}

fn dpl(entry: u64) -> u64 {
    (entry >> 45) & 0x3
}

fn long_mode(entry: u64) -> u64 {
    (entry >> 53) & 1
}

#[test]
fn segment_descriptors_have_the_expected_privilege_and_mode() {
    assert_eq!(access(KERNEL_CODE), 0x9A);
    assert_eq!(access(KERNEL_DATA), 0x92);
    assert_eq!(access(USER_CODE), 0xFA);
    assert_eq!(access(USER_DATA), 0xF2);
    assert_eq!(dpl(KERNEL_CODE), 0);
    assert_eq!(dpl(KERNEL_DATA), 0);
    assert_eq!(dpl(USER_CODE), 3);
    assert_eq!(dpl(USER_DATA), 3);
    assert_eq!(long_mode(KERNEL_CODE), 1);
    assert_eq!(long_mode(USER_CODE), 1);
    assert_eq!(long_mode(USER_DATA), 0);
}

#[test]
fn tss_descriptor_packs_base_limit_and_access() {
    let base = 0x0000_0001_2345_6789u64;
    let limit = 0x67u32;
    let [low, high] = tss_descriptor(base, limit);
    assert_eq!(low & 0xFFFF, 0x67);
    assert_eq!((low >> 16) & 0xFFFF, base & 0xFFFF);
    assert_eq!((low >> 32) & 0xFF, (base >> 16) & 0xFF);
    assert_eq!((low >> 40) & 0xFF, 0x89);
    assert_eq!((low >> 48) & 0xF, 0);
    assert_eq!((low >> 56) & 0xFF, (base >> 24) & 0xFF);
    assert_eq!(high, base >> 32);
}
