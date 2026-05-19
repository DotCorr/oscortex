#!/usr/bin/env bash
# scripts/make-initramfs.sh
# Manually pack the initramfs/ directory into initramfs.tar at the workspace root.
# This is used when you want to pre-stage a tar before the kernel build, e.g.
# to include a real `init` binary.
#
# Usage: ./scripts/make-initramfs.sh [initramfs_dir] [output_tar]
#   initramfs_dir defaults to ./initramfs
#   output_tar    defaults to ./initramfs.tar
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
WORKSPACE_ROOT="$(dirname "$SCRIPT_DIR")"

INITRAMFS_DIR="${1:-$WORKSPACE_ROOT/initramfs}"
OUTPUT_TAR="${2:-$WORKSPACE_ROOT/initramfs.tar}"

if [ ! -d "$INITRAMFS_DIR" ]; then
    echo "[make-initramfs] ERROR: $INITRAMFS_DIR does not exist" >&2
    exit 1
fi

echo "[make-initramfs] Packing $INITRAMFS_DIR → $OUTPUT_TAR"

# Use GNU tar if available (supports --posix / --format=posix for USTAR).
# On macOS, homebrew provides gtar.
if command -v gtar &>/dev/null; then
    TAR=gtar
elif tar --version 2>&1 | grep -q GNU; then
    TAR=tar
else
    # BSD tar on macOS also supports USTAR with --format=ustar.
    TAR=tar
fi

# Build the tar from the initramfs/ directory.
(cd "$INITRAMFS_DIR" && \
    find . \( -name '.gitkeep' -o -name '.DS_Store' \) -prune -o -print | \
    grep -v '^\.$' | \
    sort | \
    $TAR --format=ustar -c --files-from=- -f "$OUTPUT_TAR" 2>/dev/null || \
    $TAR -c --format=ustar -T <(find . \( -name '.gitkeep' -o -name '.DS_Store' \) -prune -o -print | grep -v '^\.$' | sort) -f "$OUTPUT_TAR" \
) || {
    # Fallback: plain tar without explicit format flag.
    (cd "$INITRAMFS_DIR" && tar -cf "$OUTPUT_TAR" \
        $(find . -not -name '.gitkeep' -not -name '.DS_Store' -not -path '.' | sort))
}

echo "[make-initramfs] Done: $(wc -c < "$OUTPUT_TAR") bytes"

