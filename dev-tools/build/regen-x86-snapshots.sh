#!/usr/bin/env bash
# Regenerate PRODUCT-mode x86_64 AOT snapshots (shell + bundled apps) from the
# prebuilt AOT dills, using a gen_snapshot that MATCHES the shipped release x64
# engine. Needed when the prebuilt x86 libapp.so files are the wrong VM mode
# (the Dart VM rejects a 'release' snapshot under a 'product' engine).
#
# Prereqs: the `oscx-engine` build container with a built product gen_snapshot at
# out/oscortex_release_x64/gen_snapshot (build it with
# `ninja -C out/oscortex_release_x64 gen_snapshot`), and the per-app AOT dills at
# apps/<app>/build/app_aot.dill.
#
# GOTCHA: this gen_snapshot rejects `--dedup_instructions` as an unknown VM flag —
# do not pass it.
#
# Usage:  dev-tools/build/regen-x86-snapshots.sh
set -euo pipefail

ROOT="$(git -C "$(dirname "${BASH_SOURCE[0]}")" rev-parse --show-toplevel)"
CONTAINER="${OSCX_ENGINE_CONTAINER:-oscx-engine}"
GEN="${X64_GEN_SNAPSHOT:-/work/engine/engine/src/out/oscortex_release_x64/gen_snapshot}"

regen() { # <app-dir-name>
  local app="$1"
  local dill="$ROOT/apps/$app/build/app_aot.dill"
  [ -f "$dill" ] || { echo "  MISSING dill: $dill" >&2; return 1; }
  echo "[regen-x86] $app: app_aot.dill → product-mode libapp.so"
  docker cp "$dill" "$CONTAINER:/work/${app}_aot.dill"
  docker exec "$CONTAINER" bash -lc \
    "$GEN --deterministic --snapshot_kind=app-aot-elf --elf=/work/${app}_x64.so --strip /work/${app}_aot.dill" 2>&1 | tail -1
  docker cp "$CONTAINER:/work/${app}_x64.so" "$ROOT/apps/$app/build/oscortex/libapp.so"
  echo "  -> $(file "$ROOT/apps/$app/build/oscortex/libapp.so" | grep -oE 'x86-64') $(stat -f %z "$ROOT/apps/$app/build/oscortex/libapp.so" 2>/dev/null || stat -c %s "$ROOT/apps/$app/build/oscortex/libapp.so")b"
}

regen oscortex_app
regen oscortex_files
regen oscortex_canvas
regen oscortex_web_link
echo "[regen-x86] done. Stage with: X86_AOT=1 SKIP_CORE_APPS=1 scripts/build-iso.sh"
