#!/usr/bin/env bash
# Assemble + build the OSCortex aarch64 Flutter shell.
#
# This is the ARM counterpart to scripts/build-iso.sh (which targets x86 + Limine).
# On -M virt there is no Limine; the kernel boots via `-kernel` and everything the
# Flutter shell host needs is delivered through the initramfs (assembled into the
# kernel by kernel/build.rs from the `initramfs/` directory):
#
#   /init                                  → the aarch64 Flutter embedder (oscortex-host)
#   /system/lib/libflutter_engine.so       → the aarch64 Flutter engine (AOT)
#   /system/lib/liboscortex_libc.so        → aarch64 libc/math helpers
#   /system/flutter/libapp.so              → the aarch64 AOT shell snapshot
#   /system/flutter/icudtl.dat             → ICU data
#   /system/flutter/flutter_assets/        → fonts + manifests (arch-neutral)
#
# The aarch64 engine .so + gen_snapshot live in the `oscx-engine` Docker image
# (built by the engine port); the AOT shell snapshot (libapp_arm64.so) is produced
# from the app's AOT dill by the version-matched arm64 gen_snapshot. We stage the
# already-built artifacts here.
#
# Usage:  scripts/build-aarch64-shell.sh
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

TARGET=aarch64-unknown-none
EMBEDDER_DIR="$ROOT/tools/flutter-embedder"
CONTAINER=oscx-engine
ARM_OUT=/work/engine/engine/src/out/oscortex_arm64_release

# Where the x86 build already prepared the arch-neutral shell assets (fonts +
# manifests). We reuse those rather than re-running the Flutter toolchain.
X86_ASSETS_SRC="${X86_ASSETS_SRC:-$ROOT/initramfs/system/flutter/flutter_assets}"

echo "[arm-shell] 1/6 building the aarch64 embedder host (oscortex-host)..."
(
    cd "$EMBEDDER_DIR"
    cargo +nightly build --release --target "$TARGET" \
        -Z build-std=core,compiler_builtins \
        -Z build-std-features=compiler-builtins-mem
)
HOST_BIN="$EMBEDDER_DIR/target/$TARGET/release/oscortex-host"
[ -f "$HOST_BIN" ] || { echo "ERROR: $HOST_BIN not built" >&2; exit 1; }

echo "[arm-shell] 2/6 staging the embedder as /init + /bin/oscortex-host..."
mkdir -p "$ROOT/initramfs/bin" "$ROOT/initramfs/system/lib" "$ROOT/initramfs/system/flutter/flutter_assets"
cp "$HOST_BIN" "$ROOT/initramfs/bin/oscortex-host"
cp "$HOST_BIN" "$ROOT/initramfs/init"

echo "[arm-shell] 3/6 staging the aarch64 Flutter engine + ICU..."
docker cp "$CONTAINER:$ARM_OUT/libflutter_engine.so" "$ROOT/initramfs/system/lib/libflutter_engine.so"
docker cp "$CONTAINER:$ARM_OUT/icudtl.dat"           "$ROOT/initramfs/system/flutter/icudtl.dat"

echo "[arm-shell] 4/6 staging the aarch64 AOT shell snapshot (libapp.so)..."
# Produced by the arm64 gen_snapshot from the app AOT dill. Prefer the staged
# /work/libapp_arm64.so; fall back to compiling it from /work/app_aot.dill.
if docker exec "$CONTAINER" test -f /work/libapp_arm64.so; then
    docker cp "$CONTAINER:/work/libapp_arm64.so" "$ROOT/initramfs/system/flutter/libapp.so"
else
    echo "[arm-shell]   /work/libapp_arm64.so absent — compiling from /work/app_aot.dill"
    docker exec "$CONTAINER" "$ARM_OUT/gen_snapshot" \
        --snapshot_kind=app-aot-elf --elf=/work/libapp_arm64.so --strip /work/app_aot.dill
    docker cp "$CONTAINER:/work/libapp_arm64.so" "$ROOT/initramfs/system/flutter/libapp.so"
fi

echo "[arm-shell] 5/6 building the aarch64 libc helper + staging assets..."
docker run --rm --platform linux/arm64 \
    -v "$ROOT:$ROOT" -w "$ROOT" \
    gcc:12 \
    gcc -shared -fPIC -ffreestanding -nostdlib -O2 \
    -o "$ROOT/initramfs/system/lib/liboscortex_libc.so" \
    "$ROOT/userspace/libc/libc.c"

# Arch-neutral shell assets (fonts, FontManifest, AssetManifest). These data files
# are IDENTICAL across architectures — the engine reads them the same way on x86
# and ARM. Under AOT we do NOT ship the JIT blobs (kernel_blob.bin / *_snapshot_data);
# the libapp.so snapshot carries the program. Without these assets the shell renders
# a blank (background-only) frame: no fonts → no text, no MaterialIcons → no icons.
APP_DIR="$ROOT/apps/oscortex_app"
APP_ASSETS_DIR="$APP_DIR/build/flutter_assets"
SHELL_ASSETS="$ROOT/initramfs/system/flutter/flutter_assets"

# Build the Flutter asset bundle if it is missing (produces build/flutter_assets:
# FontManifest.json, AssetManifest.bin, fonts/, packages/, shaders/, assets/, …).
if [ ! -f "$APP_ASSETS_DIR/FontManifest.json" ]; then
    echo "[arm-shell]   building Flutter asset bundle (flutter build bundle)..."
    if command -v flutter >/dev/null 2>&1; then
        ( cd "$APP_DIR" && flutter --suppress-analytics build bundle --debug )
    else
        echo "[arm-shell]   WARN: flutter not on PATH and no prebuilt bundle — text/icons will be blank" >&2
    fi
fi

if [ -f "$APP_ASSETS_DIR/FontManifest.json" ]; then
    echo "[arm-shell]   staging arch-neutral assets + fonts from $APP_ASSETS_DIR"
    mkdir -p "$SHELL_ASSETS"
    for f in AssetManifest.bin AssetManifest.json FontManifest.json NOTICES.Z \
             NativeAssetsManifest.json version.json; do
        [ -f "$APP_ASSETS_DIR/$f" ] && cp "$APP_ASSETS_DIR/$f" "$SHELL_ASSETS/$f"
    done
    for d in fonts packages shaders assets; do
        if [ -d "$APP_ASSETS_DIR/$d" ]; then
            rm -rf "$SHELL_ASSETS/$d"
            cp -R "$APP_ASSETS_DIR/$d" "$SHELL_ASSETS/$d"
        fi
    done
    # CRITICAL (bare metal): Flutter has NO system font provider and the bundle ships
    # no default/Roboto family, so Material's default-font text would loop forever in
    # font fallback (or render blank). Alias the common default families to the bundled
    # NotoSans so every Text (incl. Material widgets) resolves to a real glyph set.
    NOTO_SRC="$APP_DIR/assets/fonts/NotoSans.ttf"
    mkdir -p "$SHELL_ASSETS/assets/fonts"
    [ -f "$NOTO_SRC" ] && cp "$NOTO_SRC" "$SHELL_ASSETS/assets/fonts/NotoSans.ttf"
    FONT_MANIFEST="$SHELL_ASSETS/FontManifest.json"
    if [ -f "$FONT_MANIFEST" ]; then
        python3 - "$FONT_MANIFEST" <<'PYFONT'
import json, sys, os
p = sys.argv[1]
m = json.load(open(p)) if os.path.exists(p) else []
have = {e["family"] for e in m}
noto = [{"asset": "assets/fonts/NotoSans.ttf"}]
if "NotoSans" not in have:
    m.append({"family": "NotoSans", "fonts": noto})
for fam in ["Roboto", "sans-serif", "Arial", "Helvetica", ".SF UI Text", "DejaVu Sans"]:
    if fam not in have:
        m.append({"family": fam, "fonts": noto})
json.dump(m, open(p, "w"))
print("[fonts] aliased default families -> NotoSans:", sorted(e["family"] for e in m))
PYFONT
    fi
else
    echo "[arm-shell]   WARN: no Flutter asset bundle — shell will render blank (no fonts/icons)" >&2
fi
# Remove any JIT-only blobs (AOT carries the program in libapp.so).
rm -f "$SHELL_ASSETS/kernel_blob.bin" \
      "$SHELL_ASSETS/vm_snapshot_data" \
      "$SHELL_ASSETS/isolate_snapshot_data"

# Seed bundle for the shell's "Install demo" button (install:/system/seed/demo.osx).
# Arch-neutral .osx bundle; reuse the x86 checkout's copy if present.
SEED_SRC="${SEED_SRC:-$ROOT/../../../initramfs/system/seed/demo.osx}"
if [ -f "$ROOT/initramfs/system/seed/demo.osx" ]; then
    : # already staged
elif [ -f "$SEED_SRC" ]; then
    mkdir -p "$ROOT/initramfs/system/seed"
    cp "$SEED_SRC" "$ROOT/initramfs/system/seed/demo.osx"
    echo "[arm-shell]   staged demo.osx seed bundle"
else
    echo "[arm-shell]   note: no demo.osx seed found — Install demo will have nothing to install" >&2
fi

echo "[arm-shell] 6/6 building the aarch64 kernel (embeds the initramfs)..."
# Force a rebuild of the initramfs (kernel/build.rs reads the initramfs/ dir).
touch "$ROOT/kernel/src/fs/initramfs.rs" 2>/dev/null || true
cargo build --target "$TARGET" -p oscortex-kernel \
    --no-default-features --features arch-aarch64 \
    -Z build-std=core,compiler_builtins,alloc \
    -Z build-std-features=compiler-builtins-mem

echo "[arm-shell] done. Boot with: scripts/run-aarch64.sh"
echo "[arm-shell] staged initramfs contents:"
ls -la "$ROOT/initramfs/init" \
       "$ROOT/initramfs/system/lib/libflutter_engine.so" \
       "$ROOT/initramfs/system/lib/liboscortex_libc.so" \
       "$ROOT/initramfs/system/flutter/libapp.so" \
       "$ROOT/initramfs/system/flutter/icudtl.dat" 2>/dev/null
