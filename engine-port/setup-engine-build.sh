#!/usr/bin/env bash
#
# setup-engine-build.sh — one-shot, idempotent setup of the Flutter engine build
# environment used by the OSCortex native engine port (docs/native-engine-port.md).
#
# Why this exists: building the Flutter engine from source is repetitive, heavy
# infra (depot_tools + a ~22 GB gclient checkout + a linux/amd64 toolchain). This
# script captures the EXACT validated flow so any contributor can reproduce it
# with one command instead of rediscovering the pitfalls (e.g. the .gclient must
# use name="." per engine/scripts/standard.gclient — name="flutter" mis-nests it).
#
# Host requirements: Docker (running). On Apple Silicon the linux/amd64 build runs
# under emulation (slow but correct). Disk: keep ~40 GB free for checkout+build.
#
# Usage:
#   engine-port/setup-engine-build.sh [WORKSPACE_DIR]
# WORKSPACE_DIR defaults to a sibling of the repo: ../oscortex-engine-build
# (kept OUTSIDE the repo on purpose — the 22 GB checkout is never committed).
set -euo pipefail

# --- config (pinned to the Flutter SDK this repo targets) -------------------
FLUTTER_REV="582a0e7c5581dc0ca5f7bfd8662bb8db6f59d536"   # Flutter 3.41.1
ENGINE_HASH="cc8e596aa65130a0678cc59613ed1c5125184db4"   # engine (for reference)
CONTAINER="oscx-engine"
PLATFORM="linux/amd64"

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
WORKSPACE="${1:-$(dirname "$REPO_ROOT")/oscortex-engine-build}"
mkdir -p "$WORKSPACE"
echo "[setup] workspace: $WORKSPACE"
echo "[setup] flutter rev: $FLUTTER_REV"

# --- 1. persistent build container with the workspace bind-mounted ----------
if ! docker ps --filter "name=^${CONTAINER}$" --format '{{.Names}}' | grep -q "$CONTAINER"; then
  docker rm -f "$CONTAINER" >/dev/null 2>&1 || true
  docker run -d --name "$CONTAINER" --platform "$PLATFORM" \
    -v "$WORKSPACE":/work ubuntu:22.04 sleep infinity
  echo "[setup] started container $CONTAINER ($PLATFORM)"
else
  echo "[setup] container $CONTAINER already running"
fi

# --- 2. build deps + depot_tools (idempotent) -------------------------------
docker exec "$CONTAINER" bash -c '
  set -e
  export DEBIAN_FRONTEND=noninteractive
  if ! command -v git >/dev/null; then
    apt-get update -qq
    apt-get install -y -qq git curl python3 python3-pip build-essential \
      pkg-config ca-certificates lsb-release sudo xz-utils unzip zip >/tmp/apt.log 2>&1
  fi
  if [ ! -d /work/depot_tools ]; then
    git clone --depth 1 https://chromium.googlesource.com/chromium/tools/depot_tools.git /work/depot_tools >/tmp/depot.log 2>&1
  fi
  git config --global user.email "build@oscortex.local"
  git config --global user.name "oscx build"
  git config --global --add safe.directory "*"
'
echo "[setup] deps + depot_tools ready"

# --- 3. pinned .gclient (name="." is REQUIRED — see standard.gclient) -------
docker exec "$CONTAINER" bash -c "
  mkdir -p /work/engine
  cat > /work/engine/.gclient <<EOF
solutions = [
  {
    \"custom_deps\": {},
    \"deps_file\": \"DEPS\",
    \"managed\": False,
    \"name\": \".\",
    \"safesync_url\": \"\",
    \"url\": \"https://github.com/flutter/flutter.git@${FLUTTER_REV}\",
    \"custom_vars\": { \"download_emsdk\": False },
  },
]
EOF
"
echo "[setup] .gclient written (name='.', pinned)"

# --- 4. gclient sync (the big one: ~22 GB + hooks; shallow to save disk) -----
echo "[setup] running gclient sync (this takes a while; log: \$WORKSPACE/gclient-sync.log)"
docker exec "$CONTAINER" bash -c '
  export PATH=/work/depot_tools:$PATH
  export DEPOT_TOOLS_UPDATE=0 DEPOT_TOOLS_METRICS=0 GCLIENT_SUPPRESS_GIT_VERSION_WARNING=1
  cd /work/engine
  gclient sync --no-history --shallow > /work/gclient-sync.log 2>&1
'
echo "[setup] gclient sync DONE"

echo "[setup] COMPLETE. Next: engine-port/build-engine.sh"
