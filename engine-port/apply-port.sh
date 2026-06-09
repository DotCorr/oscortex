#!/usr/bin/env bash
#
# apply-port.sh — apply the OSCortex engine-port (patches + backend sources) into
# a FRESH engine checkout produced by setup-engine-build.sh. Idempotent-ish:
# patches that are already applied are skipped (via `git apply --check`).
#
# Run this after setup-engine-build.sh and before `build-engine.sh oscortex`.
set -euo pipefail

CONTAINER="oscx-engine"
REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PATCHES="$REPO_ROOT/engine-port/patches"
SRC="$REPO_ROOT/engine-port/src"

apply_patch() { # $1 = patch file, $2 = repo dir inside container (relative to /work/engine/engine/src)
  local patch="$1" reldir="$2" name
  name="$(basename "$patch")"
  docker cp "$patch" "$CONTAINER":/tmp/"$name"
  docker exec "$CONTAINER" bash -c "
    cd /work/engine/engine/src/$reldir
    if git apply --check /tmp/$name 2>/dev/null; then
      git apply /tmp/$name && echo '[apply-port] applied $name'
    elif git apply --reverse --check /tmp/$name 2>/dev/null; then
      echo '[apply-port] already applied: $name (skip)'
    else
      echo '[apply-port] WARN: $name does not apply cleanly' >&2
    fi
  "
}

# 1. GN target plumbing.
#    0001 is rooted at the flutter monorepo root (paths engine/src/flutter/...).
docker cp "$PATCHES/0001-gn-add-oscortex-target.patch" "$CONTAINER":/tmp/0001.patch
docker exec "$CONTAINER" bash -c '
  cd /work/engine
  if git apply --check /tmp/0001.patch 2>/dev/null; then git apply /tmp/0001.patch && echo "[apply-port] applied 0001 (gn target)";
  elif git apply --reverse --check /tmp/0001.patch 2>/dev/null; then echo "[apply-port] already applied: 0001 (skip)";
  else echo "[apply-port] WARN: 0001 does not apply cleanly" >&2; fi
'
#    0002 is rooted at the build/ dep.
apply_patch "$PATCHES/0002-buildconfig-is-oscortex.patch" "build"

#    0003 (fml backend wiring: build_config.h, message_loop_impl.cc, fml/BUILD.gn)
#    is rooted at the flutter monorepo root, like 0001.
docker cp "$PATCHES/0003-fml-oscortex-backend.patch" "$CONTAINER":/tmp/0003.patch
docker exec "$CONTAINER" bash -c '
  cd /work/engine
  if git apply --check /tmp/0003.patch 2>/dev/null; then git apply /tmp/0003.patch && echo "[apply-port] applied 0003 (fml backend)";
  elif git apply --reverse --check /tmp/0003.patch 2>/dev/null; then echo "[apply-port] already applied: 0003 (skip)";
  else echo "[apply-port] WARN: 0003 does not apply cleanly" >&2; fi
'

# 2. OSCortex platform backend sources (Phase 2 — copied 1:1 into the checkout).
if [ -d "$SRC" ]; then
  docker exec "$CONTAINER" bash -c 'mkdir -p /work/engine/engine/src/flutter/fml/platform/oscortex'
  for f in $(cd "$SRC" && find . -type f); do
    docker cp "$SRC/$f" "$CONTAINER":/work/engine/engine/src/"$f"
  done
  echo "[apply-port] copied backend sources from engine-port/src/"
fi

echo "[apply-port] done. Next: engine-port/build-engine.sh oscortex"
