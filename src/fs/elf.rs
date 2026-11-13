#![allow(dead_code)]
pub const MAX_SEGMENTS: usize = 16;
pub const MAX_ARGS: usize = 16;
pub const MAX_ENV: usize = 16;
pub const MAX_ARG_BYTES: usize = 8192;
pub const MAX_STRING: usize = 256;
pub const STACK_GUARD: usize = 256;

pub const PF_X: u32 = 1;
pub const PF_W: u32 = 2;
pub const PF_R: u32 = 4;

const EHDR_SIZE: usize = 64;
const PHDR_SIZE: usize = 56;
const PT_LOAD: u32 = 1;
const ET_EXEC: u16 = 2;
const EM_X86_64: u16 = 62;
const ELFCLASS64: u8 = 2;
const ELFDATA2LSB: u8 = 1;
const EV_CURRENT: u8 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ElfError {
    TooSmall,
    BadMagic,
    NotElf64,
    NotLittleEndian,
    BadVersion,
    WrongType,
    WrongMachine,
    BadHeader,
    BadProgramHeaderSize,
    NoLoadSegments,
    TooManySegments,
    SegmentOutOfFile,
    FileSizeExceedsMemory,
    AddressOverflow,
    SegmentOutsideWindow,
    OverlappingSegments,
    BadFlags,
    EntryNotExecutable,
    NoExecutableSegment,
    StackOverflow,
    TooManyArguments,
    StringTooLong,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Segment {
    pub offset: usize,
    pub vaddr: u64,
    pub filesz: usize,
    pub memsz: usize,
    pub flags: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Image {
    pub entry: u64,
    segments: [Segment; MAX_SEGMENTS],
    count: usize,
}

impl Image {
    pub fn segments(&self) -> &[Segment] {
        &self.segments[..self.count]
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Protection {
    pub writable: bool,
    pub executable: bool,
}

pub fn protection(flags: u32) -> Protection {
    Protection {
        writable: (flags & PF_W) != 0,
        executable: (flags & PF_X) != 0,
    }
}

fn read_u16(data: &[u8], offset: usize) -> Option<u16> {
    let bytes = data.get(offset..offset + 2)?;
    Some(u16::from_le_bytes([bytes[0], bytes[1]]))
}

fn read_u32(data: &[u8], offset: usize) -> Option<u32> {
    let bytes = data.get(offset..offset + 4)?;
    Some(u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
}

fn read_u64(data: &[u8], offset: usize) -> Option<u64> {
    let bytes = data.get(offset..offset + 8)?;
    Some(u64::from_le_bytes([
        bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7],
    ]))
}

fn align_up(value: u64, alignment: u64) -> Option<u64> {
    let mask = alignment - 1;
    value.checked_add(mask).map(|value| value & !mask)
}

pub fn parse(data: &[u8], window_start: u64, window_size: u64) -> Result<Image, ElfError> {
    if data.len() < EHDR_SIZE {
        return Err(ElfError::TooSmall);
    }
    if &data[0..4] != b"\x7FELF" {
        return Err(ElfError::BadMagic);
    }
    if data[4] != ELFCLASS64 {
        return Err(ElfError::NotElf64);
    }
    if data[5] != ELFDATA2LSB {
        return Err(ElfError::NotLittleEndian);
    }
    if data[6] != EV_CURRENT {
        return Err(ElfError::BadVersion);
    }
    if read_u16(data, 16).ok_or(ElfError::BadHeader)? != ET_EXEC {
        return Err(ElfError::WrongType);
    }
    if read_u16(data, 18).ok_or(ElfError::BadHeader)? != EM_X86_64 {
        return Err(ElfError::WrongMachine);
    }
    if read_u32(data, 20).ok_or(ElfError::BadHeader)? != 1 {
        return Err(ElfError::BadVersion);
    }
    if read_u16(data, 52).ok_or(ElfError::BadHeader)? as usize != EHDR_SIZE {
        return Err(ElfError::BadHeader);
    }
    if read_u16(data, 54).ok_or(ElfError::BadHeader)? as usize != PHDR_SIZE {
        return Err(ElfError::BadProgramHeaderSize);
    }
    let entry = read_u64(data, 24).ok_or(ElfError::BadHeader)?;
    let phoff = read_u64(data, 32).ok_or(ElfError::BadHeader)?;
    let phnum = read_u16(data, 56).ok_or(ElfError::BadHeader)? as usize;
    if phnum == 0 {
        return Err(ElfError::NoLoadSegments);
    }
    if phnum > MAX_SEGMENTS {
        return Err(ElfError::TooManySegments);
    }
    let phoff = usize::try_from(phoff).map_err(|_| ElfError::BadHeader)?;
    let table_size = phnum.checked_mul(PHDR_SIZE).ok_or(ElfError::BadHeader)?;
    let table_end = phoff.checked_add(table_size).ok_or(ElfError::BadHeader)?;
    if table_end > data.len() {
        return Err(ElfError::BadHeader);
    }
    let window_end = window_start
        .checked_add(window_size)
        .ok_or(ElfError::AddressOverflow)?;

    let mut segments = [Segment {
        offset: 0,
        vaddr: 0,
        filesz: 0,
        memsz: 0,
        flags: 0,
    }; MAX_SEGMENTS];
    let mut count = 0;
    let mut entry_ok = false;

    for index in 0..phnum {
        let base = phoff + index * PHDR_SIZE;
        let p_type = read_u32(data, base).ok_or(ElfError::BadHeader)?;
        if p_type != PT_LOAD {
            continue;
        }
        let p_flags = read_u32(data, base + 4).ok_or(ElfError::BadHeader)?;
        if (p_flags & !(PF_R | PF_W | PF_X)) != 0 {
            return Err(ElfError::BadFlags);
        }
        let p_offset = read_u64(data, base + 8).ok_or(ElfError::BadHeader)?;
        let p_vaddr = read_u64(data, base + 16).ok_or(ElfError::BadHeader)?;
        let p_filesz = read_u64(data, base + 32).ok_or(ElfError::BadHeader)?;
        let p_memsz = read_u64(data, base + 40).ok_or(ElfError::BadHeader)?;
        if p_filesz > p_memsz {
            return Err(ElfError::FileSizeExceedsMemory);
        }
        let p_offset = usize::try_from(p_offset).map_err(|_| ElfError::SegmentOutOfFile)?;
        let p_filesz = usize::try_from(p_filesz).map_err(|_| ElfError::SegmentOutOfFile)?;
        let p_memsz = usize::try_from(p_memsz).map_err(|_| ElfError::AddressOverflow)?;
        let file_end = p_offset
            .checked_add(p_filesz)
            .ok_or(ElfError::SegmentOutOfFile)?;
        if file_end > data.len() {
            return Err(ElfError::SegmentOutOfFile);
        }
        let memory_end = p_vaddr
            .checked_add(p_memsz as u64)
            .ok_or(ElfError::AddressOverflow)?;
        if p_memsz != 0 {
            let page_start = p_vaddr & !0xFFF;
            let page_end = align_up(memory_end, 4096).ok_or(ElfError::AddressOverflow)?;
            if page_start < window_start || page_end > window_end {
                return Err(ElfError::SegmentOutsideWindow);
            }
            for segment in segments.iter().take(count) {
                if segment.memsz == 0 {
                    continue;
                }
                let other_start = segment.vaddr & !0xFFF;
                let other_end = align_up(segment.vaddr + segment.memsz as u64, 4096)
                    .ok_or(ElfError::AddressOverflow)?;
                if page_start < other_end && other_start < page_end {
                    return Err(ElfError::OverlappingSegments);
                }
            }
        }
        if (p_flags & PF_X) != 0 && p_memsz != 0 && entry >= p_vaddr && entry < memory_end {
            entry_ok = true;
        }
        segments[count] = Segment {
            offset: p_offset,
            vaddr: p_vaddr,
            filesz: p_filesz,
            memsz: p_memsz,
            flags: p_flags,
        };
        count += 1;
    }

    if count == 0 {
        return Err(ElfError::NoLoadSegments);
    }
    if !entry_ok {
        return Err(ElfError::EntryNotExecutable);
    }
    Ok(Image {
        entry,
        segments,
        count,
    })
}

fn write_u64(buffer: &mut [u8], offset: usize, value: u64) -> Result<(), ElfError> {
    let bytes = buffer
        .get_mut(offset..offset + 8)
        .ok_or(ElfError::StackOverflow)?;
    bytes.copy_from_slice(&value.to_le_bytes());
    Ok(())
}

pub fn build_stack(
    buffer: &mut [u8],
    stack_top: u64,
    argv: &[&str],
    envp: &[&str],
) -> Result<u64, ElfError> {
    if argv.len() > MAX_ARGS || envp.len() > MAX_ENV {
        return Err(ElfError::TooManyArguments);
    }
    let stack_base = stack_top
        .checked_sub(buffer.len() as u64)
        .ok_or(ElfError::StackOverflow)?;
    let mut argv_addresses = [0u64; MAX_ARGS];
    let mut envp_addresses = [0u64; MAX_ENV];
    let mut cursor = buffer.len();
    let mut total = 0usize;
    for (index, argument) in argv.iter().enumerate() {
        if argument.len() > MAX_STRING {
            return Err(ElfError::StringTooLong);
        }
        total = total
            .checked_add(argument.len() + 1)
            .ok_or(ElfError::StackOverflow)?;
        cursor = cursor
            .checked_sub(argument.len() + 1)
            .ok_or(ElfError::StackOverflow)?;
        buffer[cursor..cursor + argument.len()].copy_from_slice(argument.as_bytes());
        buffer[cursor + argument.len()] = 0;
        argv_addresses[index] = stack_base + cursor as u64;
    }
    for (index, variable) in envp.iter().enumerate() {
        if variable.len() > MAX_STRING {
            return Err(ElfError::StringTooLong);
        }
        total = total
            .checked_add(variable.len() + 1)
            .ok_or(ElfError::StackOverflow)?;
        cursor = cursor
            .checked_sub(variable.len() + 1)
            .ok_or(ElfError::StackOverflow)?;
        buffer[cursor..cursor + variable.len()].copy_from_slice(variable.as_bytes());
        buffer[cursor + variable.len()] = 0;
        envp_addresses[index] = stack_base + cursor as u64;
    }
    if total > MAX_ARG_BYTES {
        return Err(ElfError::StackOverflow);
    }
    let words = 1usize
        .checked_add(argv.len())
        .and_then(|value| value.checked_add(1))
        .and_then(|value| value.checked_add(envp.len()))
        .and_then(|value| value.checked_add(1))
        .ok_or(ElfError::StackOverflow)?;
    let arrays_start = cursor
        .checked_sub(words * 8)
        .ok_or(ElfError::StackOverflow)?;
    let stack_pointer = arrays_start & !0xF;
    if stack_pointer < STACK_GUARD {
        return Err(ElfError::StackOverflow);
    }
    let mut offset = stack_pointer;
    write_u64(buffer, offset, argv.len() as u64)?;
    offset += 8;
    for address in argv_addresses.iter().take(argv.len()) {
        write_u64(buffer, offset, *address)?;
        offset += 8;
    }
    write_u64(buffer, offset, 0)?;
    offset += 8;
    for address in envp_addresses.iter().take(envp.len()) {
        write_u64(buffer, offset, *address)?;
        offset += 8;
    }
    write_u64(buffer, offset, 0)?;
    Ok(stack_base + stack_pointer as u64)
}
