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

# Arch-neutral shell assets (fonts, FontManifest, AssetManifest). Under AOT we do
# NOT ship kernel_blob.bin (the libapp.so snapshot carries the program). If the
# x86 build already prepared them, reuse; otherwise the embedder still boots with
# just the engine-default fonts (text may be blank until fonts land).
if [ -d "$X86_ASSETS_SRC" ] && [ "$X86_ASSETS_SRC" != "$ROOT/initramfs/system/flutter/flutter_assets" ]; then
    cp -R "$X86_ASSETS_SRC/." "$ROOT/initramfs/system/flutter/flutter_assets/"
fi
# Remove any JIT-only blobs that may have been copied in (AOT carries the program).
rm -f "$ROOT/initramfs/system/flutter/flutter_assets/kernel_blob.bin" \
      "$ROOT/initramfs/system/flutter/flutter_assets/vm_snapshot_data" \
      "$ROOT/initramfs/system/flutter/flutter_assets/isolate_snapshot_data"

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
