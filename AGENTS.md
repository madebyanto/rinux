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
- Prerequisites beyond the README: binutils (`objcopy`), `grub-mkrescue` + `xorriso`, QEMU, and nightly with `rust-src` (see `rust-toolchain.toml`). `mke2fs` is needed only by `ext2_tests`.
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
- `ext2_tests` requires `mke2fs`; no other test needs QEMU or real hardware.
- `host-tests/src/lib.rs` includes kernel sources via `#[path = "../../src/..."]`; generic fs modules are listed in `host-tests/src/fs.rs`. Add new shared modules there instead of copying code. Keep hardware-only code behind `#[cfg(target_os = "none")]` (see `src/drivers/ps2.rs` and `ata.rs`); `arch::x86_64::cpu` and frame-allocation shims live in `lib.rs`.

## Architecture notes

- Boot: `_start` in `src/arch/x86_64/boot.S` → `rust_main` in `src/main.rs`; linked at 1 MiB by `linker.ld`.
- Modules: `arch/x86_64` (CPU, GDT/IDT/TSS, PIC/PIT, serial/VGA, Multiboot2, syscall entry), `mm` (refcounted frame allocator for the first 1 GiB, heap, 4 KiB paging, per-process address spaces, COW, `uaccess`), `proc` (preemptive round-robin scheduler, 100 ms timeslice, PID 1 protection, orphan reparenting), `fs` (read-only ext2 + VFS over ATA PIO/MBR, staged ELF loader), `init` (early command line, `kernel_init`, init discovery), `syscall`, `sync`, `drivers`.
- ELF loading lives in `src/fs/binfmt_elf.rs` (`ExecImage`); user-pointer validation and the fixed address layout live in `src/mm/uaccess.rs`. There is no user-space source, `build.rs`, or disk-image recipe in this repository.
- The kernel has zero dependencies by design and uses `alloc`, a custom `SpinLock`/heap, and `panic = "abort"`. The target disables MMX/SSE (soft-float) and the red zone: no FP/SIMD or std-only crates.
- `make image` deliberately runs `objcopy -I elf64-x86-64 -O elf32-i386` on the kernel for GRUB; do not remove it.
- User space is fixed and tiny: one PML4 slot, a 64 KiB program window at `0x80_0000_0000`, and a 16 KiB stack below `0x80_0020_0000`. The root filesystem is supplied externally through `ROOTFS`.
- PID 1 is `kernel_init`, the first spawned kernel thread. It tries `init=` first, then `/sbin/init`, `/etc/init`, `/bin/init`, and `/bin/sh`, and panics if none runs. `kernel_execve` replaces the current address space in place, preserving the process slot, PID, parent link, and file descriptors. Orphans are reparented to PID 1, and exiting PID 1 panics.
- Syscalls use the `syscall`/`sysretq` instructions (not `int 0x80`). The Rust syscall table/errno live in `src/syscall/abi.rs` (Linux x86-64 numbers, errno encoded negative) and are shared with host tests; external user programs hardcode the same numbers.
- Frames are refcounted (`retain_frame` per extra owner, `free_frame` decrements). `fork` maps user pages copy-on-write; write faults go through `AddressSpace::handle_cow_fault`.
- ext2 is read-only (writes return `EROFS`). The guest disk is the external `ROOTFS` image; the kernel expects an MBR with an ext2 partition and mounts it via `fs::mount`.

## Conventions

- Commit subjects: `subsystem[, subsystem]: lowercase summary` (for example `mm, proc, fs: ...`).
- No CI or rustfmt/clippy config; match surrounding style and keep diffs focused.