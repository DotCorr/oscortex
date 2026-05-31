#!/bin/bash
# Scratch script to test QEMU mouse input response matching run-qemu.sh configuration

SERIAL_LOG="/tmp/osc_serial.log"
MONITOR_SOCK="/tmp/qemu-monitor.sock"
ISO="./oscortex.iso"
VBLK="./vdisk.img"
NVME="./nvme.img"

echo "1. Cleaning old log..."
rm -f "$SERIAL_LOG"

echo "2. Launching QEMU..."
pkill -9 qemu-system-x86_64 2>/dev/null || true
sleep 0.3

qemu-system-x86_64 \
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
    -serial file:"$SERIAL_LOG" \
    -no-reboot \
    -display cocoa \
    -monitor unix:/tmp/qemu-monitor.sock,server,nowait &

QEMU_PID=$!

echo "3. Waiting 25 seconds for boot..."
sleep 25

echo "4. Injecting mouse move relative events..."
python3 scratch/send_monitor_cmd.py "mouse_move 50 50" "mouse_move -20 10" "mouse_button 1" "mouse_button 0"
sleep 1.0

echo "7. Stopping QEMU..."
kill -INT $QEMU_PID 2>/dev/null
pkill -9 qemu-system-x86_64 2>/dev/null
sleep 1.0

echo "8. Dumping PS/2 and embedder entries from serial log..."
grep -i -E "PS2|mouse|pointer|embedder|wm-copy" "$SERIAL_LOG"
