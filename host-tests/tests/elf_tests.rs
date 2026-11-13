use hosttest::fs::elf::{
    build_stack, parse, protection, ElfError, PF_R, PF_W, PF_X,
};

const BASE: u64 = 0x0000_0080_0000_0000;
const WINDOW: u64 = 64 * 1024;

fn put_u16(data: &mut [u8], offset: usize, value: u16) {
    data[offset..offset + 2].copy_from_slice(&value.to_le_bytes());
}

fn put_u32(data: &mut [u8], offset: usize, value: u32) {
    data[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
}

fn put_u64(data: &mut [u8], offset: usize, value: u64) {
    data[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
}

fn valid_elf() -> Vec<u8> {
    let mut data = vec![0u8; 0x3000];
    data[0..4].copy_from_slice(b"\x7FELF");
    data[4] = 2;
    data[5] = 1;
    data[6] = 1;
    put_u16(&mut data, 16, 2);
    put_u16(&mut data, 18, 62);
    put_u32(&mut data, 20, 1);
    put_u64(&mut data, 24, BASE);
    put_u64(&mut data, 32, 64);
    put_u16(&mut data, 52, 64);
    put_u16(&mut data, 54, 56);
    put_u16(&mut data, 56, 2);

    let first = 64;
    put_u32(&mut data, first, 1);
    put_u32(&mut data, first + 4, PF_R | PF_X);
    put_u64(&mut data, first + 8, 0x1000);
    put_u64(&mut data, first + 16, BASE);
    put_u64(&mut data, first + 32, 4);
    put_u64(&mut data, first + 40, 4);

    let second = 64 + 56;
    put_u32(&mut data, second, 1);
    put_u32(&mut data, second + 4, PF_R | PF_W);
    put_u64(&mut data, second + 8, 0x2000);
    put_u64(&mut data, second + 16, BASE + 0x1000);
    put_u64(&mut data, second + 32, 4);
    put_u64(&mut data, second + 40, 8);

    data[0x1000..0x1004].copy_from_slice(&[0x90, 0x90, 0x90, 0xC3]);
    data[0x2000..0x2004].copy_from_slice(&[1, 2, 3, 4]);
    data
}

#[test]
fn parses_valid_elf64_and_protections() {
    let image = parse(&valid_elf(), BASE, WINDOW).unwrap();
    assert_eq!(image.entry, BASE);
    let segments = image.segments();
    assert_eq!(segments.len(), 2);
    assert_eq!(segments[0].vaddr, BASE);
    assert_eq!(segments[0].filesz, 4);
    assert_eq!(segments[0].memsz, 4);
    assert!(protection(segments[0].flags).executable);
    assert!(!protection(segments[0].flags).writable);
    assert_eq!(segments[1].vaddr, BASE + 0x1000);
    assert_eq!(segments[1].filesz, 4);
    assert_eq!(segments[1].memsz, 8);
    assert!(protection(segments[1].flags).writable);
    assert!(!protection(segments[1].flags).executable);
}

#[test]
fn rejects_bad_ident_type_machine_and_headers() {
    let mut data = valid_elf();
    data[0] = 0;
    assert_eq!(parse(&data, BASE, WINDOW), Err(ElfError::BadMagic));

    let mut data = valid_elf();
    data[4] = 1;
    assert_eq!(parse(&data, BASE, WINDOW), Err(ElfError::NotElf64));

    let mut data = valid_elf();
    data[5] = 2;
    assert_eq!(parse(&data, BASE, WINDOW), Err(ElfError::NotLittleEndian));

    let mut data = valid_elf();
    put_u16(&mut data, 16, 3);
    assert_eq!(parse(&data, BASE, WINDOW), Err(ElfError::WrongType));

    let mut data = valid_elf();
    put_u16(&mut data, 18, 3);
    assert_eq!(parse(&data, BASE, WINDOW), Err(ElfError::WrongMachine));

    let mut data = valid_elf();
    put_u64(&mut data, 32, 0x10_0000);
    assert_eq!(parse(&data, BASE, WINDOW), Err(ElfError::BadHeader));

    let mut data = valid_elf();
    put_u16(&mut data, 56, 0);
    assert_eq!(parse(&data, BASE, WINDOW), Err(ElfError::NoLoadSegments));
}

#[test]
fn rejects_bad_segments_and_entry() {
    let mut data = valid_elf();
    let second = 64 + 56;
    put_u64(&mut data, second + 32, 9);
    assert_eq!(
        parse(&data, BASE, WINDOW),
        Err(ElfError::FileSizeExceedsMemory)
    );

    let mut data = valid_elf();
    put_u64(&mut data, second + 16, BASE + WINDOW);
    assert_eq!(
        parse(&data, BASE, WINDOW),
        Err(ElfError::SegmentOutsideWindow)
    );

    let mut data = valid_elf();
    put_u64(&mut data, second + 16, BASE);
    assert_eq!(
        parse(&data, BASE, WINDOW),
        Err(ElfError::OverlappingSegments)
    );

    let mut data = valid_elf();
    put_u64(&mut data, 24, BASE + 0x1000);
    assert_eq!(
        parse(&data, BASE, WINDOW),
        Err(ElfError::EntryNotExecutable)
    );

    let mut data = valid_elf();
    put_u32(&mut data, 64, 2);
    put_u32(&mut data, 64 + 56, 2);
    assert_eq!(parse(&data, BASE, WINDOW), Err(ElfError::NoLoadSegments));
}

#[test]
fn builds_linux_stack_layout_with_arguments_and_environment() {
    let mut buffer = vec![0u8; 16 * 1024];
    let top = 0x0000_0080_0020_0000u64;
    let base = top - buffer.len() as u64;
    let rsp = build_stack(&mut buffer, top, &["/bin/init.elf", "hello"], &["A=1"]).unwrap();
    assert_eq!(rsp % 16, 0);
    let mut offset = (rsp - base) as usize;
    let argc = u64::from_le_bytes(buffer[offset..offset + 8].try_into().unwrap());
    assert_eq!(argc, 2);
    offset += 8;
    let argv0 = u64::from_le_bytes(buffer[offset..offset + 8].try_into().unwrap());
    offset += 8;
    let argv1 = u64::from_le_bytes(buffer[offset..offset + 8].try_into().unwrap());
    offset += 8;
    let argv_null = u64::from_le_bytes(buffer[offset..offset + 8].try_into().unwrap());
    assert_eq!(argv_null, 0);
    offset += 8;
    let env0 = u64::from_le_bytes(buffer[offset..offset + 8].try_into().unwrap());
    offset += 8;
    let env_null = u64::from_le_bytes(buffer[offset..offset + 8].try_into().unwrap());
    assert_eq!(env_null, 0);

    let read = |address: u64| -> String {
        let start = (address - base) as usize;
        let end = buffer[start..].iter().position(|byte| *byte == 0).unwrap() + start;
        String::from_utf8(buffer[start..end].to_vec()).unwrap()
    };
    assert_eq!(read(argv0), "/bin/init.elf");
    assert_eq!(read(argv1), "hello");
    assert_eq!(read(env0), "A=1");
}

#[test]
fn stack_builder_rejects_limits_and_overflow() {
    let mut buffer = vec![0u8; 16 * 1024];
    let top = 0x0000_0080_0020_0000u64;
    let arguments: Vec<&str> = core::iter::repeat("x").take(17).collect();
    assert_eq!(
        build_stack(&mut buffer, top, &arguments, &[]),
        Err(ElfError::TooManyArguments)
    );
    let long = "x".repeat(257);
    assert_eq!(
        build_stack(&mut buffer, top, &[long.as_str()], &[]),
        Err(ElfError::StringTooLong)
    );
    let mut tiny = vec![0u8; 64];
    assert_eq!(
        build_stack(&mut tiny, 64, &["hello"], &[]),
        Err(ElfError::StackOverflow)
    );
}
