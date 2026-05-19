#!/usr/bin/env bash
# scripts/embed-flutter-engine.sh
# Copy a real Flutter Engine shared library into the initramfs so the kernel
# can dlopen it at runtime.
#
# Usage: ./scripts/embed-flutter-engine.sh <path-to-libflutter_engine.so>
#
# The script copies the .so into initramfs/system/lib/ and rebuilds the tar.
# After running this, do a full kernel rebuild so build.rs re-packs the tar.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
WORKSPACE_ROOT="$(dirname "$SCRIPT_DIR")"

if [ $# -lt 1 ]; then
    echo "Usage: $0 <path-to-libflutter_engine.so>" >&2
    echo ""
    echo "Download the engine for the correct architecture from:" >&2
    echo "  https://storage.googleapis.com/flutter_infra_release/flutter/<hash>/linux-x64/linux-x64-embedder" >&2
    exit 1
fi

SRC="$1"
if [ ! -f "$SRC" ]; then
    echo "ERROR: $SRC does not exist" >&2
    exit 1
fi

DEST_DIR="$WORKSPACE_ROOT/initramfs/system/lib"
mkdir -p "$DEST_DIR"

echo "[embed-flutter-engine] Copying $SRC → $DEST_DIR/libflutter_engine.so"
cp "$SRC" "$DEST_DIR/libflutter_engine.so"
echo "[embed-flutter-engine] Done. Run a full rebuild to re-pack the initramfs."
echo ""
echo "  cd $WORKSPACE_ROOT"
echo "  cargo +nightly build --package oscortex-kernel --target x86_64-unknown-none \\"
echo "      -Z build-std=core,compiler_builtins,alloc \\"
echo "      -Z build-std-features=compiler-builtins-mem"

