#!/usr/bin/env bash
set -e

cd "$(dirname "$0")"

while true; do
    echo "=========================="
    echo " Rinux build script"
    echo "=========================="
    echo "1) Compile only"
    echo "2) Run existing image"
    echo "3) Compile and run"
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
            if [ ! -f build/rinux.iso ]; then
                echo "No image found in build/rinux.iso, run option 1 or 3 first."
            else
                make run
            fi
            ;;
        3)
            cargo clean
            cargo build
            make image
            make run
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