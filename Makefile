KERNEL_NAME := rinux
TARGET_DIR := target/x86_64-rinux/debug
KERNEL64 := $(TARGET_DIR)/$(KERNEL_NAME)
BUILD_DIR := build
ISO_ROOT := $(BUILD_DIR)/iso_root
ISO := $(BUILD_DIR)/rinux.iso
QEMU ?= qemu-system-x86_64
QEMU_FLAGS := -cdrom $(ISO) -m 256M -serial stdio -no-reboot -no-shutdown

.PHONY: all build image run debug check clean

all: image

build:
	cargo build

image: build
	rm -rf $(ISO_ROOT)
	mkdir -p $(ISO_ROOT)/boot/grub $(BUILD_DIR)
	objcopy -I elf64-x86-64 -O elf32-i386 $(KERNEL64) $(ISO_ROOT)/boot/rinux.elf
	cp grub/grub.cfg $(ISO_ROOT)/boot/grub/grub.cfg
	grub-mkrescue -o $(ISO) $(ISO_ROOT)

run: image
	$(QEMU) $(QEMU_FLAGS)

debug: image
	$(QEMU) $(QEMU_FLAGS) -s -S

check:
	cargo check

clean:
	cargo clean
	rm -rf $(BUILD_DIR)
	$(MAKE) -C host-tests clean