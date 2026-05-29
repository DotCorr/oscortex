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
    # Raw block image for kernel app_store (OSSTORE1 layout). Do not mkfs.ext2 —
    # that puts a filesystem at sector 0 and is unrelated to our block catalog.
fi

# Create a small NVMe disk image (16 MiB).
NVME="$ROOT/nvme.img"
if [ ! -f "$NVME" ]; then
    echo "[QEMU] Creating 16 MiB NVMe image: $NVME"
    dd if=/dev/zero of="$NVME" bs=1M count=16 2>/dev/null
fi

# Stale QEMU from a prior run holds an exclusive lock on vdisk.img.
if command -v lsof >/dev/null 2>&1 && lsof "$VBLK" >/dev/null 2>&1; then
    echo "[QEMU] vdisk.img is locked — stopping stale qemu-system-x86..."
    pkill -9 qemu-system-x86 2>/dev/null || true
    sleep 1
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
    -m 2048M \
    -smp 2 \
    -cpu qemu64,+x2apic \
    -machine q35 \
    -device virtio-net-pci,netdev=net0 \
    -netdev user,id=net0 \
    -device virtio-blk-pci,drive=vblk \
    -drive "file=$VBLK,format=raw,id=vblk,if=none" \
    -device nvme,drive=nvmedrive,serial=oscortex0 \
    -drive "file=$NVME,format=raw,id=nvmedrive,if=none" \
    -device qemu-xhci,id=xhci \
    -serial stdio \
    -no-reboot \
    $DISPLAY_FLAGS \
    $EXTRA
