#!/usr/bin/env bash
#
# build-engine.sh — configure (GN) + build (ninja) the Flutter engine in the
# container set up by setup-engine-build.sh.
#
# Usage:
#   engine-port/build-engine.sh baseline        # host linux-x64 debug (Phase 0 toolchain proof)
#   engine-port/build-engine.sh oscortex        # the OSCortex native target (Phase 1+)
#
# Notes:
#  - --prebuilt-dart-sdk: use the prebuilt Dart SDK (skips building Dart from
#    source = much faster under emulation). Only drop it if editing Dart sources.
#  - --no-rbe/--no-goma: no remote build backend available; build locally.
#  - OSCortex renders in SOFTWARE, so the oscortex target drops Impeller (GPU) —
#    the slowest ~300 objects of every build (handled in the GN target config).
set -euo pipefail

CONTAINER="oscx-engine"
TARGET="${1:-baseline}"

case "$TARGET" in
  baseline)
    GN_ARGS="--unoptimized --runtime-mode=debug --no-rbe --no-goma --prebuilt-dart-sdk --linux-cpu=x64"
    OUT="host_debug_unopt_x64"
    NINJA_TARGET="flutter/shell/platform/embedder:flutter_engine"
    ;;
  oscortex)
    # The 'oscortex' --target-os is added by apply-port.sh (engine-port/patches).
    GN_ARGS="--unoptimized --runtime-mode=debug --no-rbe --no-goma --prebuilt-dart-sdk --target-os=oscortex --linux-cpu=x64"
    OUT="oscortex_debug_unopt_x64"
    NINJA_TARGET="flutter/shell/platform/embedder:flutter_engine"
    ;;
  *)
    echo "usage: $0 [baseline|oscortex]" >&2; exit 1 ;;
esac

echo "[build] target=$TARGET out=out/$OUT"
docker exec "$CONTAINER" bash -c "
  set -e
  export PATH=/work/depot_tools:\$PATH
  cd /work/engine/engine/src
  echo '[build] gn configure...'
  python3 flutter/tools/gn $GN_ARGS
  echo '[build] ninja ($NINJA_TARGET)...'
  ninja -C out/$OUT $NINJA_TARGET
"
echo "[build] DONE -> out/$OUT/libflutter_engine.so"
