use hosttest::user_range::{
    range_valid, stack_valid, PROGRAM_BASE, PROGRAM_SIZE, STACK_BASE, STACK_TOP,
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
