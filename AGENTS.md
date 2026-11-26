# AGENTS.md

Hobby x86-64 kernel in Rust (`no_std`, no external crates), booted by Multiboot2/GRUB in QEMU. The `README` status list is stale (omits proc/fs/syscall/init); trust `Makefile`, `.cargo/config.toml`, and `src/`.

## Build and run

- `make image` is the full build: `cargo build`, then `objcopy` converts the kernel ELF and the Makefile packages `grub/grub.cfg.in` into `build/rinux.iso`. It does not build or stage any user-space program.
- `cargo build` alone only produces the kernel ELF under `target/x86_64-rinux/debug/`; it does not produce `build/rinux.iso`.
- `make run` boots the ISO plus an external root filesystem image attached as an IDE disk. Set `ROOTFS=/path/to/rootfs.img`; the default is `build/rootfs.img`. The target fails fast with a `ROOTFS is required` message when the file is missing.
- `KERNEL_CMDLINE` is injected into the GRUB entry (default `root=/dev/hda1`). Empty it to exercise the default init lookup.
- `make debug` adds a gdb stub (`-s -S`, :1234); symbols are at `target/x86_64-rinux/debug/rinux`.
- Headless smoke test (with a root filesystem): `timeout 10 make run QEMU='qemu-system-x86_64 -display none' ROOTFS=/path/to/rootfs.img`. Timeout exits 124; look for `Rinux 0.1.0`, `cmdline:` diagnostics, ext2 mount messages, and `Run <path> as init process`. Without `ROOTFS`, expect the fail-fast message instead.
- `make check` (or `cargo check`) no longer runs a `build.rs`; GNU `as` and `ld` are not needed for type-checking or the kernel build. `objcopy`, `grub-mkrescue`, and `xorriso` are needed for `make image`.
- Prerequisites beyond the README: binutils (`objcopy`), `grub-mkrescue` + `xorriso`, QEMU, and nightly with `rust-src` (see `rust-toolchain.toml`). `mke2fs` and `e2fsck` are needed only by `ext2_tests`.
- `make clean` runs `cargo clean` and removes `build/`; it does not clean `host-tests/target`.
- `build.sh` is an interactive menu that runs `cargo clean` before building; use Make targets for non-interactive work.

## Host tests

Kernel code is unit-tested on the host from `host-tests/`; there is no test harness in the root crate. From the repo root:

```sh
HOST_TESTS="$PWD/host-tests"
(cd /tmp && cargo +nightly test --manifest-path "$HOST_TESTS/Cargo.toml")
```

Do not run `cargo test` inside the repo (even from `host-tests/`): Cargo merges the root `.cargo/config.toml` (`build-std`), which makes the host build fail with a duplicate `alloc` lang item. Running outside the tree avoids that config while `--manifest-path` still finds the tests; `+nightly` restores the pinned toolchain that cwd-based rustup discovery loses.

- One suite: `... --test frame_tests`. One test: `... --test ext2_tests <filter>`.
- `ext2_tests` requires `mke2fs` and `e2fsck`; no other test needs QEMU or real hardware.
- `host-tests/src/lib.rs` includes kernel sources via `#[path = "../../src/..."]`; generic fs modules are listed in `host-tests/src/fs.rs`. Add new shared modules there instead of copying code. Keep hardware-only code behind `#[cfg(target_os = "none")]` (see `src/drivers/ps2.rs` and `ata.rs`); `arch::x86_64::cpu` and frame-allocation shims live in `lib.rs`.

## Architecture notes

- Boot: `_start` in `src/arch/x86_64/boot.S` → `rust_main` in `src/main.rs`; linked at 1 MiB by `linker.ld`.
- Modules: `arch/x86_64` (CPU, GDT/IDT/TSS, PIC/PIT, serial/VGA, Multiboot2, syscall entry), `mm` (refcounted frame allocator for the first 1 GiB, heap, 4 KiB paging, per-process address spaces, COW, `uaccess`), `proc` (preemptive round-robin scheduler, 100 ms timeslice, PID 1 protection, orphan reparenting, pipe wait queues, Unix signals), `fs` (writable ext2 + VFS over ATA PIO/MBR, pipes, staged ELF loader), `init` (early command line, `kernel_init`, init discovery), `syscall`, `sync`, `drivers`.
- ELF loading lives in `src/fs/binfmt_elf.rs` (`ExecImage`); user-pointer validation and the fixed address layout live in `src/mm/uaccess.rs`. There is no user-space source, `build.rs`, or disk-image recipe in this repository.
- The kernel has zero dependencies by design and uses `alloc`, a custom `SpinLock`/heap, and `panic = "abort"`. The target disables MMX/SSE (soft-float) and the red zone: no FP/SIMD or std-only crates.
- `make image` deliberately runs `objcopy -I elf64-x86-64 -O elf32-i386` on the kernel for GRUB; do not remove it.
- User space is fixed and tiny: one PML4 slot, a 64 KiB program window at `0x80_0000_0000`, a read-only/executable `rt_sigreturn` trampoline page at `0x80_0010_0000`, and a 16 KiB stack below `0x80_0020_0000`. The root filesystem is supplied externally through `ROOTFS`.
- PID 1 is `kernel_init`, the first spawned kernel thread. It tries `init=` first, then `/sbin/init`, `/etc/init`, `/bin/init`, and `/bin/sh`, and panics if none runs. `kernel_execve` replaces the current address space in place, preserving the process slot, PID, parent link, and file descriptors. Orphans are reparented to PID 1, and exiting PID 1 panics.
- Syscalls use the `syscall`/`sysretq` instructions (not `int 0x80`). The Rust syscall table/errno live in `src/syscall/abi.rs` (Linux x86-64 numbers, errno encoded negative) and are shared with host tests; external user programs hardcode the same numbers.
- Frames are refcounted (`retain_frame` per extra owner, `free_frame` decrements). `fork` maps user pages copy-on-write; write faults go through `AddressSpace::handle_cow_fault`.
- Pipes (`src/fs/pipe.rs`) are a 4096-byte ring behind a `SpinLock`, shared by a read and a write endpoint held in `Arc`s. `FdEntry::Pipe` clones share an endpoint, so `fork` inherits both ends and an end closes only when every descriptor is gone. The ring itself never blocks: `try_read`/`try_write` report `Empty`/`Full`, and `src/syscall/mod.rs` retries after `scheduler::block_on_pipe`.
- Each pipe id comes from a fixed scheduler table (`MAX_PIPES`) holding read/write `PidQueue`s; `scheduler::init` installs the wake/release hooks that endpoint drops call. `block_on_pipe` runs its readiness predicate under the scheduler lock with interrupts disabled (lock order scheduler → pipe), so a wakeup cannot be lost. Destructors that can re-enter the scheduler (pipe endpoint drops) must always run *outside* that lock: exit, `try_wait`, reaping, close, and fork rollback all detach the value first.
- A process closes its descriptors when it exits, not when its zombie is reaped, so a blocked reader sees end-of-file as soon as the last writer goes away (`exit_current` swaps in `FdTable::empty()` and drops the old table after unlocking).
- Forked children start *directly* in `fork_child_start`, not in `task_start`: `task_start` executes `sti` and `call`, and an IRQ taken there pushes a ~176-byte trap frame from the stack top onto the copied `SyscallFrame` at `top - FORK_FRAME_OFFSET` (`src/proc/context.rs`), corrupting the `sysretq` target. `context_switch` must also keep `TSS.rsp0` in sync (`gdt::set_rsp0`) on every switch.
- ext2 is mounted read-write from the external `ROOTFS` image; the kernel expects an MBR with an ext2 partition and mounts it via `fs::mount`. See [ext2 read-write](#ext2-read-write) for the on-disk ordering and flush policy.

### ext2 read-write

- Layout: `src/fs/ext2/mod.rs` (mount, allocation, file and namespace operations), `bitmap.rs` (block/inode bitmaps), `layout.rs` (group/block/inode geometry), `structs.rs` (superblock, group descriptor and inode parsing), `dir.rs` (directory-entry parse/insert/remove/rename), `inode.rs` (block-pointer encode/decode). Host tests include the same sources through `host-tests/src/fs.rs`; hardware-only code stays behind `#[cfg(target_os = "none")]`.
- Write-through ordering: write children before parents so a crash between writes never exposes an unallocated block or inode: block bitmap → zeroed/new data block → inode pointers/size → group descriptor → superblock. Use the existing helpers (`allocate_block`, `write_block`, `write_inode`, `write_group`, `write_superblock`) rather than raw sector writes.
- Flush policy: `ata::write_sectors` does not flush. Every mutating `Ext2` operation (`create_file`, `write_file`, `truncate_inode`, `mkdir`, `rmdir`, `unlink`, `rename`, `release_unlinked_inode`, `sync`) calls `device.flush()` once before returning, so one syscall costs one cache flush no matter how many blocks it wrote. `sync` additionally rewrites dirty group descriptors and the superblock. Do not reintroduce per-block flushes in the ATA driver.
- Directory indexing: `COMPAT_DIR_INDEX` is accepted so such volumes mount, but the driver neither reads nor maintains htree indexes; mutating a directory whose inode has `EXT2_INDEX_FL` fails with `Unsupported` instead of corrupting the index.
- Open-unlinked inodes: `Vfs` keeps one open count per inode, incremented by `open`/`fork` and decremented on close/exit. `unlink` and a replacing `rename` only drop the directory entry and link count; when the last link disappears while descriptors remain, the blocks and inode survive until the final `Vfs::close` calls `release_unlinked`. Releasing a still-linked or already-freed inode is a no-op, so the release path needs no pending-unlink list.
- Lock ordering: `SCHEDULER` → `VFS` → `ATA`/block cache. Never call into the VFS (including `fs::close_node`, `fs::retain_node` or a deferred unlink) while holding `SCHEDULER`: `fork` snapshots the inherited descriptor inodes, releases the scheduler lock, then retains them; exit and close detach their `FdTable`/`FdEntry` before calling the VFS. The scheduler lock is usually taken with interrupts disabled, so taking the VFS beneath it can deadlock against a preempted VFS holder.

### Unix signals

- The scheduler-free core lives in `src/proc/signal.rs`: signal numbers/names, `SigSet`, `SigAction` flags (`SA_RESTART`, `SA_SIGINFO`, `SA_ONSTACK`, `SA_NODEFER`, `SA_RESETHAND`, `SA_RESTORER`), `Siginfo`, `SigContext`, and the `SignalFrame` layout. Register snapshots cross the kernel/user boundary through `UserContext` in `src/arch/x86_64/frame.rs` (`from_syscall_frame`/`write_to_syscall_frame`, `from_trap_frame`/`write_to_trap_frame`, `to_sig_context`/`from_sig_context`).
- `binfmt_elf::build_image` maps one read-only/executable trampoline page at `uaccess::TRAMPOLINE_BASE = 0x0000_0080_0010_0000`, containing `mov rax, 15; syscall; ud2` (`SIGRETURN_NUMBER = 15`). A handler's return slot points at the trampoline; `rt_sigreturn` validates the saved `cs`/`ss`/`rip`/`rsp` and turns a corrupt frame into `SIGSEGV`.
- Delivery runs at both ring-3 exits: `syscall_dispatch` calls `scheduler::deliver_pending_signals` on the `sysretq` path via `UserContext::from_syscall_frame`, and `interrupt_dispatch` calls `deliver_pending` on the `iretq` path, guarded by `frame.cs & 3 == 3`, via `from_trap_frame`/`write_to_trap_frame`.
- Lock ordering and COW safety are critical. `plan_delivery` runs under `SCHEDULER`: it selects/prunes with `next_deliverable` and takes the `interrupted_syscall` record. The lock is then dropped *before* `SignalFrame::write_to`, because storing the frame can take a COW fault whose handler re-enters `SCHEDULER` (lock order scheduler → page tables). `commit_delivery` re-acquires `SCHEDULER` to clear the pending bit, bump `depth`, install `handler_mask`, and apply `SA_RESETHAND`. Never write user memory while holding the scheduler lock.
- Ring-3 CPU exceptions become signals for the current task via `scheduler::raise_current_signal`. `signal::hardware_fault_signal` maps `#DE`→`SIGFPE`/`FPE_INTDIV`, `#DB`→`SIGTRAP`/`TRAP_TRACE`, `#BP`→`SIGTRAP`/`TRAP_BRKPT`, `#UD`→`SIGILL`/`ILL_ILLOPC`, `#AC`→`SIGBUS`/`BUS_ADRALN`, and `#PF`→`SIGSEGV` with `si_addr = CR2` and `SEGV_MAPERR` (not present) vs `SEGV_ACCERR` (present). COW write faults resolve and retry first; ring-0 exceptions keep the fatal register dump/halt.
- Blocking syscalls are interruptible. `scheduler::pending_interrupting` skips blocked, `SIG_IGN` and default-ignored signals (default-ignored `SIGCHLD` must not break reads); `block_on_pipe`, `block_on_keyboard` and `block_on_child` re-check it under `SCHEDULER` to close the check/register race. Loops retry the I/O or `try_wait` first, then call `note_interrupted_syscall(number, restartable)` and return `-EINTR`. With `SA_RESTART`, delivery rewinds the saved context (`rip -= 2` for the two-byte `syscall` instruction, `rax = original syscall number`) so `rt_sigreturn` re-executes the call.
- `exit_current_status` marks the zombie and, under `SCHEDULER`, queues `SIGCHLD` with `Siginfo::child` (CLD_EXITED, or CLD_KILLED/CLD_DUMPED from `ExitStatus::child_info`) to the live original parent only. Orphans reparented to PID 1 get no synthetic `SIGCHLD`, and the `waitpid` wakeup is independent of the signal. A pipe write with no readers raises `SIGPIPE` and returns `-EPIPE`.
- `can_signal` protects PID 1: only PID 1 may signal itself; any other sender gets `SignalError::PermissionDenied` (`EPERM`). `SIGKILL`/`SIGSTOP` are never catchable, ignorable, or blockable.

## Conventions

- Commit subjects: `subsystem[, subsystem]: lowercase summary` (for example `mm, proc, fs: ...`).
- No CI or rustfmt/clippy config; match surrounding style and keep diffs focused.