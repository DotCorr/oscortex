#!/bin/bash
# OSCortex QEMU Runner — graphical window + live serial log

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ISO="$SCRIPT_DIR/oscortex.iso"
SERIAL_LOG="/tmp/oscortex-serial.log"

# Kill any lingering QEMU that might hold the ISO lock
pkill -9 qemu-system-x86_64 2>/dev/null || true
sleep 0.3

if [ ! -f "$ISO" ]; then
    echo "ERROR: $ISO not found!"
    echo "Run: bash scripts/build-iso.sh"
    exit 1
fi

rm -f "$SERIAL_LOG"
touch "$SERIAL_LOG"

# Create a small virtio-blk disk image for ext2 testing (8 MiB).
VBLK="$SCRIPT_DIR/vdisk.img"
if [ ! -f "$VBLK" ]; then
    echo "[QEMU] Creating 8 MiB virtio-blk image: $VBLK"
    dd if=/dev/zero of="$VBLK" bs=1M count=8 2>/dev/null
fi

# Create a small NVMe disk image (16 MiB).
NVME="$SCRIPT_DIR/nvme.img"
if [ ! -f "$NVME" ]; then
    echo "[QEMU] Creating 16 MiB NVMe image: $NVME"
    dd if=/dev/zero of="$NVME" bs=1M count=16 2>/dev/null
fi

echo "=========================================="
echo "OSCortex Boot — graphical window + serial"
echo "=========================================="
echo "Serial log: $SERIAL_LOG"
echo "A QEMU window will open. Close it or Ctrl+C here to stop."
echo ""

# Real graphical window (Cocoa on macOS); readonly avoids lock conflicts.
## NOTE: multi-core is now SAFE (the SMP livelock is fixed — user threads run
## BSP-only so the cooperative scheduler's single-core assumption holds). We
## still default to smp=1: the secondary core only halts (no parallel scheduler
## yet), so it adds ISR overhead without speeding anything up. Bump to cpus=2+
## if you want to exercise the multi-core path — it renders correctly either way.
## Also: first frame takes ~60-90s (the engine JIT-compiles the whole Material
## framework on emulated bare metal). Be patient — the shell WILL appear.
qemu-system-x86_64 \
    -M q35 \
    -cpu qemu64,+x2apic \
    -smp cpus=1 \
    -m 2G \
    -cdrom "$ISO" \
    -boot d \
    -device virtio-net-pci,netdev=net0 \
    -netdev user,id=net0 \
    -device virtio-blk-pci,drive=vblk \
    -drive "file=$VBLK,format=raw,id=vblk,if=none" \
    -device nvme,drive=nvmedrive,serial=oscortex0 \
    -drive "file=$NVME,format=raw,id=nvmedrive,if=none" \
    -serial file:"$SERIAL_LOG" \
    -display cocoa \
    -vga std \
    -monitor unix:/tmp/qemu-monitor.sock,server,nowait \
    -no-reboot &

QEMU_PID=$!

sleep 0.5

tail -f "$SERIAL_LOG" &
TAIL_PID=$!

trap "kill $QEMU_PID $TAIL_PID 2>/dev/null; exit 0" INT TERM
wait $QEMU_PID 2>/dev/null || true
kill $TAIL_PID 2>/dev/null || true

echo ""
echo "=========================================="
echo "QEMU session ended"
echo "=========================================="

