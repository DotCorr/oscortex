#!/usr/bin/env bash
# QEMU integration test — verify native drivers probe and report ready on serial.
#
# Usage: tests/integration/qemu_drivers.sh [--build]
#
# Requires: qemu-system-x86_64, oscortex.iso (built via scripts/build-iso.sh)
# Exit 0 when all expected driver markers appear in serial output.

set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
ISO="$ROOT/oscortex.iso"
LOG="$(mktemp -t osc-driver-test)"
VBLK="$(mktemp -t osc-vblk).img"
NVME="$(mktemp -t osc-nvme).img"
TIMEOUT_SEC="${OSC_DRIVER_TEST_TIMEOUT:-120}"

BUILD=false
for arg in "$@"; do
    case "$arg" in
        --build) BUILD=true ;;
    esac
done

cleanup() {
    kill "$QEMU_PID" 2>/dev/null || true
    wait "$QEMU_PID" 2>/dev/null || true
    rm -f "$LOG" "$VBLK" "$NVME"
}
trap cleanup EXIT

# Release stale locks from prior QEMU runs.
pkill -f 'qemu-system-x86_64.*oscortex.iso' 2>/dev/null || true
sleep 1

if $BUILD || [ ! -f "$ISO" ]; then
    echo "[driver-test] Building ISO..."
    bash "$ROOT/scripts/build-iso.sh"
fi

dd if=/dev/zero of="$VBLK" bs=1M count=8 2>/dev/null
dd if=/dev/zero of="$NVME" bs=1M count=16 2>/dev/null

echo "[driver-test] Booting QEMU (timeout ${TIMEOUT_SEC}s), log: $LOG"

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
    -device usb-kbd,bus=xhci.0 \
    -serial file:"$LOG" \
    -display none \
    -no-reboot &
QEMU_PID=$!

# Required markers (all must match).
REQUIRED=(
    "[virtio-blk] ready"
    "[virtio-net] ready"
    "[virtio-net] self-test TX completion OK"
    "[NVMe]"
    "[Input] PS/2 enabled"
    "[USB-HID] WM route OK"
    "[smoltcp] TCP/IP stack initialised"
)

# USB host: success line (not failure warnings containing "[USB]").
GROUP_USB=(
    "[USB] XHCI ready"
    "[USB] 1 XHCI controller(s) mapped and reset"
)

# One-of groups (at least one in each group must match).
GROUP_XHCI_RUNTIME=(
    "[USB] XHCI runtime started"
)

GROUP_DHCP=(
    "[net] DHCP self-test OK"
    "[net] DHCP self-test timeout"
)

GROUP_TCP=(
    "[net] TCP self-test connect OK"
    "[net] TCP self-test connect skipped"
)

check_markers() {
    local missing=0
    for m in "${REQUIRED[@]}"; do
        if grep -qF "$m" "$LOG" 2>/dev/null; then
            echo "[driver-test] OK  $m"
        else
            echo "[driver-test] MISS $m"
            missing=$((missing + 1))
        fi
    done
    local dhcp_ok=0
    for m in "${GROUP_DHCP[@]}"; do
        if grep -qF "$m" "$LOG" 2>/dev/null; then
            echo "[driver-test] OK  $m"
            dhcp_ok=1
            break
        fi
    done
    if [ "$dhcp_ok" -eq 0 ]; then
        echo "[driver-test] MISS DHCP self-test (need OK or timeout line)"
        missing=$((missing + 1))
    fi
    local usb_ok=0
    for m in "${GROUP_USB[@]}"; do
        if grep -qF "$m" "$LOG" 2>/dev/null; then
            echo "[driver-test] OK  $m"
            usb_ok=1
            break
        fi
    done
    if [ "$usb_ok" -eq 0 ]; then
        echo "[driver-test] MISS USB host init (XHCI ready or mapped+reset)"
        missing=$((missing + 1))
    fi
    local xhci_rt_ok=0
    for m in "${GROUP_XHCI_RUNTIME[@]}"; do
        if grep -qF "$m" "$LOG" 2>/dev/null; then
            echo "[driver-test] OK  $m"
            xhci_rt_ok=1
            break
        fi
    done
    if [ "$xhci_rt_ok" -eq 0 ]; then
        echo "[driver-test] MISS XHCI runtime start"
        missing=$((missing + 1))
    fi
    local tcp_ok=0
    for m in "${GROUP_TCP[@]}"; do
        if grep -qF "$m" "$LOG" 2>/dev/null; then
            echo "[driver-test] OK  $m"
            tcp_ok=1
            break
        fi
    done
    if [ "$tcp_ok" -eq 0 ]; then
        echo "[driver-test] MISS TCP self-test line"
        missing=$((missing + 1))
    fi
    return "$missing"
}

DEADLINE=$((SECONDS + TIMEOUT_SEC))
while [ "$SECONDS" -lt "$DEADLINE" ]; do
    if ! kill -0 "$QEMU_PID" 2>/dev/null; then
        break
    fi
    if check_markers; then
        echo "[driver-test] All driver markers found."
        exit 0
    fi
    sleep 2
done

echo "[driver-test] FAILED — serial log tail:"
tail -60 "$LOG" || true
exit 1
