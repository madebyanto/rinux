use hosttest::user_range::{
    pml4_index, range_valid, stack_valid, PROGRAM_BASE, PROGRAM_SIZE, STACK_BASE, STACK_TOP,
    USER_PML4_INDEX,
};

#[test]
fn accepts_the_user_program_and_stack_ranges() {
    assert!(range_valid(PROGRAM_BASE, 1));
    assert!(range_valid(PROGRAM_BASE, PROGRAM_SIZE));
    assert!(range_valid(PROGRAM_BASE + 16, 32));
    assert!(range_valid(STACK_BASE, 1));
    assert!(range_valid(STACK_TOP - 1, 1));
    assert!(range_valid(STACK_TOP - 16, 16));
}

#[test]
fn stack_valid_rejects_program_and_kernel_addresses() {
    assert!(stack_valid(STACK_BASE, 1));
    assert!(stack_valid(STACK_TOP - 16, 16));
    assert!(!stack_valid(PROGRAM_BASE, 1));
    assert!(!stack_valid(STACK_BASE - 1, 1));
    assert!(!stack_valid(STACK_TOP, 1));
}

#[test]
fn rejects_kernel_and_unmapped_addresses() {
    assert!(!range_valid(0, 8));
    assert!(!range_valid(0x10_0000, 8));
    assert!(!range_valid(PROGRAM_BASE - 1, 1));
    assert!(!range_valid(PROGRAM_BASE + PROGRAM_SIZE, 1));
    assert!(!range_valid(STACK_BASE - 1, 1));
    assert!(!range_valid(STACK_TOP, 1));
}

#[test]
fn rejects_ranges_that_cross_a_boundary_or_overflow() {
    assert!(!range_valid(PROGRAM_BASE + PROGRAM_SIZE - 1, 2));
    assert!(!range_valid(STACK_TOP - 1, 2));
    assert!(!range_valid(usize::MAX, 2));
    assert!(!range_valid(usize::MAX - 1, 2));
}

#[test]
fn user_ranges_live_in_a_single_pml4_slot() {
    assert_eq!(USER_PML4_INDEX, 1);
    assert_eq!(pml4_index(PROGRAM_BASE), USER_PML4_INDEX);
    assert_eq!(pml4_index(STACK_BASE), USER_PML4_INDEX);
    assert_eq!(pml4_index(STACK_TOP - 1), USER_PML4_INDEX);
    assert_eq!(pml4_index(0), 0);
    assert_eq!(pml4_index(0x4444_0000_0000), 136);
}
