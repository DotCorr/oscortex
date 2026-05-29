#!/usr/bin/env bash
# Run all OSCortex kernel tests (host unit + build + optional QEMU integration).
#
# Usage:
#   tests/run_all.sh           # unit + kernel cross-build
#   tests/run_all.sh --qemu    # unit + build + QEMU driver integration

set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
RUN_QEMU=false

for arg in "$@"; do
    case "$arg" in
        --qemu) RUN_QEMU=true ;;
        --help|-h)
            echo "Usage: tests/run_all.sh [--qemu]"
            exit 0
            ;;
    esac
done

echo "=== OSCortex kernel unit tests ==="
cargo test --manifest-path "$ROOT/tests/kernel/Cargo.toml"

echo "=== OSCortex kernel cross-build (x86_64-unknown-none) ==="
bash "$ROOT/tests/integration/kernel_build.sh"

if $RUN_QEMU; then
    if ! command -v qemu-system-x86_64 >/dev/null 2>&1; then
        echo "ERROR: qemu-system-x86_64 not found — skip integration or install QEMU" >&2
        exit 1
    fi
    echo "=== OSCortex QEMU driver integration ==="
    bash "$ROOT/tests/integration/qemu_drivers.sh" --build
else
    echo "Skipping QEMU integration (pass --qemu to enable)."
fi

echo "=== All requested tests passed ==="
