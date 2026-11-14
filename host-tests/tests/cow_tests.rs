use hosttest::address_space::{AddressSpace, CowError};
use hosttest::fault::{
    PageFaultAccess, PageFaultError, PageFaultOrigin, FAULT_INSTRUCTION, FAULT_PRESENT,
    FAULT_RESERVED, FAULT_USER, FAULT_WRITE,
};
use hosttest::frame::{FrameError, FRAME_SIZE};
use hosttest::pte::{
    child_mapping_flags, frame_address, is_cow, is_present, is_user, is_writable, make_cow,
    make_writable, validate_cow_leaf, with_frame, CowLeafError, ADDRESS_MASK, COW, COW_BIT,
    NO_EXECUTE, PRESENT, USER, WRITABLE,
};

const PHYS: u64 = 0x1234_5000;
const OTHER_PHYS: usize = 0x9ABC_D000;
const USER_VIRT: usize = 0x80_0000_0000;
const USER_VIRT_2: usize = USER_VIRT + 0x1000;
const USER_VIRT_3: usize = USER_VIRT + 0x2000;

#[test]
fn cow_bit_is_the_software_available_bit() {
    assert_eq!(COW_BIT, 9);
    assert_eq!(COW, 1 << 9);
    assert_eq!(COW & ADDRESS_MASK, 0);
}

#[test]
fn cow_transforms_preserve_frame_and_permissions() {
    let entry = PHYS | PRESENT | WRITABLE | USER | NO_EXECUTE;

    let cow = make_cow(entry);
    assert!(is_present(cow));
    assert!(is_user(cow));
    assert!(is_cow(cow));
    assert!(!is_writable(cow));
    assert_eq!(cow & NO_EXECUTE, NO_EXECUTE);
    assert_eq!(frame_address(cow), PHYS as usize);

    let writable = make_writable(cow);
    assert!(is_writable(writable));
    assert!(!is_cow(writable));
    assert!(is_user(writable));
    assert_eq!(writable & NO_EXECUTE, NO_EXECUTE);
    assert_eq!(frame_address(writable), PHYS as usize);
}

#[test]
fn child_mapping_flags_are_always_read_only() {
    let writable = PHYS | PRESENT | WRITABLE | USER | NO_EXECUTE;
    let flags = child_mapping_flags(writable);
    assert_eq!(flags & WRITABLE, 0);
    assert_eq!(flags & COW, COW);
    assert_eq!(flags & USER, USER);
    assert_eq!(flags & NO_EXECUTE, NO_EXECUTE);
    assert_eq!(flags & ADDRESS_MASK, 0);

    let already_cow = make_cow(writable);
    let flags = child_mapping_flags(already_cow);
    assert_eq!(flags & WRITABLE, 0);
    assert_eq!(flags & COW, COW);

    let read_only = PHYS | PRESENT | USER | NO_EXECUTE;
    let flags = child_mapping_flags(read_only);
    assert_eq!(flags & WRITABLE, 0);
    assert_eq!(flags & COW, 0);
    assert_eq!(flags & USER, USER);
    assert_eq!(flags & NO_EXECUTE, NO_EXECUTE);
}

#[test]
fn replacing_a_pte_frame_keeps_permissions() {
    let cow = make_cow(PHYS | PRESENT | WRITABLE | USER | NO_EXECUTE);
    let replaced = with_frame(cow, OTHER_PHYS);

    assert_eq!(frame_address(replaced), OTHER_PHYS);
    assert!(is_present(replaced));
    assert!(is_user(replaced));
    assert!(is_cow(replaced));
    assert!(!is_writable(replaced));
    assert_eq!(replaced & NO_EXECUTE, NO_EXECUTE);
}

#[test]
fn decodes_page_fault_error_codes() {
    let kernel_write = PageFaultError::new(FAULT_PRESENT | FAULT_WRITE);
    assert!(kernel_write.present());
    assert!(kernel_write.write());
    assert!(!kernel_write.user());
    assert_eq!(kernel_write.access(), PageFaultAccess::Write);
    assert_eq!(kernel_write.origin(), PageFaultOrigin::Kernel);
    assert!(kernel_write.is_cow_candidate());

    let user_write = PageFaultError::new(FAULT_PRESENT | FAULT_WRITE | FAULT_USER);
    assert_eq!(user_write.origin(), PageFaultOrigin::User);
    assert!(user_write.is_cow_candidate());

    let read = PageFaultError::new(FAULT_PRESENT | FAULT_USER);
    assert_eq!(read.access(), PageFaultAccess::Read);
    assert!(!read.is_cow_candidate());

    let not_present = PageFaultError::new(FAULT_WRITE | FAULT_USER);
    assert!(!not_present.present());
    assert!(!not_present.is_cow_candidate());

    let instruction = PageFaultError::new(FAULT_PRESENT | FAULT_WRITE | FAULT_INSTRUCTION);
    assert_eq!(instruction.access(), PageFaultAccess::InstructionFetch);
    assert!(!instruction.is_cow_candidate());

    let reserved = PageFaultError::new(FAULT_PRESENT | FAULT_WRITE | FAULT_RESERVED);
    assert!(reserved.reserved());
    assert!(!reserved.is_cow_candidate());
}

#[test]
fn validate_cow_leaf_accepts_only_read_only_user_cow_pages() {
    let cow = make_cow(PHYS | PRESENT | WRITABLE | USER);
    assert_eq!(validate_cow_leaf(cow), Ok(()));

    assert_eq!(
        validate_cow_leaf(PHYS | USER | COW),
        Err(CowLeafError::NotPresent)
    );
    assert_eq!(
        validate_cow_leaf(PHYS | PRESENT | COW),
        Err(CowLeafError::NotUser)
    );
    assert_eq!(
        validate_cow_leaf(PHYS | PRESENT | USER),
        Err(CowLeafError::NotCow)
    );
    assert_eq!(
        validate_cow_leaf(PHYS | PRESENT | USER | WRITABLE | COW),
        Err(CowLeafError::Writable)
    );
}

fn fill_frame(address: usize, byte: u8) {
    unsafe { core::ptr::write_bytes(address as *mut u8, byte, FRAME_SIZE) };
}

fn frame_byte(address: usize, offset: usize) -> u8 {
    unsafe { core::ptr::read_volatile((address + offset) as *const u8) }
}

#[test]
fn resolves_cow_faults_in_place_and_by_copying() {
    hosttest::init_frames();
    hosttest::paging::init_kernel_root();

    // Sole owner: promote the existing frame without allocating or copying.
    let mut parent = AddressSpace::new().expect("parent address space");
    let original = hosttest::allocate_frame().expect("original frame");
    fill_frame(original, 0x5A);
    parent
        .map_user(USER_VIRT, original, USER | WRITABLE | NO_EXECUTE)
        .expect("map original");
    let child = parent.clone_user().expect("clone");
    assert_eq!(hosttest::frame_refcount(original), Ok(2));
    assert_eq!(
        hosttest::paging::translate_in(child.root(), USER_VIRT),
        Some(original)
    );
    drop(child);
    assert_eq!(hosttest::frame_refcount(original), Ok(1));

    let free_before = hosttest::free_frame_count();
    parent.handle_cow_fault(USER_VIRT).expect("promote sole owner");
    assert_eq!(
        hosttest::paging::translate_in(parent.root(), USER_VIRT),
        Some(original)
    );
    assert_eq!(hosttest::frame_refcount(original), Ok(1));
    assert_eq!(hosttest::free_frame_count(), free_before);
    assert_eq!(
        parent.handle_cow_fault(USER_VIRT),
        Err(CowError::Leaf(CowLeafError::NotCow))
    );
    drop(parent);

    // Shared owner: copy the page, then drop the old reference.
    let mut parent = AddressSpace::new().expect("parent address space");
    let shared = hosttest::allocate_frame().expect("shared frame");
    fill_frame(shared, 0xA5);
    parent
        .map_user(USER_VIRT, shared, USER | WRITABLE | NO_EXECUTE)
        .expect("map shared");
    let child = parent.clone_user().expect("clone");
    assert_eq!(hosttest::frame_refcount(shared), Ok(2));

    let free_before = hosttest::free_frame_count();
    parent.handle_cow_fault(USER_VIRT).expect("copy shared page");
    let private = hosttest::paging::translate_in(parent.root(), USER_VIRT).expect("parent page");
    assert_ne!(private, shared);
    assert_eq!(hosttest::frame_refcount(shared), Ok(1));
    assert_eq!(hosttest::frame_refcount(private), Ok(1));
    assert_eq!(hosttest::free_frame_count() + 1, free_before);
    assert_eq!(
        hosttest::paging::translate_in(child.root(), USER_VIRT),
        Some(shared)
    );
    assert_eq!(frame_byte(shared, 0), 0xA5);
    assert_eq!(frame_byte(private, 0), 0xA5);
    assert_eq!(frame_byte(private, FRAME_SIZE - 1), 0xA5);
    drop(child);
    assert_eq!(
        hosttest::frame_refcount(shared),
        Err(FrameError::NotAllocated)
    );
    drop(parent);

    // Leaf validation reaches the fault path intact.
    let mut other = AddressSpace::new().expect("parent address space");
    assert_eq!(
        other.handle_cow_fault(USER_VIRT),
        Err(CowError::Leaf(CowLeafError::NotPresent))
    );
    let writable = hosttest::allocate_frame().expect("writable frame");
    other
        .map_user(USER_VIRT, writable, USER | WRITABLE | NO_EXECUTE)
        .expect("map writable");
    assert_eq!(
        other.handle_cow_fault(USER_VIRT),
        Err(CowError::Leaf(CowLeafError::NotCow))
    );

    let supervisor = hosttest::allocate_frame().expect("supervisor frame");
    hosttest::paging::map_in(other.root(), USER_VIRT_2, supervisor, WRITABLE)
        .expect("map supervisor page");
    assert_eq!(
        other.handle_cow_fault(USER_VIRT_2),
        Err(CowError::Leaf(CowLeafError::NotUser))
    );

    let writable_cow = hosttest::allocate_frame().expect("writable cow frame");
    hosttest::paging::map_in(
        other.root(),
        USER_VIRT_3,
        writable_cow,
        USER | WRITABLE | COW,
    )
    .expect("map writable cow page");
    assert_eq!(
        other.handle_cow_fault(USER_VIRT_3),
        Err(CowError::Leaf(CowLeafError::Writable))
    );
}
