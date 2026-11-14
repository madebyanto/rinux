#!/usr/bin/env bash
set -e

cd "$(dirname "$0")"

mkdir -p build

create_rootfs() {
    echo "=========================="
    echo " Creating build/rootfs.img"
    echo "=========================="
    
    dd if=/dev/zero of=build/rootfs.img bs=1M count=32 status=none
    
    # 2. Partition with sfdisk (MBR, partition 1 at LBA 2048, type 83)
    echo "label: dos" | sfdisk build/rootfs.img >/dev/null 2>&1
    echo "start=2048, type=83" | sfdisk -N1 build/rootfs.img >/dev/null 2>&1
    
    echo "Formatting ext2 partition (requires sudo)..."
    sudo sh -c '
        LOOP_DEV=$(losetup -fP --show build/rootfs.img)
        mkfs.ext2 -b 1024 -I 128 -L "rinux-root" -m 0 "${LOOP_DEV}p1"
        
        mkdir -p mnt_rootfs
        mount "${LOOP_DEV}p1" mnt_rootfs
        mkdir -p mnt_rootfs/sbin mnt_rootfs/bin mnt_rootfs/etc
        
        # Placeholder /sbin/init if none exists yet
        if [ ! -f mnt_rootfs/sbin/init ]; then
            touch mnt_rootfs/sbin/init
            chmod +x mnt_rootfs/sbin/init
        fi
        
        umount mnt_rootfs
        rmdir mnt_rootfs
        losetup -d "$LOOP_DEV"
    '
    echo "rootfs.img created successfully at build/rootfs.img!"
}

while true; do
    echo "=========================="
    echo " Rinux build script"
    echo "=========================="
    echo "1) Compile only"
    echo "2) Run existing image"
    echo "3) Compile, create rootfs & run"
    echo "4) Exit"
    echo
    read -rp "Choice: " choice
    echo

    case "$choice" in
        1)
            cargo clean
            cargo build
            make image
            ;;
        2)
            if [ ! -f build/rinux.iso ] || [ ! -f build/rootfs.img ]; then
                echo "Missing build/rinux.iso or build/rootfs.img. Run option 3 first."
            else
                make run ROOTFS=build/rootfs.img
            fi
            ;;
        3)
            cargo clean
            cargo build
            make image
            
            if [ ! -f build/rootfs.img ]; then
                create_rootfs
            else
                echo "Using existing build/rootfs.img."
            fi
            
            make run ROOTFS=build/rootfs.img
            ;;
        4)
            exit 0
            ;;
        *)
            echo "Invalid option."
            ;;
    esac

    echo
    read -rp "Press Enter to return to the menu..."
done