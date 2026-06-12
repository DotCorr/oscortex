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
GEN_SNAP="$ROOT/tools/flutter-engine/linux-x64/gen_snapshot"

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
# Locate native gen_snapshot if possible to avoid Docker overhead/deadlocks.
HOST_GEN_SNAP=""
for path in \
    "$FLUTTER_HOME/bin/cache/artifacts/engine/android-x64-release/darwin-x64/gen_snapshot" \
    "/Users/ghostportal/sdks/flutter344/bin/cache/artifacts/engine/android-x64-release/darwin-x64/gen_snapshot" \
    "/opt/homebrew/Caskroom/flutter/3.38.9/flutter/bin/cache/artifacts/engine/android-x64-release/darwin-x64/gen_snapshot"; do
    if [ -f "$path" ]; then
        HOST_GEN_SNAP="$path"
        break
    fi
done

if [ -n "$HOST_GEN_SNAP" ]; then
    echo "[osx] Using native host gen_snapshot: $HOST_GEN_SNAP"
    "$HOST_GEN_SNAP" \
        --deterministic \
        --snapshot_kind=app-aot-elf \
        --elf="$LIBAPP_SO" \
        --strip \
        "$AOT_DILL" 2>&1
else
    echo "[osx] Falling back to Docker-based gen_snapshot..."
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
fi

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
