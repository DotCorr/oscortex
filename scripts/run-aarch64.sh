#!/usr/bin/env bash
# Build + boot the OSCortex aarch64 kernel directly under qemu-system-aarch64.
#
# Direct `-kernel` boot (no Limine — that is the x86_64 boot protocol). The ARM
# `_start` (kernel/src/arch/aarch64/boot.rs) saves the device-tree pointer from
# x0 and routes into the PRODUCTION boot path (boot_prod.rs): PL011 serial → MMU
# → EL1 exception vectors → GIC → device-tree RAM discovery → the SHARED
# `kernel_main`. That runs the real subsystem init (heap, scheduler, VFS, process
# table, Cortex runtime, …) and spawns the `/init` process at EL0, whose syscalls
# are serviced through the shared dispatcher — all over PL011 serial.
#
# The self-contained platform-primitive bring-up (EL1/MMU/vectors/GIC/timer/SVC/
# EL0 self-test, kernel/src/arch/aarch64/bringup*.rs) is still present and was the
# proving ground for these primitives; the production path above now reuses them.
#
# Usage:
#   scripts/run-aarch64.sh           # single-core boot
#   scripts/run-aarch64.sh smp2      # 2 cores (exercises the PSCI CPU_ON path)
set -euo pipefail

cd "$(dirname "$0")/.."

TARGET=aarch64-unknown-none
KERNEL=target/$TARGET/debug/kernel

echo "[run-aarch64] building kernel for $TARGET ..."
cargo build --target "$TARGET" -p oscortex-kernel \
    --no-default-features --features arch-aarch64 \
    -Z build-std=core,compiler_builtins,alloc \
    -Z build-std-features=compiler-builtins-mem

SMP=1
DISPLAY_MODE="${DISPLAY_MODE:-none}"   # none | gtk | cocoa | sdl
for arg in "$@"; do
    case "$arg" in
        smp2)          SMP=2 ;;
        gtk|cocoa|sdl) DISPLAY_MODE="$arg" ;;
        graphical)     DISPLAY_MODE="cocoa" ;;  # macOS default backend
    esac
done

# `-device ramfb`: QEMU's software framebuffer, configured by the guest through
# fw_cfg (kernel/src/arch/aarch64/ramfb.rs). With DISPLAY_MODE=none the kernel
# still drives the buffer (capture it via the QEMU monitor `screendump`); a real
# display backend (cocoa/gtk/sdl) shows it on screen.
COMMON=(
    -M virt -cpu cortex-a72 -smp "$SMP" -m 2G
    -kernel "$KERNEL"
    -device ramfb
)

if [[ "$DISPLAY_MODE" == "none" ]]; then
    echo "[run-aarch64] booting (smp=$SMP, headless ramfb) — Ctrl-A X to quit ..."
    echo "[run-aarch64] monitor: /tmp/qemu-arm-monitor.sock (screendump to capture the fb)"
    exec qemu-system-aarch64 "${COMMON[@]}" \
        -display none \
        -serial mon:stdio \
        -monitor unix:/tmp/qemu-arm-monitor.sock,server,nowait
else
    echo "[run-aarch64] booting (smp=$SMP, display=$DISPLAY_MODE) — close window or Ctrl-A X to quit ..."
    exec qemu-system-aarch64 "${COMMON[@]}" \
        -display "$DISPLAY_MODE" \
        -serial mon:stdio
fi
