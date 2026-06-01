#!/usr/bin/env bash
set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

echo "=== Fast Kernel ISO Build ==="
cd "$ROOT"

cargo +nightly build \
    --release \
    --package oscortex-kernel \
    --target x86_64-unknown-none \
    -Z build-std=core,compiler_builtins,alloc \
    -Z build-std-features=compiler-builtins-mem

KERNEL_ELF="$ROOT/target/x86_64-unknown-none/release/kernel"
cp "$KERNEL_ELF" "$ROOT/iso_root/boot/kernel"

echo "Rebuilding ISO..."
xorriso -as mkisofs \
    -b boot/limine/limine-bios-cd.bin \
    -no-emul-boot -boot-load-size 4 -boot-info-table \
    --efi-boot boot/limine/limine-uefi-cd.bin \
    -efi-boot-part --efi-boot-image --protective-msdos-label \
    "$ROOT/iso_root" -o "$ROOT/oscortex.iso" 2>/dev/null

limine bios-install "$ROOT/oscortex.iso" 2>/dev/null || true
echo "Fast build complete!"
