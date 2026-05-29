#!/usr/bin/env bash
# Verify kernel cross-build on the host (x86_64-unknown-none).
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$ROOT"

echo "[kernel-build] Building oscortex-kernel for x86_64-unknown-none..."
cargo +nightly build \
    --release \
    --package oscortex-kernel \
    --target x86_64-unknown-none \
    -Z build-std=core,compiler_builtins,alloc \
    -Z build-std-features=compiler-builtins-mem

ELF="$ROOT/target/x86_64-unknown-none/release/kernel"
if [ ! -f "$ELF" ]; then
    echo "[kernel-build] FAILED — kernel ELF missing: $ELF" >&2
    exit 1
fi

echo "[kernel-build] OK — $(file "$ELF" | head -1)"
