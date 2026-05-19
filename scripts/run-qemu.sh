#!/usr/bin/env bash
# OSCortex QEMU launcher
# Usage: ./scripts/run-qemu.sh [--no-display] [--kvm]
#
# Creates disk images if they don't exist, then boots oscortex.iso.

set -e
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
ISO="$ROOT/oscortex.iso"

if [ ! -f "$ISO" ]; then
    echo "ISO not found. Building first..."
    bash "$ROOT/scripts/build-iso.sh"
fi

# Create a small virtio-blk disk image for ext2 testing (8 MiB).
VBLK="$ROOT/vdisk.img"
if [ ! -f "$VBLK" ]; then
    echo "[QEMU] Creating 8 MiB virtio-blk image: $VBLK"
    dd if=/dev/zero of="$VBLK" bs=1M count=8 2>/dev/null
    # mkfs.ext2 is needed to use the ext2 driver.  Skip silently if not present.
    if command -v mkfs.ext2 &>/dev/null; then
        mkfs.ext2 -b 1024 "$VBLK" &>/dev/null
        echo "[QEMU] Formatted $VBLK as ext2"
    fi
fi

# Create a small NVMe disk image (16 MiB).
NVME="$ROOT/nvme.img"
if [ ! -f "$NVME" ]; then
    echo "[QEMU] Creating 16 MiB NVMe image: $NVME"
    dd if=/dev/zero of="$NVME" bs=1M count=16 2>/dev/null
fi

DISPLAY_FLAGS="-display cocoa"
EXTRA=""

for arg in "$@"; do
    case "$arg" in
        --no-display) DISPLAY_FLAGS="-display none" ;;
        --kvm)        EXTRA="$EXTRA -accel kvm" ;;
    esac
done

echo "[QEMU] Booting $ISO ..."
exec qemu-system-x86_64 \
    -cdrom "$ISO" \
    -m 512M \
    -smp 2 \
    -cpu qemu64,+x2apic \
    -machine q35 \
    -device virtio-net-pci,netdev=net0 \
    -netdev user,id=net0 \
    -device virtio-blk-pci,drive=vblk \
    -drive "file=$VBLK,format=raw,id=vblk,if=none" \
    -device nvme,drive=nvmedrive,serial=oscortex0 \
    -drive "file=$NVME,format=raw,id=nvmedrive,if=none" \
    -serial stdio \
    -no-reboot \
    $DISPLAY_FLAGS \
    $EXTRA
