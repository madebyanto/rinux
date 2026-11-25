#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "$0")"

build_dir=build
rootfs_img="$build_dir/rootfs.img"
rootfs_mnt="$build_dir/mnt_rootfs"
rootfs_loop="$build_dir/rootfs.loop"

mkdir -p "$build_dir"

rootfs_mounted() {
    mountpoint -q "$rootfs_mnt"
}

rootfs_attached() {
    [ -f "$rootfs_loop" ]
}

attach_rootfs() {
    local loop_dev

    if rootfs_attached; then
        echo "rootfs.img already attached to $(cat "$rootfs_loop")."
        return 0
    fi

    loop_dev=$(sudo losetup --find --partscan --show "$rootfs_img")
    echo "$loop_dev" > "$rootfs_loop"
    sudo udevadm settle >/dev/null 2>&1 || true
}

detach_rootfs() {
    if rootfs_attached; then
        sudo losetup --detach "$(cat "$rootfs_loop")"
        rm -f "$rootfs_loop"
    fi
}

mount_rootfs() {
    if rootfs_mounted; then
        echo "rootfs already mounted on $rootfs_mnt."
        return 0
    fi

    attach_rootfs
    sudo mkdir -p "$rootfs_mnt"
    sudo mount "$(cat "$rootfs_loop")p1" "$rootfs_mnt"
    echo "rootfs mounted on $rootfs_mnt."
}

unmount_rootfs() {
    if rootfs_mounted; then
        sync
        sudo umount "$rootfs_mnt" || {
            echo "Cannot unmount $rootfs_mnt: device busy."
            return 1
        }
    fi

    detach_rootfs
    echo "rootfs unmounted."
}

create_rootfs() {
    echo "=========================="
    echo " Creating $rootfs_img"
    echo "=========================="
    dd if=/dev/zero of="$rootfs_img" bs=1M count=32 status=none
    echo "label: dos" | sfdisk "$rootfs_img" >/dev/null 2>&1
    echo "start=2048, type=83" | sfdisk -N1 "$rootfs_img" >/dev/null 2>&1

    attach_rootfs
    echo "Formatting ext2 partition (requires sudo)..."
    sudo mkfs.ext2 -b 1024 -I 128 -L "rinux-root" -m 0 "$(cat "$rootfs_loop")p1"

    mount_rootfs
    sudo mkdir -p "$rootfs_mnt/sbin" "$rootfs_mnt/bin" "$rootfs_mnt/etc"
    unmount_rootfs

    echo "rootfs.img created successfully at $rootfs_img!"
}

run_vm() {
    unmount_rootfs
    make run ROOTFS="$rootfs_img"
}

cleanup() {
    unmount_rootfs >/dev/null 2>&1 || true
}

trap cleanup EXIT

while true; do
    echo "=========================="
    echo " Rinux build script"
    echo "=========================="
    echo "1) Compile only"
    echo "2) Run existing image"
    echo "3) Compile, create rootfs & run"
    echo "4) Mount existing rootfs"
    echo "5) Unmount rootfs"
    echo "6) Exit"
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
            if [ ! -f build/rinux.iso ] || [ ! -f "$rootfs_img" ]; then
                echo "Missing build/rinux.iso or $rootfs_img. Run option 3 first."
            else
                run_vm
            fi
            ;;
        3)
            cargo clean
            cargo build
            make image
            if [ ! -f "$rootfs_img" ]; then
                create_rootfs
            else
                echo "Using existing $rootfs_img."
            fi
            run_vm
            ;;
        4)
            if [ ! -f "$rootfs_img" ]; then
                echo "Missing $rootfs_img. Run option 3 first."
            else
                mount_rootfs || true
            fi
            ;;
        5)
            unmount_rootfs || true
            ;;
        6)
            exit 0
            ;;
        *)
            echo "Invalid option."
            ;;
    esac

    echo
    read -rp "Press Enter to return to the menu..."
done