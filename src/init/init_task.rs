//! PID 1: the first kernel thread and the init discovery pipeline.
//!
//! `kernel_init` never returns. It walks the candidate list, running each path
//! through the executable pipeline until one commits into user space. If every
//! candidate fails, the kernel panics, matching Linux's boot failure mode.

use crate::arch::x86_64::cpu;
use crate::fs::binfmt_elf::{self, ExecImage, LoadError};
use crate::init::cmdline;
use crate::proc::{context, scheduler};

/// Maximum number of candidates: one `init=` path plus four fallbacks.
pub const MAX_INIT_CANDIDATES: usize = 5;

/// Standard fallback order used when `init=` is absent or fails.
pub const FALLBACK_INIT_PATHS: [&str; 4] = ["/sbin/init", "/etc/init", "/bin/init", "/bin/sh"];

const _: () = assert!(FALLBACK_INIT_PATHS.len() + 1 == MAX_INIT_CANDIDATES);

/// PID 1 entry point.
pub extern "C" fn kernel_init() -> ! {
    let params = cmdline::params();
    let explicit = params
        .init
        .as_ref()
        .and_then(|value| core::str::from_utf8(value.as_bytes()).ok())
        .filter(|path| binfmt_elf::validate_init_path(path).is_ok());

    let mut candidates = [None; MAX_INIT_CANDIDATES];
    let mut count = 0;
    if let Some(path) = explicit {
        candidates[count] = Some(path);
        count += 1;
    }
    for path in FALLBACK_INIT_PATHS {
        candidates[count] = Some(path);
        count += 1;
    }

    for candidate in candidates.iter().take(count).flatten() {
        crate::kprintln!("Run {} as init process", candidate);
        // The console descriptors must exist before the first user instruction.
        if scheduler::init_current_stdio().is_err() {
            panic!("kernel_init: no current process");
        }
        if let Err(error) = kernel_execve(candidate, &[candidate], &[]) {
            crate::kprintln!(
                "Failed to execute {} (error {})",
                candidate,
                -error.errno()
            );
        }
    }

    panic!("No working init found. Try passing init= option to kernel.");
}

/// Performs an in-place `execve` for the running kernel thread.
///
/// Strings are borrowed directly from kernel memory. On success the current
/// process slot, PID, parent link, and file descriptors are preserved while
/// the address space and user entry state are replaced. This function does not
/// return on success.
pub fn kernel_execve(path: &str, argv: &[&str], envp: &[&str]) -> Result<(), LoadError> {
    // Stage 1: path validation.
    binfmt_elf::validate_init_path(path)?;
    // Stage 2: open/stat and read the executable.
    let file = binfmt_elf::open_executable(path)?;
    // Stage 3: ELF header and segment validation.
    let parsed = binfmt_elf::validate_elf(&file)?;
    // Stage 4: image build (address space, segments, stack, argv/envp).
    let image = binfmt_elf::build_image(&file, &parsed, argv, envp)?;
    // Stage 5: commit into the current process and enter ring 3.
    commit(image)
}

fn commit(image: ExecImage) -> ! {
    let ExecImage {
        entry,
        stack_top,
        address_space,
    } = image;
    let new_root = address_space.root();
    let old = match scheduler::replace_address_space(address_space) {
        Ok(old) => old,
        Err(error) => panic!("kernel_execve: cannot replace address space: {:?}", error),
    };
    cpu::write_cr3(new_root);
    drop(old);
    if scheduler::reset_signals_for_exec().is_err() {
        panic!("kernel_execve: cannot reset signal state");
    }
    context::enter_user(entry, stack_top)
}
