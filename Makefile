KERNEL_NAME := rinux
TARGET_DIR := target/x86_64-rinux/debug
KERNEL64 := $(TARGET_DIR)/$(KERNEL_NAME)
BUILD_DIR := build
ISO_ROOT := $(BUILD_DIR)/iso_root
ISO := $(BUILD_DIR)/rinux.iso
DISK_IMG := $(BUILD_DIR)/disk.img
DISK_PART := $(BUILD_DIR)/disk-part.img
DISK_ROOT := $(BUILD_DIR)/disk_root
DISK_START_LBA := 2048
DISK_SECTORS := 24576
QEMU ?= qemu-system-x86_64
QEMU_FLAGS := -boot order=d -cdrom $(ISO) \
	-drive file=$(DISK_IMG),format=raw,if=ide,index=0,media=disk \
	-m 256M -serial stdio -no-reboot -no-shutdown

.PHONY: all build image run debug check clean

all: image

build:
	cargo build

$(DISK_IMG): build
	mkdir -p $(DISK_ROOT)/sub $(DISK_ROOT)/bin $(BUILD_DIR)
	printf 'hello from ext2\n' > $(DISK_ROOT)/hello.txt
	printf 'rinux ext2 demo\n' > $(DISK_ROOT)/readme.txt
	printf 'nested file\n' > $(DISK_ROOT)/sub/nested.txt
	cp $(BUILD_DIR)/user/bin/init.elf $(DISK_ROOT)/bin/init.elf
	cp $(BUILD_DIR)/user/bin/echo.elf $(DISK_ROOT)/bin/echo.elf
	truncate -s 32M $(DISK_IMG)
	printf 'label: dos\nstart=$(DISK_START_LBA), size=$(DISK_SECTORS), type=83\n' | sfdisk $(DISK_IMG)
	truncate -s 12M $(DISK_PART)
	mke2fs -t ext2 -L rinux -F -d $(DISK_ROOT) $(DISK_PART)
	dd if=$(DISK_PART) of=$(DISK_IMG) bs=512 seek=$(DISK_START_LBA) conv=notrunc status=none
	rm -f $(DISK_PART)

image: build $(DISK_IMG)
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
