KERNEL_NAME := rinux
PROFILE ?= debug
TARGET_DIR := target/x86_64-rinux/$(PROFILE)
KERNEL64 := $(TARGET_DIR)/$(KERNEL_NAME)
BUILD_DIR := build
KERNEL_ELF := $(BUILD_DIR)/rinux.elf
ISO_ROOT := $(BUILD_DIR)/iso_root
ISO := $(BUILD_DIR)/rinux.iso

# The root filesystem is produced by the separate userspace repository.
ROOTFS ?= $(BUILD_DIR)/rootfs.img

# Kernel command line injected into grub.cfg.  Empty by default so the kernel
# exercises the standard init fallback path.
KERNEL_CMDLINE ?= root=/dev/hda1
CMDLINE_ESCAPED := $(subst &,\&,$(KERNEL_CMDLINE))

QEMU ?= qemu-system-x86_64
QEMU_FLAGS := -boot order=d -cdrom $(ISO) \
	-m 256M -serial stdio -no-reboot -no-shutdown
QEMU_DISK := -drive file=$(ROOTFS),format=raw,if=ide,index=0,media=disk

ifeq ($(PROFILE),release)
CARGO_FLAGS := --release
else
CARGO_FLAGS :=
endif

.PHONY: all build kernel image iso run debug check check-rootfs clean

all: image

build:
	cargo build $(CARGO_FLAGS)

$(KERNEL_ELF): build
	@mkdir -p $(BUILD_DIR)
	objcopy -I elf64-x86-64 -O elf32-i386 $(KERNEL64) $(KERNEL_ELF)

kernel: $(KERNEL_ELF)

$(ISO): $(KERNEL_ELF)
	rm -rf $(ISO_ROOT)
	mkdir -p $(ISO_ROOT)/boot/grub
	cp $(KERNEL_ELF) $(ISO_ROOT)/boot/rinux.elf
	sed 's|@KERNEL_CMDLINE@|$(CMDLINE_ESCAPED)|' \
		grub/grub.cfg.in > $(ISO_ROOT)/boot/grub/grub.cfg
	grub-mkrescue -o $(ISO) $(ISO_ROOT)

image: $(ISO)

check-rootfs:
	@test -f "$(ROOTFS)" || { \
		echo "*** ROOTFS is required: set ROOTFS=/path/to/rootfs.img" >&2; \
		exit 1; \
	}

run: image check-rootfs
	$(QEMU) $(QEMU_FLAGS) $(QEMU_DISK)

debug: image check-rootfs
	$(QEMU) $(QEMU_FLAGS) $(QEMU_DISK) -s -S

check:
	cargo check

clean:
	cargo clean
	rm -rf $(BUILD_DIR)
