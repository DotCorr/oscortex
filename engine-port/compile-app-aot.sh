#!/usr/bin/env bash
#
# compile-app-aot.sh — compile a Flutter app to a native AOT snapshot (libapp.so)
# for OSCortex. This is the per-app packaging step under AOT (replaces shipping a
# JIT kernel_blob.bin).
#
# Pipeline:  app .dart  --frontend_server-->  AOT kernel .dill
#                       --gen_snapshot------>  libapp.so (native ELF, real instructions)
#
# KEY: the gen_snapshot MUST be the one built from the same engine tree as the
# runtime engine (fetch-engine.sh stages it at tools/flutter-engine/bin/gen_snapshot).
# Engine + gen_snapshot version match is what makes AOT deserialize cleanly — a
# mismatch is what produced the old "Snapshot expects N base objects, provided 0"
# dead-end. See docs/native-engine-port.md.
#
# Usage:  engine-port/compile-app-aot.sh <app_dir> <out_libapp.so>
#   e.g.  engine-port/compile-app-aot.sh apps/oscortex_app initramfs/system/flutter/libapp.so
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
APP_DIR="${1:?usage: compile-app-aot.sh <app_dir> <out_libapp.so>}"
OUT="${2:?usage: compile-app-aot.sh <app_dir> <out_libapp.so>}"
CONTAINER="oscx-engine"
# Which built engine out/ dir's gen_snapshot to use. Defaults to the x86_64
# release. For an arm64 (aarch64) libapp.so, point this at the arm64 build:
#   OSCORTEX_ENGINE_OUT=oscortex_arm64_release engine-port/compile-app-aot.sh ...
# The gen_snapshot MUST match the engine the app runs against (same arch + tree).
ENGINE_OUT="${OSCORTEX_ENGINE_OUT:-oscortex_release}"

FL="${FLUTTER_ROOT:-/opt/homebrew/share/flutter}"
DARTAOT="$FL/bin/cache/dart-sdk/bin/dartaotruntime"
FRONTEND="$FL/bin/cache/artifacts/engine/darwin-x64/frontend_server_aot.dart.snapshot"
SDK_PRODUCT="$FL/bin/cache/artifacts/engine/common/flutter_patched_sdk_product/"
GEN_SNAP="$REPO_ROOT/tools/flutter-engine/bin/gen_snapshot"   # from fetch-engine.sh

[ -x "$DARTAOT" ]   || { echo "missing dartaotruntime ($DARTAOT)"   >&2; exit 1; }
[ -f "$FRONTEND" ]  || { echo "missing frontend_server ($FRONTEND)" >&2; exit 1; }
[ -f "$APP_DIR/.dart_tool/package_config.json" ] || ( cd "$APP_DIR" && "$FL/bin/flutter" pub get )

# Workspace bind-mounted into the build container (for the linux gen_snapshot).
WS="$(docker inspect "$CONTAINER" --format '{{ range .Mounts }}{{ if eq .Destination "/work" }}{{ .Source }}{{ end }}{{ end }}' 2>/dev/null)"
[ -n "$WS" ] || { echo "build container '$CONTAINER' not running (needed for gen_snapshot)" >&2; exit 1; }
DILL="$WS/app_aot.dill"

echo "[aot] 1/2 frontend_server: $APP_DIR/lib/main.dart -> app_aot.dill"
"$DARTAOT" "$FRONTEND" \
  --sdk-root "$SDK_PRODUCT" --target=flutter --aot --tfa \
  --packages="$APP_DIR/.dart_tool/package_config.json" \
  --output-dill "$DILL" \
  "$APP_DIR/lib/main.dart" 2>&1 | grep -vE '^\+|warning' | tail -3

echo "[aot] 2/2 gen_snapshot (version-matched): app_aot.dill -> libapp.so"
# Prefer the staged gen_snapshot; else use the one in the build container.
if [ -x "$GEN_SNAP" ]; then
  echo "[aot]   (note: staged gen_snapshot is linux-x64; running it via the build container)"
fi
docker exec "$CONTAINER" "/work/engine/engine/src/out/$ENGINE_OUT/gen_snapshot" \
  --snapshot_kind=app-aot-elf --elf=/work/libapp.so --strip /work/app_aot.dill

mkdir -p "$(dirname "$OUT")"
cp "$WS/libapp.so" "$OUT"
echo "[aot] done -> $OUT ($(ls -lah "$OUT" | awk '{print $5}'))"
