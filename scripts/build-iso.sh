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

# NOTE: the old [0.3/5] step compiled the shell to an AOT libapp.so via a vendored
# gen_snapshot (formerly scratch/linux-x64/gen_snapshot) and patch_libapp.py. That
# whole path is GONE: OSCortex runs the JIT engine off kernel_blob.bin, not an AOT
# libapp.so. Native AOT is being done properly via the engine port — see
# docs/native-engine-port.md. (Removed with the scratch/ debugging tree.)

mkdir -p "$ROOT/initramfs/Applications"
if [ -n "${SKIP_CORE_APPS:-}" ]; then
    # The core-app rebuild drives tools/build-flutter-osx.sh, whose AOT step needs
    # the oscx-engine Docker image. When that image is unavailable, skip the rebuild
    # and reuse the app assets already staged in initramfs/Applications (the apps run
    # JIT off kernel_blob.bin — arch-independent — so the staged bundles still render
    # their launcher tiles). Set SKIP_CORE_APPS=1 to take this path.
    echo "[0.35/5] SKIP_CORE_APPS set — reusing staged app assets (Docker/oscx-engine not required)"
    if [ -n "${X86_AOT:-}" ]; then
        # AOT path uses prebuilt libapp.so snapshots, not JIT kernel_blob — require those.
        for a in "Files.app" "Canvas.app" "Web Link.app"; do
            src=""
            case "$a" in
              "Files.app") src="oscortex_files" ;;
              "Canvas.app") src="oscortex_canvas" ;;
              "Web Link.app") src="oscortex_web_link" ;;
            esac
            if [ ! -f "$ROOT/apps/$src/build/oscortex/libapp.so" ]; then
                echo "ERROR: X86_AOT set but app AOT snapshot missing: apps/$src/build/oscortex/libapp.so" >&2
                exit 1
            fi
        done
    else
        for a in "Canvas.app/flutter_assets/kernel_blob.bin" "Files.app/flutter_assets/kernel_blob.bin" "Web Link.app/flutter_assets/kernel_blob.bin"; do
            if [ ! -f "$ROOT/initramfs/Applications/$a" ]; then
                echo "ERROR: SKIP_CORE_APPS set but staged app asset missing: initramfs/Applications/$a" >&2
                exit 1
            fi
        done
    fi
else
    echo "[0.35/5] Building core system apps into /Applications..."
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
fi

if [ -d "$APP_ASSETS_DIR" ]; then
    echo "[0.4/5] Syncing shell Flutter assets into initramfs..."
    mkdir -p "$ROOT/initramfs/system/flutter/flutter_assets"

    # JIT path needs kernel_blob.bin (the Dart program for the JIT engine); the
    # X86_AOT path carries the program in libapp.so and skips this entirely.
    if [ -z "${X86_AOT:-}" ]; then
        for f in kernel_blob.bin; do
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
    fi

    # Each launched app is its own Flutter host and hits the same bare-metal font
    # issue as the shell: no system font provider + no default/Roboto family ->
    # Material text livelocks in font fallback. Give every app the bundled
    # NotoSans and alias the common default families to it.
    NOTO_SRC="$APP_DIR/assets/fonts/NotoSans.ttf"
    for APP_ASSETS in \
        "$ROOT/initramfs/Applications/Canvas.app/flutter_assets" \
        "$ROOT/initramfs/Applications/Files.app/flutter_assets" \
        "$ROOT/initramfs/Applications/Web Link.app/flutter_assets"; do
        [ -d "$APP_ASSETS" ] || continue
        mkdir -p "$APP_ASSETS/assets/fonts"
        [ -f "$NOTO_SRC" ] && cp "$NOTO_SRC" "$APP_ASSETS/assets/fonts/NotoSans.ttf"
        python3 - "$APP_ASSETS/FontManifest.json" <<'PYFONT'
import json, sys, os
p = sys.argv[1]
m = json.load(open(p)) if os.path.exists(p) else []
have = {e["family"] for e in m}
noto = [{"asset": "assets/fonts/NotoSans.ttf"}]
if "NotoSans" not in have: m.append({"family": "NotoSans", "fonts": noto})
for fam in ["Roboto", "sans-serif", "Arial", "Helvetica", ".SF UI Text", "DejaVu Sans"]:
    if fam not in have: m.append({"family": fam, "fonts": noto})
json.dump(m, open(p, "w"))
print("[fonts] app", os.path.dirname(p).split('/')[-2], "-> NotoSans default")
PYFONT
    done

    rm -f \
        "$ROOT/initramfs/system/flutter/kernel_blob.bin" \
        "$ROOT/initramfs/system/flutter/vm_snapshot_data" \
        "$ROOT/initramfs/system/flutter/isolate_snapshot_data" \
        "$ROOT/initramfs/system/flutter/flutter_assets/vm_snapshot_data" \
        "$ROOT/initramfs/system/flutter/flutter_assets/isolate_snapshot_data" \
        "$ROOT/initramfs/Applications/Canvas.app/flutter_assets/vm_snapshot_data" \
        "$ROOT/initramfs/Applications/Canvas.app/flutter_assets/isolate_snapshot_data" \
        "$ROOT/initramfs/Applications/Files.app/flutter_assets/vm_snapshot_data" \
        "$ROOT/initramfs/Applications/Files.app/flutter_assets/isolate_snapshot_data" \
        "$ROOT/initramfs/Applications/Web Link.app/flutter_assets/vm_snapshot_data" \
        "$ROOT/initramfs/Applications/Web Link.app/flutter_assets/isolate_snapshot_data"

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

    # CRITICAL (bare metal): Flutter has NO system font provider here, and the
    # bundle ships no default/Roboto family — so Material's default-font text
    # would loop forever in font fallback and the first frame never renders.
    # Alias the common default families to the bundled NotoSans so every Text
    # (incl. Material widgets) resolves to a real glyph set.
    FONT_MANIFEST="$ROOT/initramfs/system/flutter/flutter_assets/FontManifest.json"
    if [ -f "$FONT_MANIFEST" ]; then
        python3 - "$FONT_MANIFEST" <<'PYFONT'
import json, sys
p = sys.argv[1]
m = json.load(open(p))
have = {e["family"] for e in m}
noto = [{"asset": "assets/fonts/NotoSans.ttf"}]
for fam in ["Roboto", "sans-serif", "Arial", "Helvetica", ".SF UI Text", "DejaVu Sans"]:
    if fam not in have:
        m.append({"family": fam, "fonts": noto})
json.dump(m, open(p, "w"))
print("[fonts] aliased default families -> NotoSans:", sorted(e["family"] for e in m))
PYFONT
    fi
else
    echo "ERROR: Flutter app assets directory missing: $APP_ASSETS_DIR" >&2
    exit 1
fi

if [ -f "$ROOT/initramfs/system/lib/liboscortex_libc.so" ]; then
    echo "[0.5/5] Skipping compilation of userspace libc helper (already exists)..."
else
    echo "[0.5/5] Compiling and staging userspace libc helper..."
    mkdir -p "$ROOT/initramfs/system/lib"
    docker run --rm --platform linux/amd64 \
        -v "$ROOT:$ROOT" \
        -w "$ROOT" \
        gcc:12 \
        gcc -shared -fPIC -ffreestanding -nostdlib -O2 \
        -o "$ROOT/initramfs/system/lib/liboscortex_libc.so" \
        "$ROOT/userspace/libc/libc.c"
fi

echo "[0.51/5] Staging Flutter engine runtime..."
if [ ! -f "$FLUTTER_ENGINE_SO" ]; then
    echo "ERROR: missing Flutter engine binary: $FLUTTER_ENGINE_SO" >&2
    exit 1
fi
cp "$FLUTTER_ENGINE_SO" "$ROOT/initramfs/system/lib/libflutter_engine.so"
echo "[0.51/5] Staging PATCHED profile engine"

REQUIRED_FILES=(
    "$ROOT/initramfs/init"
    "$ROOT/initramfs/bin/oscortex-host"
    "$ROOT/initramfs/system/lib/libflutter_engine.so"
    "$ROOT/initramfs/system/lib/liboscortex_libc.so"
    "$ROOT/initramfs/system/flutter/icudtl.dat"
    "$ROOT/initramfs/Applications/Canvas.app/Canvas.osx"
    "$ROOT/initramfs/Applications/Files.app/Files.osx"
    "$ROOT/initramfs/Applications/Web Link.app/Web Link.osx"
)
# JIT path requires kernel_blob; the X86_AOT path stages + validates its libapp.so
# snapshots in the dedicated block below, so it skips the kernel_blob requirement.
if [ -z "${X86_AOT:-}" ]; then
    REQUIRED_FILES+=(
        "$ROOT/initramfs/system/flutter/flutter_assets/kernel_blob.bin"
        "$ROOT/initramfs/Applications/Canvas.app/flutter_assets/kernel_blob.bin"
        "$ROOT/initramfs/Applications/Files.app/flutter_assets/kernel_blob.bin"
        "$ROOT/initramfs/Applications/Web Link.app/flutter_assets/kernel_blob.bin"
    )
fi
for req in "${REQUIRED_FILES[@]}"; do
    if [ ! -f "$req" ]; then
        echo "ERROR: required Flutter artifact missing: $req" >&2
        exit 1
    fi
done

# x86_64 AOT path (X86_AOT=1): the patched release x64 engine is AOT-only (reports
# FlutterEngineRunsAOTCompiledDartCode=true), so the shell/apps must load their AOT
# libapp.so snapshots — NOT kernel_blob (the release engine has no Dart kernel
# compiler). The matching x86-64 AOT snapshots are prebuilt under apps/*/build/
# oscortex/libapp.so (version-matched to the engine). Stage them like the arm64
# flow. Without X86_AOT, keep the legacy JIT path (strip AOT snapshots, use
# kernel_blob) for a JIT/debug engine.
if [ -n "${X86_AOT:-}" ]; then
    echo "[1.5/5] X86_AOT: staging x86-64 AOT snapshots (shell + apps)..."
    SHELL_AOT="$ROOT/apps/oscortex_app/build/oscortex/libapp.so"
    [ -f "$SHELL_AOT" ] || { echo "ERROR: X86_AOT but shell snapshot missing: $SHELL_AOT" >&2; exit 1; }
    cp "$SHELL_AOT" "$ROOT/initramfs/system/flutter/libapp.so"
    # Map each bundled app to its prebuilt x86-64 AOT snapshot.
    stage_app_aot() { # <app-src-dir> <App.app dir name>
        local src="$ROOT/apps/$1/build/oscortex/libapp.so"
        local dst="$ROOT/initramfs/Applications/$2/libapp.so"
        if [ -f "$src" ]; then cp "$src" "$dst"; echo "  staged $2/libapp.so"; \
        else echo "  WARN: missing app AOT snapshot $src" >&2; fi
    }
    stage_app_aot oscortex_files   "Files.app"
    stage_app_aot oscortex_canvas  "Canvas.app"
    stage_app_aot oscortex_web_link "Web Link.app"
    # AOT carries the program in libapp.so — drop the JIT kernel_blob so the
    # embedder takes the AOT path (mirrors the arm64 build).
    rm -f "$ROOT/initramfs/system/flutter/flutter_assets/kernel_blob.bin" \
          "$ROOT/initramfs/Applications/Files.app/flutter_assets/kernel_blob.bin" \
          "$ROOT/initramfs/Applications/Canvas.app/flutter_assets/kernel_blob.bin" \
          "$ROOT/initramfs/Applications/Web Link.app/flutter_assets/kernel_blob.bin"
else
    # Legacy JIT path: strip AOT snapshots so the embedder uses kernel_blob.
    rm -f "$ROOT/initramfs/system/flutter/libapp.so" \
          "$ROOT/initramfs/system/flutter/app.aot"
fi

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

cat "$KERNEL_ELF" > "$ISO_ROOT/boot/kernel"

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
cat "$STAGED_ENGINE_SO" > "$ISO_ROOT/boot/libflutter_engine.so"
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
