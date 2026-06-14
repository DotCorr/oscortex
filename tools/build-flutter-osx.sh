#!/usr/bin/env bash
# Build a Flutter project into an OSCortex .osx bundle plus optional JIT assets.
set -euo pipefail

if [ "$#" -lt 3 ]; then
    echo "Usage: $0 <flutter_app_dir> <display_name> <output.osx> [assets_output_dir]" >&2
    exit 2
fi

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
APP_DIR="$(cd "$1" && pwd)"
DISPLAY_NAME="$2"
OUTPUT_OSX="$3"
ASSETS_OUTPUT_DIR="${4:-}"

FLUTTER_BIN="${FLUTTER_BIN:-flutter}"
FLUTTER_HOME="${FLUTTER_HOME:-/opt/homebrew/share/flutter}"
DARTAOT="$FLUTTER_HOME/bin/cache/dart-sdk/bin/dartaotruntime"
FRONTEND_SERVER="$FLUTTER_HOME/bin/cache/artifacts/engine/darwin-x64/frontend_server_aot.dart.snapshot"
SDK_ROOT_PRODUCT="$FLUTTER_HOME/bin/cache/artifacts/engine/common/flutter_patched_sdk/"
# Use the gen_snapshot that MATCHES the shipped x86_64 engine
# (tools/flutter-engine/libflutter_engine.so). The two x64 gen_snapshots are
# different Dart build flavors: tools/flutter-engine/linux-x64/gen_snapshot emits
# a 'release' snapshot, but the shipped engine is 'product' — that mismatch
# ("snapshot requires release but the VM has product") left the x86 shell unable
# to load its Dart code → blank screen. The matched (product) gen_snapshot ships
# in the oscortex-x64-release engine bundle.
GEN_SNAP="$ROOT/.engine-cache/oscortex-engine-1/oscortex-x64-release/gen_snapshot"
if [ ! -x "$GEN_SNAP" ]; then
    GEN_SNAP="$ROOT/tools/flutter-engine/linux-x64/gen_snapshot"
fi

PKG_CONFIG="$APP_DIR/.dart_tool/package_config.json"
APP_MAIN="$APP_DIR/lib/main.dart"
AOT_DILL="$APP_DIR/build/app_aot.dill"
LIBAPP_SO="$APP_DIR/build/oscortex/libapp.so"

echo "[osx] Flutter pub get: $APP_DIR"
(cd "$APP_DIR" && "$FLUTTER_BIN" pub get)

echo "[osx] Building debug/JIT bundle assets: $DISPLAY_NAME"
(cd "$APP_DIR" && "$FLUTTER_BIN" --suppress-analytics build bundle --debug)

echo "[osx] Compiling AOT dill: $DISPLAY_NAME"
"$DARTAOT" "$FRONTEND_SERVER" \
    --sdk-root "$SDK_ROOT_PRODUCT" \
    --target=flutter \
    --aot \
    --tfa \
    --packages="$PKG_CONFIG" \
    --output-dill "$AOT_DILL" \
    "$APP_MAIN" 2>&1 | grep -v '^+' | tail -5

if [ ! -f "$AOT_DILL" ]; then
    echo "ERROR: AOT kernel compilation failed for $DISPLAY_NAME" >&2
    exit 1
fi

mkdir -p "$(dirname "$LIBAPP_SO")"
docker run --rm --platform linux/amd64 \
    -v "$ROOT:$ROOT" \
    -w "$ROOT" \
    ubuntu:22.04 \
    "$GEN_SNAP" \
    --deterministic \
    --snapshot_kind=app-aot-elf \
    --elf="$LIBAPP_SO" \
    --dedup_instructions \
    --strip \
    "$AOT_DILL" 2>&1

python3 "$ROOT/tools/flutter-engine/patch_libapp.py" "$LIBAPP_SO"

python3 "$ROOT/tools/oscortex-pack.py" \
    "$LIBAPP_SO" \
    "$OUTPUT_OSX" \
    --name "$DISPLAY_NAME"

if [ -n "$ASSETS_OUTPUT_DIR" ]; then
    APP_ASSETS_DIR="$APP_DIR/build/flutter_assets"
    if [ ! -d "$APP_ASSETS_DIR" ]; then
        echo "ERROR: Flutter assets missing for $DISPLAY_NAME: $APP_ASSETS_DIR" >&2
        exit 1
    fi
    rm -rf "$ASSETS_OUTPUT_DIR"
    mkdir -p "$ASSETS_OUTPUT_DIR"
    cp -R "$APP_ASSETS_DIR"/. "$ASSETS_OUTPUT_DIR"/
    python3 "$ROOT/tools/flutter-engine/engine_patch.py" \
        --kernel-blob "$ASSETS_OUTPUT_DIR/kernel_blob.bin"
fi

echo "[osx] Built $OUTPUT_OSX"
