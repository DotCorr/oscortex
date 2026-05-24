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
FLUTTER_ENGINE_SO="$ROOT/tools/flutter-engine/libflutter_engine.so"

if [ -d "$EMBEDDER_DIR" ]; then
    echo "[0/4] Building flutter-embedder userspace binary..."
    (cd "$EMBEDDER_DIR" && \
        cargo +nightly build \
            --release \
            --target "$EMBEDDER_TARGET" \
            -Z build-std=core,compiler_builtins \
            -Z build-std-features=compiler-builtins-mem \
            2>&1)

    EMBEDDER_BIN="$EMBEDDER_DIR/target/$EMBEDDER_TARGET/release/flutter-embedder"
    if [ -f "$EMBEDDER_BIN" ]; then
        mkdir -p "$ROOT/initramfs/bin"
        cp "$EMBEDDER_BIN" "$ROOT/initramfs/bin/flutter-embedder"
        echo "[0/4] flutter-embedder staged to initramfs/bin/flutter-embedder"
    else
        echo "ERROR: flutter-embedder binary not found after build: $EMBEDDER_BIN" >&2
        exit 1
    fi
else
    echo "ERROR: flutter-embedder directory missing: $EMBEDDER_DIR" >&2
    exit 1
fi

# Sync Flutter app assets into initramfs so engine/snapshot inputs stay consistent.
APP_DIR="$ROOT/apps/oscortex_app"
APP_ASSETS_DIR="$APP_DIR/build/flutter_assets"
if [ -d "$APP_DIR" ]; then
    echo "[0.3/4] Building oscortex_app Flutter bundle (debug/JIT assets)..."
    (
        cd "$APP_DIR"
        flutter --suppress-analytics build bundle --debug
    )
else
    echo "ERROR: Flutter app directory missing: $APP_DIR" >&2
    exit 1
fi

if [ -d "$APP_ASSETS_DIR" ]; then
    echo "[0.4/4] Syncing oscortex_app Flutter assets into initramfs..."
    mkdir -p "$ROOT/initramfs/system/flutter/flutter_assets"

    for f in kernel_blob.bin vm_snapshot_data isolate_snapshot_data; do
        if [ ! -f "$APP_ASSETS_DIR/$f" ]; then
            echo "ERROR: required app asset missing: $APP_ASSETS_DIR/$f" >&2
            exit 1
        fi
        cp "$APP_ASSETS_DIR/$f" "$ROOT/initramfs/system/flutter/flutter_assets/$f"
    done

    # Strict path contract: snapshots are only consumed from
    # /system/flutter/flutter_assets. Clear stale legacy top-level copies.
    rm -f \
        "$ROOT/initramfs/system/flutter/kernel_blob.bin" \
        "$ROOT/initramfs/system/flutter/vm_snapshot_data" \
        "$ROOT/initramfs/system/flutter/isolate_snapshot_data"

    for f in AssetManifest.bin FontManifest.json NOTICES.Z NativeAssetsManifest.json version.json; do
        if [ -f "$APP_ASSETS_DIR/$f" ]; then
            cp "$APP_ASSETS_DIR/$f" "$ROOT/initramfs/system/flutter/flutter_assets/$f"
        fi
    done

    for d in fonts packages shaders; do
        if [ -d "$APP_ASSETS_DIR/$d" ]; then
            rm -rf "$ROOT/initramfs/system/flutter/flutter_assets/$d"
            cp -R "$APP_ASSETS_DIR/$d" "$ROOT/initramfs/system/flutter/flutter_assets/$d"
        fi
    done
else
    echo "ERROR: Flutter app assets directory missing: $APP_ASSETS_DIR" >&2
    echo "       Build it first with: cd apps/oscortex_app && flutter build bundle" >&2
    exit 1
fi

echo "[0.42/4] Staging subsystem app packages and registry..."
mkdir -p "$ROOT/initramfs/system/apps"
REGISTRY_JSON="$ROOT/initramfs/system/apps/registry.json"
REGISTRY_TMP="$ROOT/initramfs/system/apps/.registry.tmp"
printf '{\n  "apps": [\n' > "$REGISTRY_TMP"
first=1

for app_path in "$ROOT"/apps/*; do
    [ -d "$app_path" ] || continue
    [ -f "$app_path/pubspec.yaml" ] || continue

    app_id="$(basename "$app_path")"
    app_assets="$app_path/build/flutter_assets"

    if [ ! -d "$app_assets" ] || [ ! -f "$app_assets/kernel_blob.bin" ] || [ ! -f "$app_assets/vm_snapshot_data" ] || [ ! -f "$app_assets/isolate_snapshot_data" ]; then
        echo "[0.42/4] Building subsystem Flutter app bundle: $app_id"
        (
            cd "$app_path"
            flutter --suppress-analytics build bundle --debug
        )
    fi

    if [ ! -d "$app_assets" ]; then
        echo "ERROR: app assets missing for subsystem app: $app_id ($app_assets)" >&2
        exit 1
    fi

    dst="$ROOT/initramfs/system/apps/$app_id"
    mkdir -p "$dst/flutter_assets"

    for f in kernel_blob.bin vm_snapshot_data isolate_snapshot_data; do
        if [ ! -f "$app_assets/$f" ]; then
            echo "ERROR: required subsystem app asset missing: $app_assets/$f" >&2
            exit 1
        fi
        cp "$app_assets/$f" "$dst/flutter_assets/$f"
    done

    for f in AssetManifest.bin FontManifest.json NOTICES.Z NativeAssetsManifest.json version.json; do
        if [ -f "$app_assets/$f" ]; then
            cp "$app_assets/$f" "$dst/flutter_assets/$f"
        fi
    done

    for d in fonts packages shaders; do
        if [ -d "$app_assets/$d" ]; then
            rm -rf "$dst/flutter_assets/$d"
            cp -R "$app_assets/$d" "$dst/flutter_assets/$d"
        fi
    done

    if [ "$first" -eq 0 ]; then
        printf ',\n' >> "$REGISTRY_TMP"
    fi
    first=0
    printf '    {"id":"%s","title":"%s","bundle_path":"/system/apps/%s/flutter_assets"}' "$app_id" "$app_id" "$app_id" >> "$REGISTRY_TMP"
done

printf '\n  ]\n}\n' >> "$REGISTRY_TMP"
mv "$REGISTRY_TMP" "$REGISTRY_JSON"
cp "$REGISTRY_JSON" "$ROOT/initramfs/system/flutter/flutter_assets/system_apps_registry.json"

echo "[0.45/4] Staging Flutter engine runtime into initramfs subsystem..."
if [ ! -f "$FLUTTER_ENGINE_SO" ]; then
    echo "ERROR: missing Flutter engine binary: $FLUTTER_ENGINE_SO" >&2
    exit 1
fi
mkdir -p "$ROOT/initramfs/system/lib"
cp "$FLUTTER_ENGINE_SO" "$ROOT/initramfs/system/lib/libflutter_engine.so"

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
        echo "[0.5/4] WARNING: userspace/init build failed (non-fatal)"
    }

    INIT_BIN="$INIT_DIR/target/$INIT_TARGET/release/init"
    if [ -f "$INIT_BIN" ]; then
        mkdir -p "$ROOT/initramfs/bin"
        cp "$INIT_BIN" "$ROOT/initramfs/bin/init"
        echo "[0.5/4] userspace/init staged to initramfs/bin/init ($(wc -c < "$INIT_BIN") bytes)"
    fi
fi

# Flutter-only boot policy: flutter-embedder is always /init (PID 1)
EMBEDDER_BIN="$ROOT/tools/flutter-embedder/target/x86_64-unknown-none/release/flutter-embedder"
if [ -f "$EMBEDDER_BIN" ]; then
    cp "$EMBEDDER_BIN" "$ROOT/initramfs/init"
    echo "[0.6/4] Staged flutter-embedder to initramfs/init as PID 1"
else
    echo "ERROR: flutter-embedder binary not found! Cannot stage as PID 1." >&2
    exit 1
fi

# Required Flutter runtime assets for the embedder path.
REQUIRED_FILES=(
    "$ROOT/initramfs/init"
    "$ROOT/initramfs/bin/flutter-embedder"
    "$ROOT/initramfs/system/lib/libflutter_engine.so"
    "$ROOT/initramfs/system/apps/registry.json"
    "$ROOT/initramfs/system/flutter/icudtl.dat"
    "$ROOT/initramfs/system/flutter/flutter_assets/kernel_blob.bin"
    "$ROOT/initramfs/system/flutter/flutter_assets/vm_snapshot_data"
    "$ROOT/initramfs/system/flutter/flutter_assets/isolate_snapshot_data"
    "$ROOT/initramfs/system/flutter/flutter_assets/system_apps_registry.json"
)
for req in "${REQUIRED_FILES[@]}"; do
    if [ ! -f "$req" ]; then
        echo "ERROR: required Flutter artifact missing: $req" >&2
        exit 1
    fi
done

echo "[1/4] Building kernel ELF..."
cd "$ROOT"
cargo +nightly build \
    --release \
    --package oscortex-kernel \
    --target x86_64-unknown-none \
    -Z build-std=core,compiler_builtins,alloc \
    -Z build-std-features=compiler-builtins-mem

KERNEL_ELF="$ROOT/target/x86_64-unknown-none/release/kernel"

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

# Stage the exact same engine binary from initramfs into the Limine module.
STAGED_ENGINE_SO="$ROOT/initramfs/system/lib/libflutter_engine.so"
cp "$STAGED_ENGINE_SO" "$ISO_ROOT/boot/libflutter_engine.so"
echo "[2/4] Staged libflutter_engine.so ($(du -sh "$STAGED_ENGINE_SO" | cut -f1)) as Limine module"

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

