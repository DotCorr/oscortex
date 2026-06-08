#!/usr/bin/env bash
# Build + boot the OSCortex aarch64 kernel directly under qemu-system-aarch64.
#
# This is the ARM bring-up path: a direct `-kernel` boot (no Limine) that runs
# the platform-primitive milestones — EL1, MMU, exception vectors, GIC + generic
# timer, SVC syscalls, and an EL0 user process — printing progress over PL011
# serial. See kernel/src/arch/aarch64/bringup*.rs.
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
[[ "${1:-}" == "smp2" ]] && SMP=2

echo "[run-aarch64] booting (smp=$SMP) — Ctrl-A X to quit ..."
exec qemu-system-aarch64 \
    -M virt -cpu cortex-a72 -smp "$SMP" -m 2G \
    -nographic -kernel "$KERNEL" \
    -serial mon:stdio
