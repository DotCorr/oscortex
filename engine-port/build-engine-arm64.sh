#!/usr/bin/env bash
#
# build-engine-arm64.sh — configure (GN) + build (ninja) the OSCortex Flutter
# engine + gen_snapshot in RELEASE/AOT mode, CROSS-COMPILED to aarch64 (arm64).
#
# This mirrors the validated x86_64 oscortex release build exactly (same
# is_oscortex flag, same software-render/no-Impeller-at-runtime config, same
# release/AOT flags), but targets arm64. The host toolchain stays linux-x64; we
# cross-compile to arm64 using the engine's prebuilt clang (multi-arch) + the
# bundled debian_bullseye_arm64 sysroot. Mirrors the engine's own linux-arm64
# cross-build path (--target-os=linux --linux-cpu=arm64 is treated as a host
# build that cross-targets arm64).
#
# Prereqs (run once, in order):
#   engine-port/setup-engine-build.sh     # depot_tools + gclient checkout
#   engine-port/apply-port.sh             # patches 0001-0004 + oscortex backend
# Patch 0004 teaches flutter/tools/gn to honor --linux-cpu=arm64 for the
# oscortex target (target_cpu = arm64, dart_target_arch = arm64).
#
# Usage:
#   engine-port/build-engine-arm64.sh                 # release (default)
#   engine-port/build-engine-arm64.sh release
#
# Output (inside the container workspace, NOT committed to the repo):
#   /work/engine/engine/src/out/oscortex_arm64_release/libflutter_engine.so  (arm64 ELF)
#   /work/engine/engine/src/out/oscortex_arm64_release/gen_snapshot          (arm64-targeting)
set -euo pipefail

CONTAINER="oscx-engine"
MODE="${1:-release}"

case "$MODE" in
  release)
    # IDENTICAL to the x86 oscortex release build, plus --linux-cpu=arm64 (which
    # patch 0004 routes to target_cpu=arm64). --target-dir gives a stable,
    # explicit out/ name so it never collides with the x64 release.
    GN_ARGS="--runtime-mode=release --no-rbe --no-goma --prebuilt-dart-sdk --target-os=oscortex --linux-cpu=arm64 --target-dir=oscortex_arm64_release"
    OUT="oscortex_arm64_release"
    ;;
  *)
    echo "usage: $0 [release]" >&2; exit 1 ;;
esac

# The engine bundles its own ninja; depot_tools' ninja wrapper can't find it
# without a CIPD dep, so call the vendored binary directly (same one the x64
# release build used).
NINJA="/work/engine/third_party/ninja/ninja"

echo "[build-arm64] mode=$MODE out=out/$OUT (cross-compile x64 host -> arm64 target)"
docker exec "$CONTAINER" bash -c "
  set -e
  export PATH=/work/depot_tools:\$PATH
  export DEPOT_TOOLS_UPDATE=0 DEPOT_TOOLS_METRICS=0
  cd /work/engine/engine/src
  echo '[build-arm64] gn configure...'
  python3 flutter/tools/gn $GN_ARGS
  echo '[build-arm64] confirming target_cpu...'
  grep -E 'target_cpu|dart_target_arch|is_oscortex|flutter_runtime_mode' out/$OUT/args.gn
  echo '[build-arm64] ninja (flutter_engine gen_snapshot) — this is the long pole...'
  $NINJA -C out/$OUT flutter_engine gen_snapshot
"
echo "[build-arm64] DONE"
echo "[build-arm64]   engine: /work/engine/engine/src/out/$OUT/libflutter_engine.so"
echo "[build-arm64]   gen_snapshot: /work/engine/engine/src/out/$OUT/gen_snapshot"
echo "[build-arm64] verify arch: docker exec $CONTAINER file /work/engine/engine/src/out/$OUT/libflutter_engine.so"
