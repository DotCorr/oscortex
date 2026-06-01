#!/usr/bin/env bash
# build-iso.sh — build bootable OSCortex ISO
# Usage: ./scripts/build-iso.sh [--run]
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
LIMINE_DIR="/opt/homebrew/share/limine"
ISO_ROOT="$ROOT/iso_root"
OUTPUT="$ROOT/oscortex.iso"
USERSPACE_TARGET="x86_64-unknown-none"
FLUTTER_ENGINE_SO="$ROOT/tools/flutter-engine/libflutter_engine.so"

echo "[0/5] Building PID 1 supervisor (userspace/init)..."
INIT_DIR="$ROOT/userspace/init"
(
    cd "$INIT_DIR"
    cargo +nightly build \
        --release \
        --target "$USERSPACE_TARGET" \
        -Z build-std=core,compiler_builtins \
        -Z build-std-features=compiler-builtins-mem \
        2>&1
)
INIT_BIN="$INIT_DIR/target/$USERSPACE_TARGET/release/init"
if [ ! -f "$INIT_BIN" ]; then
    echo "ERROR: init binary not found: $INIT_BIN" >&2
    exit 1
fi
mkdir -p "$ROOT/initramfs"
cp "$INIT_BIN" "$ROOT/initramfs/bin/init-supervisor"
echo "[0/5] Staged init supervisor to initramfs/bin/init-supervisor (future multi-app respawn)"

echo "[0.1/5] Building Flutter host (tools/flutter-embedder → oscortex-host)..."
EMBEDDER_DIR="$ROOT/tools/flutter-embedder"
(
    cd "$EMBEDDER_DIR"
    cargo +nightly build \
        --release \
        --target "$USERSPACE_TARGET" \
        -Z build-std=core,compiler_builtins \
        -Z build-std-features=compiler-builtins-mem \
        2>&1
)
HOST_BIN="$EMBEDDER_DIR/target/$USERSPACE_TARGET/release/oscortex-host"
if [ ! -f "$HOST_BIN" ]; then
    echo "ERROR: oscortex-host binary not found: $HOST_BIN" >&2
    exit 1
fi
mkdir -p "$ROOT/initramfs/bin"
cp "$HOST_BIN" "$ROOT/initramfs/bin/oscortex-host"
# Flutter task runners assume the shell host is PID 1 (matches working run-visible.log).
cp "$HOST_BIN" "$ROOT/initramfs/init"
echo "[0.1/5] Staged oscortex-host as initramfs/init (PID 1 shell) and bin/oscortex-host"

# Sync Flutter shell assets into initramfs.
APP_DIR="$ROOT/apps/oscortex_app"
APP_ASSETS_DIR="$APP_DIR/build/flutter_assets"
if [ -d "$APP_DIR" ]; then
    echo "[0.2/5] Building oscortex_app Flutter bundle (debug/JIT assets)..."
    (
        cd "$APP_DIR"
        flutter --suppress-analytics build bundle --debug
    )
else
    echo "ERROR: Flutter app directory missing: $APP_DIR" >&2
    exit 1
fi

echo "[0.3/5] Compiling system shell to AOT ELF (libapp.so)..."
DARTAOT="/opt/homebrew/share/flutter/bin/cache/dart-sdk/bin/dartaotruntime"
FRONTEND_SERVER="/opt/homebrew/share/flutter/bin/cache/artifacts/engine/darwin-x64/frontend_server_aot.dart.snapshot"
SDK_ROOT_PRODUCT="/opt/homebrew/share/flutter/bin/cache/artifacts/engine/common/flutter_patched_sdk_product/"
GEN_SNAP="/opt/homebrew/share/flutter/bin/cache/artifacts/engine/darwin-x64/gen_snapshot_x64"
PKG_CONFIG="$APP_DIR/.dart_tool/package_config.json"
APP_MAIN="$APP_DIR/lib/main.dart"
AOT_DILL="$APP_DIR/build/app_aot.dill"
LIBAPP_SO_DEST="$ROOT/initramfs/system/flutter/libapp.so"

"$DARTAOT" "$FRONTEND_SERVER" \
    --sdk-root "$SDK_ROOT_PRODUCT" \
    --target=flutter \
    --aot \
    --tfa \
    --packages="$PKG_CONFIG" \
    --output-dill "$AOT_DILL" \
    "$APP_MAIN" 2>&1 | grep -v '^+' | tail -5

if [ ! -f "$AOT_DILL" ]; then
    echo "ERROR: AOT kernel compilation failed — app_aot.dill not produced" >&2
    exit 1
fi

mkdir -p "$ROOT/initramfs/system/flutter"
"$GEN_SNAP" \
    --deterministic \
    --snapshot_kind=app-aot-elf \
    --elf="$LIBAPP_SO_DEST" \
    --strip \
    "$AOT_DILL" 2>&1

if [ ! -f "$LIBAPP_SO_DEST" ]; then
    echo "ERROR: gen_snapshot_x64 failed — libapp.so not produced" >&2
    exit 1
fi
echo "[0.3/5] libapp.so staged: $(wc -c < "$LIBAPP_SO_DEST") bytes"

echo "[0.35/5] Building core system apps into /Applications..."
mkdir -p "$ROOT/initramfs/Applications"
"$ROOT/tools/build-flutter-osx.sh" \
    "$ROOT/apps/oscortex_canvas" \
    "Canvas" \
    "$ROOT/initramfs/Applications/Canvas.app/Canvas.osx" \
    "$ROOT/initramfs/Applications/Canvas.app/flutter_assets"
"$ROOT/tools/build-flutter-osx.sh" \
    "$ROOT/apps/oscortex_files" \
    "Files" \
    "$ROOT/initramfs/Applications/Files.app/Files.osx" \
    "$ROOT/initramfs/Applications/Files.app/flutter_assets"
"$ROOT/tools/build-flutter-osx.sh" \
    "$ROOT/apps/oscortex_web_link" \
    "Web Link" \
    "$ROOT/initramfs/Applications/Web Link.app/Web Link.osx" \
    "$ROOT/initramfs/Applications/Web Link.app/flutter_assets"

if [ -d "$APP_ASSETS_DIR" ]; then
    echo "[0.4/5] Syncing shell Flutter assets into initramfs..."
    mkdir -p "$ROOT/initramfs/system/flutter/flutter_assets"

    for f in kernel_blob.bin vm_snapshot_data isolate_snapshot_data; do
        if [ ! -f "$APP_ASSETS_DIR/$f" ]; then
            echo "ERROR: required app asset missing: $APP_ASSETS_DIR/$f" >&2
            exit 1
        fi
        cp "$APP_ASSETS_DIR/$f" "$ROOT/initramfs/system/flutter/flutter_assets/$f"
    done
    python3 "$ROOT/tools/flutter-engine/engine_patch.py" --kernel-blob "$ROOT/initramfs/system/flutter/flutter_assets/kernel_blob.bin"
    python3 "$ROOT/tools/flutter-engine/engine_patch.py" --kernel-blob "$ROOT/initramfs/Applications/Canvas.app/flutter_assets/kernel_blob.bin"
    python3 "$ROOT/tools/flutter-engine/engine_patch.py" --kernel-blob "$ROOT/initramfs/Applications/Files.app/flutter_assets/kernel_blob.bin"
    python3 "$ROOT/tools/flutter-engine/engine_patch.py" --kernel-blob "$ROOT/initramfs/Applications/Web Link.app/flutter_assets/kernel_blob.bin"

    rm -f \
        "$ROOT/initramfs/system/flutter/kernel_blob.bin" \
        "$ROOT/initramfs/system/flutter/vm_snapshot_data" \
        "$ROOT/initramfs/system/flutter/isolate_snapshot_data"

    for f in AssetManifest.bin FontManifest.json NOTICES.Z NativeAssetsManifest.json version.json; do
        if [ -f "$APP_ASSETS_DIR/$f" ]; then
            cp "$APP_ASSETS_DIR/$f" "$ROOT/initramfs/system/flutter/flutter_assets/$f"
        fi
    done

    for d in fonts packages shaders assets; do
        if [ -d "$APP_ASSETS_DIR/$d" ]; then
            rm -rf "$ROOT/initramfs/system/flutter/flutter_assets/$d"
            cp -R "$APP_ASSETS_DIR/$d" "$ROOT/initramfs/system/flutter/flutter_assets/$d"
        fi
    done
else
    echo "ERROR: Flutter app assets directory missing: $APP_ASSETS_DIR" >&2
    exit 1
fi

echo "[0.5/5] Staging Flutter engine runtime..."
if [ ! -f "$FLUTTER_ENGINE_SO" ]; then
    echo "ERROR: missing Flutter engine binary: $FLUTTER_ENGINE_SO" >&2
    exit 1
fi
mkdir -p "$ROOT/initramfs/system/lib"
unzip -p "$ROOT/tools/flutter-engine/linux-x64-embedder.zip" libflutter_engine.so > "$ROOT/initramfs/system/lib/libflutter_engine.so"
if [ "${OSC_SKIP_ENGINE_PATCH:-0}" = "1" ]; then
    echo "[0.5/5] OSC_SKIP_ENGINE_PATCH=1 — staging PRISTINE engine (no P1-P10 patches)"
else
    python3 "$ROOT/tools/flutter-engine/engine_patch.py" \
        --engine "$ROOT/initramfs/system/lib/libflutter_engine.so" \
        --apply-all
fi

REQUIRED_FILES=(
    "$ROOT/initramfs/init"
    "$ROOT/initramfs/bin/oscortex-host"
    "$ROOT/initramfs/system/lib/libflutter_engine.so"
    "$ROOT/initramfs/system/flutter/icudtl.dat"
    "$ROOT/initramfs/system/flutter/libapp.so"
    "$ROOT/initramfs/Applications/Canvas.app/Canvas.osx"
    "$ROOT/initramfs/Applications/Files.app/Files.osx"
    "$ROOT/initramfs/Applications/Web Link.app/Web Link.osx"
    "$ROOT/initramfs/system/flutter/flutter_assets/kernel_blob.bin"
    "$ROOT/initramfs/system/flutter/flutter_assets/vm_snapshot_data"
    "$ROOT/initramfs/system/flutter/flutter_assets/isolate_snapshot_data"
    "$ROOT/initramfs/Applications/Canvas.app/flutter_assets/kernel_blob.bin"
    "$ROOT/initramfs/Applications/Files.app/flutter_assets/kernel_blob.bin"
    "$ROOT/initramfs/Applications/Web Link.app/flutter_assets/kernel_blob.bin"
)
for req in "${REQUIRED_FILES[@]}"; do
    if [ ! -f "$req" ]; then
        echo "ERROR: required Flutter artifact missing: $req" >&2
        exit 1
    fi
done

echo "[1/5] Building kernel ELF..."
touch "$ROOT/kernel/src/fs/initramfs.rs"
cd "$ROOT"
cargo +nightly build \
    --release \
    --package oscortex-kernel \
    --target x86_64-unknown-none \
    -Z build-std=core,compiler_builtins,alloc \
    -Z build-std-features=compiler-builtins-mem

KERNEL_ELF="$ROOT/target/x86_64-unknown-none/release/kernel"

echo "[2/5] Staging ISO root..."
rm -rf "$ISO_ROOT"
mkdir -p "$ISO_ROOT/boot/limine"
mkdir -p "$ISO_ROOT/EFI/BOOT"

cp "$KERNEL_ELF" "$ISO_ROOT/boot/kernel"

cp "$LIMINE_DIR/limine-bios-cd.bin" "$ISO_ROOT/boot/limine/"
cp "$LIMINE_DIR/limine-bios.sys"    "$ISO_ROOT/boot/limine/"
cp "$LIMINE_DIR/limine-uefi-cd.bin" "$ISO_ROOT/boot/limine/"
cp "$LIMINE_DIR/BOOTX64.EFI"        "$ISO_ROOT/EFI/BOOT/"
cp "$LIMINE_DIR/BOOTIA32.EFI"       "$ISO_ROOT/EFI/BOOT/"

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

STAGED_ENGINE_SO="$ROOT/initramfs/system/lib/libflutter_engine.so"
cp "$STAGED_ENGINE_SO" "$ISO_ROOT/boot/libflutter_engine.so"
echo "[2/5] Staged libflutter_engine.so ($(du -sh "$STAGED_ENGINE_SO" | cut -f1)) as Limine module"

cp "$ISO_ROOT/boot/limine/limine.conf" "$ISO_ROOT/limine.conf"
cp "$ISO_ROOT/boot/limine/limine.conf" "$ISO_ROOT/EFI/BOOT/limine.conf"

echo "[3/5] Building ISO with xorriso..."
xorriso -as mkisofs \
    -b boot/limine/limine-bios-cd.bin \
    -no-emul-boot -boot-load-size 4 -boot-info-table \
    --efi-boot boot/limine/limine-uefi-cd.bin \
    -efi-boot-part --efi-boot-image --protective-msdos-label \
    "$ISO_ROOT" -o "$OUTPUT" 2>/dev/null

limine bios-install "$OUTPUT" 2>/dev/null || true

echo "[4/5] ISO built: $OUTPUT ($(du -sh "$OUTPUT" | cut -f1))"

if [[ "${1:-}" == "--run" ]]; then
    echo ""
    echo "Launching QEMU... (serial output on stdout, Ctrl+A X to quit)"
    echo ""
    qemu-system-x86_64 \
        -cdrom "$OUTPUT" \
        -cpu qemu64,+x2apic \
        -m 2G \
        -smp 1 \
        -serial stdio \
        -display none \
        -no-reboot \
        -d int,cpu_reset \
        -D "$ROOT/qemu-log.txt" \
        2>&1
fi
