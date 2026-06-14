#!/usr/bin/env bash
# build-iso-aarch64.sh — build a bootable aarch64 OSCortex ISO that boots via the
# Limine UEFI protocol (so it runs in UTM / on UEFI ARM machines), then optionally
# boot it under qemu-system-aarch64 with the edk2 AAVMF ARM UEFI firmware.
#
# This mirrors scripts/build-iso.sh's Limine ISO assembly, but for aarch64:
#   * kernel built with the `limine-boot` feature (higher-half link, Limine entry),
#   * BOOTAA64.EFI as the EFI bootloader,
#   * limine.conf with the OSCortex entry.
#
# The kernel embeds the `initramfs/` tree (via kernel/build.rs), including `/init`
# (the aarch64 Flutter shell host) and the aarch64 engine + libapp. That tree is
# SHARED with the x86 build (scripts/build-iso.sh), which overwrites it with x86
# binaries — so running the x86 build before this one leaves an x86 `/init` the
# aarch64 kernel can't spawn ("Failed to spawn /init: wrong ELF machine"), hanging
# the boot before the shell. The guard below fails loudly in that case so we never
# package a contaminated ISO; re-stage the aarch64 runtime first.
#
# Usage:
#   scripts/build-iso-aarch64.sh           # build the ISO
#   scripts/build-iso-aarch64.sh --run     # build + boot under QEMU UEFI (headless)
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
# Limine asset dir (limine-uefi-cd.bin, BOOTAA64.EFI). Defaults to the Homebrew
# location for local macOS builds; override with LIMINE_DIR=... on CI (the GitHub
# runner clones limine to $HOME/limine).
LIMINE_DIR="${LIMINE_DIR:-/opt/homebrew/share/limine}"
ISO_ROOT="$ROOT/iso_root_aarch64"
OUTPUT="$ROOT/oscortex-aarch64.iso"
TARGET="aarch64-unknown-none"
# edk2 AArch64 UEFI firmware shipped with Homebrew QEMU.
QEMU_SHARE="$(dirname "$(command -v qemu-system-aarch64)")/../share/qemu"
AAVMF_CODE="$QEMU_SHARE/edk2-aarch64-code.fd"

# Guard: refuse to build if the shared initramfs/ holds a wrong-arch /init
# (cross-arch contamination from an x86 build). Packaging it would hang the boot.
INIT_BIN="$ROOT/initramfs/init"
if [ -f "$INIT_BIN" ] && ! file -b "$INIT_BIN" | grep -qi "aarch64"; then
    echo "ERROR: $INIT_BIN is not an aarch64 binary — initramfs/ is contaminated:" >&2
    echo "       $(file -b "$INIT_BIN")" >&2
    echo "       Re-stage the aarch64 runtime (scripts/build-aarch64-shell.sh, or" >&2
    echo "       restore the arm64 shell-runtime tarball) before building the ISO." >&2
    exit 1
fi

echo "[1/4] Building aarch64 kernel ELF (Limine higher-half)..."
cd "$ROOT"
cargo build \
    --release \
    --package oscortex-kernel \
    --target "$TARGET" \
    --no-default-features \
    --features arch-aarch64,limine-boot \
    -Z build-std=core,compiler_builtins,alloc \
    -Z build-std-features=compiler-builtins-mem

KERNEL_ELF="$ROOT/target/$TARGET/release/kernel"
if [ ! -f "$KERNEL_ELF" ]; then
    echo "ERROR: kernel ELF not found: $KERNEL_ELF" >&2
    exit 1
fi

echo "[2/4] Staging ISO root..."
rm -rf "$ISO_ROOT"
mkdir -p "$ISO_ROOT/boot/limine"
mkdir -p "$ISO_ROOT/EFI/BOOT"

cat "$KERNEL_ELF" > "$ISO_ROOT/boot/kernel"

# Limine UEFI artifacts for aarch64: the CD EFI image + the AArch64 EFI app.
cp "$LIMINE_DIR/limine-uefi-cd.bin" "$ISO_ROOT/boot/limine/"
cp "$LIMINE_DIR/BOOTAA64.EFI"       "$ISO_ROOT/EFI/BOOT/"

cat > "$ISO_ROOT/boot/limine/limine.conf" << 'EOF'
# OSCortex aarch64 boot configuration (Limine UEFI)
timeout: 0
serial: yes
verbose: yes

/OSCortex AI-First Kernel
    protocol: limine
    path: boot():/boot/kernel
    kaslr: no
EOF

cp "$ISO_ROOT/boot/limine/limine.conf" "$ISO_ROOT/limine.conf"
cp "$ISO_ROOT/boot/limine/limine.conf" "$ISO_ROOT/EFI/BOOT/limine.conf"

echo "[3/4] Building ISO with xorriso (UEFI-only, no BIOS on ARM)..."
xorriso -as mkisofs \
    --efi-boot boot/limine/limine-uefi-cd.bin \
    -efi-boot-part --efi-boot-image --protective-msdos-label \
    "$ISO_ROOT" -o "$OUTPUT" 2>/dev/null

echo "[4/4] ISO built: $OUTPUT ($(du -sh "$OUTPUT" | cut -f1))"
echo ""
echo "Boot under QEMU UEFI (AArch64):"
echo "  qemu-system-aarch64 -M virt -cpu cortex-a72 -m 2G -smp 1 \\"
echo "    -bios $AAVMF_CODE \\"
echo "    -cdrom $OUTPUT -device ramfb \\"
echo "    -serial stdio -display none"
echo ""
echo "In UTM: create a new ARM64 VM, attach $OUTPUT as a CD/DVD, use the"
echo "edk2 AArch64 UEFI firmware (UTM default for ARM), boot."

if [[ "${1:-}" == "--run" ]]; then
    if [ ! -f "$AAVMF_CODE" ]; then
        echo "ERROR: AArch64 UEFI firmware not found: $AAVMF_CODE" >&2
        exit 1
    fi
    SERIAL_LOG="${SERIAL_LOG:-/tmp/limine-serial.log}"
    echo ""
    echo "Launching QEMU UEFI (headless, serial → $SERIAL_LOG)..."
    qemu-system-aarch64 \
        -M virt -cpu cortex-a72 -m 2G -smp 1 \
        -bios "$AAVMF_CODE" \
        -cdrom "$OUTPUT" \
        -device ramfb \
        -serial "file:$SERIAL_LOG" \
        -display none \
        -no-reboot \
        2>&1 &
    QEMU_PID=$!
    echo "QEMU PID $QEMU_PID — serial log: $SERIAL_LOG"
    wait "$QEMU_PID" || true
fi
