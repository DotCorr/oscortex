#!/usr/bin/env bash
#
# fetch-engine.sh — download the pinned prebuilt OSCortex engine artifact and
# stage it where the OSCortex build expects it. THIS is what devs + CI run.
# No gclient checkout, no hour-long build — seconds.
#
# Usage:  engine-port/fetch-engine.sh [arch] [mode]
#   arch: x64 (default) | arm64 | riscv64
#   mode: release (default) | debug
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
# shellcheck source=/dev/null
source "$REPO_ROOT/engine-port/artifact.config"

ARCH="${1:-x64}"
MODE="${2:-release}"
NAME="oscortex-${ARCH}-${MODE}"
URL="$ARTIFACT_BASE_URL/$ARTIFACT_VERSION/${NAME}.tar.gz"
CACHE="$REPO_ROOT/.engine-cache/$ARTIFACT_VERSION/$NAME"

if [ -f "$CACHE/libflutter_engine.so" ]; then
  echo "[fetch-engine] using cache: $CACHE"
else
  if [[ "$ARTIFACT_BASE_URL" == *CHANGEME* ]]; then
    cat >&2 <<EOF
[fetch-engine] No artifact host configured yet.
  No prebuilt engine has been published. Until then, build locally (maintainer flow):
      engine-port/setup-engine-build.sh   # once
      engine-port/build-engine.sh $MODE
  Once artifacts are published, set OSCORTEX_ENGINE_BASE_URL (or edit
  engine-port/artifact.config) to your R2 URL and re-run this.
EOF
    exit 2
  fi
  echo "[fetch-engine] downloading $URL"
  mkdir -p "$CACHE"
  tmp="$CACHE.tar.gz"
  curl -fSL "$URL" -o "$tmp"
  if curl -fSL "$URL.sha256" -o "$tmp.sha256" 2>/dev/null; then
    want="$(awk '{print $1}' "$tmp.sha256")"
    got="$(sha256sum "$tmp" | awk '{print $1}')"
    [ "$want" = "$got" ] || { echo "[fetch-engine] CHECKSUM MISMATCH ($got != $want)" >&2; exit 1; }
    echo "[fetch-engine] checksum OK"
  fi
  tar -xzf "$tmp" -C "$CACHE"
  rm -f "$tmp" "$tmp.sha256"
fi

# Stage into the locations the OSCortex build/runtime expect.
mkdir -p "$REPO_ROOT/initramfs/system/lib" \
         "$REPO_ROOT/initramfs/system/flutter" \
         "$REPO_ROOT/tools/flutter-engine/bin"
cp "$CACHE/libflutter_engine.so" "$REPO_ROOT/initramfs/system/lib/libflutter_engine.so"
cp "$CACHE/libflutter_engine.so" "$REPO_ROOT/tools/flutter-engine/libflutter_engine.so"
[ -f "$CACHE/icudtl.dat" ]    && cp "$CACHE/icudtl.dat" "$REPO_ROOT/initramfs/system/flutter/icudtl.dat"
[ -f "$CACHE/gen_snapshot" ]  && { cp "$CACHE/gen_snapshot" "$REPO_ROOT/tools/flutter-engine/bin/gen_snapshot"; chmod +x "$REPO_ROOT/tools/flutter-engine/bin/gen_snapshot"; }

echo "[fetch-engine] staged $NAME ($ARTIFACT_VERSION). No engine build needed — run the normal OS build."
