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

echo "=========================================="
echo "OSCortex Boot — graphical window + serial"
echo "=========================================="
echo "Serial log: $SERIAL_LOG"
echo "A QEMU window will open. Close it or Ctrl+C here to stop."
echo ""

# Real graphical window (Cocoa on macOS); readonly avoids lock conflicts.
## NOTE: smp=1 (single core) is REQUIRED for now — multi-core hits an SMP
## scheduler/sync race that livelocks the Flutter engine threads. Single core
## runs the cooperative path cleanly and renders the UI reliably.
## Also: first frame takes ~60-90s (the engine JIT-compiles the whole Material
## framework on emulated bare metal). Be patient — the shell WILL appear.
qemu-system-x86_64 \
    -M q35 \
    -cpu qemu64,+x2apic \
    -smp cpus=1 \
    -m 2G \
    -cdrom "$ISO" \
    -boot d \
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

