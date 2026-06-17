#!/usr/bin/env bash
# Recover the aarch64 build tree after an x86 build clobbered the shared
# initramfs/ (the build-fragility issue — see dev-tools/README.md). Re-stages the
# arm64 shell + app AOT snapshots, the arm64 engine, the arm64 embedder, drops the
# x86 JIT kernel_blob, and rebuilds the arm64 kernel.
#
# Prereqs: the `oscx-engine` build container running (holds the arm64 engine), and
# the prebuilt arm64 AOT snapshots under apps/*/build/oscortex/libapp-arm64.so.
#
# Usage:  dev-tools/build/restore-arm64.sh
set -euo pipefail

ROOT="$(git -C "$(dirname "${BASH_SOURCE[0]}")" rev-parse --show-toplevel)"
cd "$ROOT"
CONTAINER="${OSCX_ENGINE_CONTAINER:-oscx-engine}"
ARM_ENGINE_OUT="${ARM_ENGINE_OUT:-/work/engine/engine/src/out/linux_release_arm64}"

echo "[arm-restore] 1/5 shell + app AOT snapshots (aarch64)"
cp "apps/oscortex_app/build/oscortex/libapp-arm64.so"       initramfs/system/flutter/libapp.so
cp "apps/oscortex_files/build/oscortex/libapp-arm64.so"     "initramfs/Applications/Files.app/libapp.so"
cp "apps/oscortex_canvas/build/oscortex/libapp-arm64.so"    "initramfs/Applications/Canvas.app/libapp.so"
cp "apps/oscortex_web_link/build/oscortex/libapp-arm64.so"  "initramfs/Applications/Web Link.app/libapp.so"

echo "[arm-restore] 2/5 drop x86 JIT kernel_blob (arm is AOT)"
rm -f initramfs/system/flutter/flutter_assets/kernel_blob.bin initramfs/system/flutter/kernel_blob.bin
for a in Files Canvas "Web Link"; do rm -f "initramfs/Applications/$a.app/flutter_assets/kernel_blob.bin"; done

echo "[arm-restore] 3/5 arm64 engine from the build container"
docker cp "$CONTAINER:$ARM_ENGINE_OUT/libflutter_engine.so" initramfs/system/lib/libflutter_engine.so

echo "[arm-restore] 4/5 arm64 embedder → /init"
( cd tools/flutter-embedder && cargo +nightly build --release --target aarch64-unknown-none \
    -Z build-std=core,compiler_builtins -Z build-std-features=compiler-builtins-mem >/dev/null 2>&1 )
cp tools/flutter-embedder/target/aarch64-unknown-none/release/oscortex-host initramfs/init
cp tools/flutter-embedder/target/aarch64-unknown-none/release/oscortex-host initramfs/bin/oscortex-host

echo "[arm-restore] 5/5 build aarch64 kernel"
touch kernel/src/fs/initramfs.rs
cargo build --target aarch64-unknown-none -p oscortex-kernel \
    --no-default-features --features arch-aarch64 \
    -Z build-std=core,compiler_builtins,alloc -Z build-std-features=compiler-builtins-mem 2>&1 | tail -2

echo "[arm-restore] done. engine=$(file initramfs/system/lib/libflutter_engine.so | grep -oE 'aarch64|x86-64') libapp=$(file initramfs/system/flutter/libapp.so | grep -oE 'aarch64|x86-64') init=$(file initramfs/init | grep -oE 'aarch64|x86-64')"
