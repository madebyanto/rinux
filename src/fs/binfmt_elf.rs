//! ELF executable loader.
//!
//! This is the only binary format the kernel understands. It maps `PT_LOAD`
//! segments at their fixed virtual addresses inside the user program window
//! and builds the initial stack with `argv`/`envp`.

use crate::fs::elf::{self, ElfError};
use crate::fs::vfs::FileType;
use crate::fs::vfs::VfsError;
use crate::mm;
use crate::mm::address_space::AddressSpace;
use crate::mm::uaccess::{
    PROGRAM_BASE, PROGRAM_SIZE, STACK_BASE, STACK_PAGES, STACK_SIZE, STACK_TOP,
};
use crate::syscall::abi;
use alloc::vec::Vec;
use core::ptr::{copy_nonoverlapping, write_bytes};

const MAX_ELF_FILE: usize = 256 * 1024;
const PAGE_SIZE: usize = 4096;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoadError {
    InvalidPath,
    Vfs(VfsError),
    Elf(ElfError),
    OutOfMemory,
    MapFailed,
    FileTooLarge,
    NotRegular,
    ReadFailed,
}

impl LoadError {
    /// Maps a loader failure to a Linux errno value (positive).
    pub fn errno(self) -> i64 {
        match self {
            LoadError::InvalidPath => abi::EINVAL,
            LoadError::Vfs(VfsError::NotFound) => abi::ENOENT,
            LoadError::Vfs(VfsError::NotDirectory) => abi::ENOTDIR,
            LoadError::Vfs(VfsError::NotFile) => abi::EISDIR,
            LoadError::Vfs(VfsError::InvalidPath) => abi::EINVAL,
            LoadError::Vfs(_) => abi::EIO,
            LoadError::Elf(ElfError::StackOverflow)
            | LoadError::Elf(ElfError::TooManyArguments)
            | LoadError::Elf(ElfError::StringTooLong) => abi::E2BIG,
            LoadError::Elf(_) => abi::ENOEXEC,
            LoadError::OutOfMemory | LoadError::MapFailed => abi::ENOMEM,
            LoadError::FileTooLarge | LoadError::ReadFailed => abi::ENOEXEC,
            LoadError::NotRegular => abi::EISDIR,
        }
    }
}

impl From<VfsError> for LoadError {
    fn from(error: VfsError) -> Self {
        LoadError::Vfs(error)
    }
}

impl From<ElfError> for LoadError {
    fn from(error: ElfError) -> Self {
        LoadError::Elf(error)
    }
}

impl core::fmt::Display for LoadError {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            LoadError::InvalidPath => write!(formatter, "invalid executable path"),
            LoadError::Vfs(error) => write!(formatter, "{}", error),
            LoadError::Elf(error) => write!(formatter, "ELF error {:?}", error),
            LoadError::OutOfMemory => write!(formatter, "out of memory"),
            LoadError::MapFailed => write!(formatter, "user mapping failed"),
            LoadError::FileTooLarge => write!(formatter, "ELF file too large"),
            LoadError::NotRegular => write!(formatter, "not a regular file"),
            LoadError::ReadFailed => write!(formatter, "ELF read failed"),
        }
    }
}

pub struct ExecImage {
    pub entry: u64,
    pub stack_top: u64,
    pub address_space: AddressSpace,
}

/// An executable file read into memory, ready for ELF validation.
pub struct ExecFile {
    data: Vec<u8>,
}

impl ExecFile {
    pub fn bytes(&self) -> &[u8] {
        &self.data
    }
}

/// Pipeline stage 1: reject paths that cannot name a kernel executable.
pub fn validate_path(path: &str) -> Result<(), LoadError> {
    if path.is_empty() || path.as_bytes().contains(&0) {
        return Err(LoadError::InvalidPath);
    }
    Ok(())
}

/// Pipeline stage 1 for init discovery: the path must also be absolute.
pub fn validate_init_path(path: &str) -> Result<(), LoadError> {
    validate_path(path)?;
    if !path.starts_with('/') {
        return Err(LoadError::InvalidPath);
    }
    Ok(())
}

/// Pipeline stage 2: stat and open the file, then read it into memory.
pub fn open_executable(path: &str) -> Result<ExecFile, LoadError> {
    let stat = crate::fs::stat_path(path)?;
    if stat.kind != FileType::Regular {
        return Err(LoadError::NotRegular);
    }
    let size = usize::try_from(stat.size).map_err(|_| LoadError::FileTooLarge)?;
    if size == 0 || size > MAX_ELF_FILE {
        return Err(LoadError::FileTooLarge);
    }
    let file = crate::fs::open(path)?;
    let mut data = Vec::new();
    data.try_reserve_exact(size)
        .map_err(|_| LoadError::OutOfMemory)?;
    data.resize(size, 0);
    let mut offset = 0;
    while offset < size {
        let count = crate::fs::read_node(file.node, offset as u64, &mut data[offset..])?;
        if count == 0 {
            break;
        }
        offset += count;
    }
    if offset != size {
        return Err(LoadError::ReadFailed);
    }
    Ok(ExecFile { data })
}

/// Pipeline stage 3: validate the ELF headers and load segments.
pub fn validate_elf(file: &ExecFile) -> Result<elf::Image, LoadError> {
    Ok(elf::parse(
        file.bytes(),
        PROGRAM_BASE as u64,
        PROGRAM_SIZE as u64,
    )?)
}

/// Pipeline stage 4: allocate and populate the new user address space.
pub fn build_image(
    file: &ExecFile,
    image: &elf::Image,
    argv: &[&str],
    envp: &[&str],
) -> Result<ExecImage, LoadError> {
    let data = file.bytes();
    let mut address_space = AddressSpace::new().map_err(|_| LoadError::OutOfMemory)?;

    for segment in image.segments() {
        if segment.memsz == 0 {
            continue;
        }
        let memory_end = segment
            .vaddr
            .checked_add(segment.memsz as u64)
            .ok_or(LoadError::Elf(ElfError::AddressOverflow))?;
        let page_start = segment.vaddr & !0xFFF;
        let page_end = align_up(memory_end).ok_or(LoadError::Elf(ElfError::AddressOverflow))?;
        let protection = elf::protection(segment.flags);
        let mut flags = crate::mm::paging::USER;
        if protection.writable {
            flags |= crate::mm::paging::WRITABLE;
        }
        if !protection.executable {
            flags |= crate::mm::paging::NO_EXECUTE;
        }
        let mut page = page_start;
        while page < page_end {
            let phys = mm::allocate_frame().ok_or(LoadError::OutOfMemory)?;
            unsafe { write_bytes(phys as *mut u8, 0, PAGE_SIZE) };
            let copy_start = segment.vaddr.max(page);
            let copy_end = memory_end.min(page + PAGE_SIZE as u64);
            if copy_end > copy_start {
                let file_bytes = (segment.vaddr + segment.filesz as u64).min(copy_end);
                if file_bytes > copy_start {
                    let source = segment.offset + (copy_start - segment.vaddr) as usize;
                    let destination = phys + (copy_start - page) as usize;
                    let count = (file_bytes - copy_start) as usize;
                    unsafe {
                        copy_nonoverlapping(
                            data.as_ptr().add(source),
                            destination as *mut u8,
                            count,
                        );
                    }
                }
            }
            if address_space.map_user(page as usize, phys, flags).is_err() {
                let _ = mm::release_frame(phys);
                return Err(LoadError::MapFailed);
            }
            page += PAGE_SIZE as u64;
        }
    }

    let mut stack_frames = Vec::new();
    stack_frames
        .try_reserve_exact(STACK_PAGES)
        .map_err(|_| LoadError::OutOfMemory)?;
    for page in 0..STACK_PAGES {
        let virt = STACK_BASE + page * PAGE_SIZE;
        let phys = mm::allocate_frame().ok_or(LoadError::OutOfMemory)?;
        unsafe { write_bytes(phys as *mut u8, 0, PAGE_SIZE) };
        if address_space
            .map_user(
                virt,
                phys,
                crate::mm::paging::USER
                    | crate::mm::paging::WRITABLE
                    | crate::mm::paging::NO_EXECUTE,
            )
            .is_err()
        {
            let _ = mm::release_frame(phys);
            return Err(LoadError::MapFailed);
        }
        stack_frames.push(phys);
    }

    let mut stack = Vec::new();
    stack
        .try_reserve_exact(STACK_SIZE)
        .map_err(|_| LoadError::OutOfMemory)?;
    stack.resize(STACK_SIZE, 0);
    let stack_pointer = elf::build_stack(&mut stack, STACK_TOP as u64, argv, envp)?;
    for (page, phys) in stack_frames.iter().enumerate() {
        unsafe {
            copy_nonoverlapping(
                stack.as_ptr().add(page * PAGE_SIZE),
                *phys as *mut u8,
                PAGE_SIZE,
            );
        }
    }

    Ok(ExecImage {
        entry: image.entry,
        stack_top: stack_pointer,
        address_space,
    })
}

/// Runs stages 1-4 for an already resolved path.
pub fn prepare(path: &str, argv: &[&str], envp: &[&str]) -> Result<ExecImage, LoadError> {
    validate_path(path)?;
    let file = open_executable(path)?;
    let image = validate_elf(&file)?;
    build_image(&file, &image, argv, envp)
}

/// Loads an executable for `execve` from user space.
///
/// Kernel-space init discovery uses the individual pipeline stages directly so
/// it can report which candidate failed.
pub fn load(path: &str, argv: &[&str], envp: &[&str]) -> Result<ExecImage, LoadError> {
    prepare(path, argv, envp)
}

fn align_up(value: u64) -> Option<u64> {
    value
        .checked_add(PAGE_SIZE as u64 - 1)
        .map(|value| value & !(PAGE_SIZE as u64 - 1))
}
