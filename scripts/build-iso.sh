#!/usr/bin/env bash
# build-iso.sh — build bootable OSCortex ISO
# Usage: ./scripts/build-iso.sh [--run]
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
LIMINE_DIR="/opt/homebrew/share/limine"
ISO_ROOT="$ROOT/iso_root"
OUTPUT="$ROOT/oscortex.iso"

# Phase 32-A: Build the flutter-embedder userspace binary if sources exist,
# then stage it into the initramfs directory before the kernel build.
EMBEDDER_DIR="$ROOT/tools/flutter-embedder"
EMBEDDER_TARGET="x86_64-unknown-none"

if [ -d "$EMBEDDER_DIR" ]; then
    echo "[0/4] Building flutter-embedder userspace binary..."
    (cd "$EMBEDDER_DIR" && \
        cargo +nightly build \
            --target "$EMBEDDER_TARGET" \
            -Z build-std=core,compiler_builtins \
            -Z build-std-features=compiler-builtins-mem \
            2>&1) || echo "[0/4] flutter-embedder build failed (non-fatal — kernel build continues)"

    EMBEDDER_BIN="$EMBEDDER_DIR/target/$EMBEDDER_TARGET/debug/flutter-embedder"
    if [ -f "$EMBEDDER_BIN" ]; then
        mkdir -p "$ROOT/initramfs/bin"
        cp "$EMBEDDER_BIN" "$ROOT/initramfs/bin/flutter-embedder"
        echo "[0/4] flutter-embedder staged to initramfs/bin/flutter-embedder"
    fi
fi

# Phase 42: Build the real /init userspace binary and stage it to initramfs.
INIT_DIR="$ROOT/userspace/init"
INIT_TARGET="x86_64-unknown-none"

if [ -d "$INIT_DIR" ]; then
    echo "[0.5/4] Building userspace/init ELF..."
    (cd "$INIT_DIR" && \
        cargo +nightly build \
            --release \
            --target "$INIT_TARGET" \
            -Z build-std=core,compiler_builtins \
            -Z build-std-features=compiler-builtins-mem \
            2>&1) || {
        echo "[0.5/4] WARNING: userspace/init build failed — falling back to placeholder"
    }

    INIT_BIN="$INIT_DIR/target/$INIT_TARGET/release/init"
    if [ -f "$INIT_BIN" ]; then
        mkdir -p "$ROOT/initramfs/bin"
        cp "$INIT_BIN" "$ROOT/initramfs/bin/init"
        # Copy to /init as a fallback in case launcher build is skipped or fails
        cp "$INIT_BIN" "$ROOT/initramfs/init"
        echo "[0.5/4] userspace/init staged to initramfs/bin/init and /init fallback ($(wc -c < "$INIT_BIN") bytes)"
    fi
fi

# Phase 47: Build /bin/hello and stage it to initramfs.
HELLO_DIR="$ROOT/userspace/hello"
HELLO_TARGET="x86_64-unknown-none"

if [ -d "$HELLO_DIR" ]; then
    echo "[0.6/4] Building userspace/hello ELF..."
    (cd "$HELLO_DIR" && \
        cargo +nightly build \
            --release \
            --target "$HELLO_TARGET" \
            -Z build-std=core,compiler_builtins \
            -Z build-std-features=compiler-builtins-mem \
            2>&1) || {
        echo "[0.6/4] WARNING: userspace/hello build failed (non-fatal)"
    }

    HELLO_BIN="$HELLO_DIR/target/$HELLO_TARGET/release/hello"
    if [ -f "$HELLO_BIN" ]; then
        mkdir -p "$ROOT/initramfs/bin"
        cp "$HELLO_BIN" "$ROOT/initramfs/bin/hello"
        echo "[0.6/4] userspace/hello staged to initramfs/bin/hello ($(wc -c < "$HELLO_BIN") bytes)"
    fi
fi

# Phase 62: Build and stage launcher as the primary /init (PID 1)
LAUNCHER_DIR="$ROOT/userspace/launcher"
LAUNCHER_TARGET="x86_64-unknown-none"
if [ -d "$LAUNCHER_DIR" ]; then
    echo "[0.7/4] Building userspace/launcher ELF..."
    (cd "$LAUNCHER_DIR" && \
        cargo +nightly build \
            --release \
            --target "$LAUNCHER_TARGET" \
            -Z build-std=core,compiler_builtins \
            -Z build-std-features=compiler-builtins-mem \
            2>&1) || {
        echo "[0.7/4] WARNING: userspace/launcher build failed"
    }

    LAUNCHER_BIN="$LAUNCHER_DIR/target/$LAUNCHER_TARGET/release/launcher"
    if [ -f "$LAUNCHER_BIN" ]; then
        cp "$LAUNCHER_BIN" "$ROOT/initramfs/init"
        echo "[0.7/4] userspace/launcher staged to initramfs/init ($(wc -c < "$LAUNCHER_BIN") bytes)"
    else
        echo "[0.7/4] WARNING: userspace/launcher binary not found, keeping fallback /init"
    fi
else
    echo "[0.7/4] WARNING: userspace/launcher directory not found, keeping fallback /init"
fi

# Phase 61: Build stub app (Files, Settings, Editor placeholders).
STUB_DIR="$ROOT/userspace/stub"
STUB_TARGET="x86_64-unknown-none"

if [ -d "$STUB_DIR" ]; then
    echo "[0.8/4] Building userspace/stub ELF..."
    (cd "$STUB_DIR" && \
        cargo +nightly build \
            --release \
            --target "$STUB_TARGET" \
            -Z build-std=core,compiler_builtins \
            -Z build-std-features=compiler-builtins-mem \
            2>&1) || {
        echo "[0.8/4] WARNING: userspace/stub build failed (non-fatal)"
    }

    STUB_BIN="$STUB_DIR/target/$STUB_TARGET/release/stub"
    if [ -f "$STUB_BIN" ]; then
        mkdir -p "$ROOT/initramfs/bin"
        for APP in files settings editor; do
            cp "$STUB_BIN" "$ROOT/initramfs/bin/$APP"
        done
        echo "[0.8/4] stub staged to initramfs/bin/{files,settings,editor} ($(wc -c < "$STUB_BIN") bytes each)"
    fi
fi

echo "[1/4] Building kernel ELF..."
cd "$ROOT"
cargo +nightly build \
    --package oscortex-kernel \
    --target x86_64-unknown-none \
    -Z build-std=core,compiler_builtins,alloc \
    -Z build-std-features=compiler-builtins-mem

KERNEL_ELF="$ROOT/target/x86_64-unknown-none/debug/kernel"

echo "[2/4] Staging ISO root..."
rm -rf "$ISO_ROOT"
mkdir -p "$ISO_ROOT/boot/limine"
mkdir -p "$ISO_ROOT/EFI/BOOT"

# Copy kernel
cp "$KERNEL_ELF" "$ISO_ROOT/boot/kernel"

# Copy Limine boot files
cp "$LIMINE_DIR/limine-bios-cd.bin" "$ISO_ROOT/boot/limine/"
cp "$LIMINE_DIR/limine-bios.sys"    "$ISO_ROOT/boot/limine/"
cp "$LIMINE_DIR/limine-uefi-cd.bin" "$ISO_ROOT/boot/limine/"
cp "$LIMINE_DIR/BOOTX64.EFI"        "$ISO_ROOT/EFI/BOOT/"
cp "$LIMINE_DIR/BOOTIA32.EFI"       "$ISO_ROOT/EFI/BOOT/"

# Write Limine configuration (Limine 9+ format)
cat > "$ISO_ROOT/boot/limine/limine.conf" << 'EOF'
# OSCortex boot configuration
timeout: 0
serial: yes
verbose: yes

/OSCortex AI-First Kernel
    protocol: limine
    path: boot():/boot/kernel
    kaslr: no
    module_path: boot():/boot/libflutter_engine.so
    module_cmdline: libflutter_engine.so
EOF

# Stage Flutter engine as a Limine module (so the kernel can dlopen it at runtime).
FLUTTER_ENGINE_SO="$ROOT/tools/flutter-engine/libflutter_engine.so"
if [ -f "$FLUTTER_ENGINE_SO" ]; then
    cp "$FLUTTER_ENGINE_SO" "$ISO_ROOT/boot/libflutter_engine.so"
    echo "[2/4] Staged libflutter_engine.so ($(du -sh "$FLUTTER_ENGINE_SO" | cut -f1)) as Limine module"
else
    echo "[2/4] WARNING: tools/flutter-engine/libflutter_engine.so not found — engine will not load"
fi

# Some UEFI firmwares load Limine from EFI removable path and expect config
# near the loader/root. Keep mirrored copies for maximum compatibility.
cp "$ISO_ROOT/boot/limine/limine.conf" "$ISO_ROOT/limine.conf"
cp "$ISO_ROOT/boot/limine/limine.conf" "$ISO_ROOT/EFI/BOOT/limine.conf"

echo "[3/4] Building ISO with xorriso..."
xorriso -as mkisofs \
    -b boot/limine/limine-bios-cd.bin \
    -no-emul-boot -boot-load-size 4 -boot-info-table \
    --efi-boot boot/limine/limine-uefi-cd.bin \
    -efi-boot-part --efi-boot-image --protective-msdos-label \
    "$ISO_ROOT" -o "$OUTPUT" 2>/dev/null

# Install Limine BIOS stage 2
limine bios-install "$OUTPUT" 2>/dev/null || true

echo "[4/4] ISO built: $OUTPUT ($(du -sh "$OUTPUT" | cut -f1))"

if [[ "${1:-}" == "--run" ]]; then
    echo ""
    echo "Launching QEMU... (serial output on stdout, Ctrl+A X to quit)"
    echo ""
    qemu-system-x86_64 \
        -cdrom "$OUTPUT" \
        -cpu qemu64,+x2apic \
        -m 512M \
        -smp 2 \
        -serial stdio \
        -display none \
        -no-reboot \
        -d int,cpu_reset \
        -D "$ROOT/qemu-log.txt" \
        2>&1
fi

