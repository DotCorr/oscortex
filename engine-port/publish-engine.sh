#!/usr/bin/env bash
#
# publish-engine.sh — package the built engine artifacts and upload them to R2.
# MAINTAINER flow: run after build-engine.sh, when you've changed engine-port/ or
# bumped the Flutter pin (remember to bump ARTIFACT_VERSION in artifact.config).
# Everyone else uses fetch-engine.sh.
#
# Usage:  engine-port/publish-engine.sh [arch] [mode]
#   arch: x64 (default) | arm64 | riscv64
#   mode: release (default) | debug
#
# Upload backend (pick one):
#   - rclone remote named 'r2'  (rclone config; R2 is S3-compatible), OR
#   - aws CLI + R2_ENDPOINT + R2 creds in the env.
# Set R2_BUCKET (default: oscortex-engine).
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
# shellcheck source=/dev/null
source "$REPO_ROOT/engine-port/artifact.config"

CONTAINER="oscx-engine"
ARCH="${1:-x64}"
MODE="${2:-release}"
NAME="oscortex-${ARCH}-${MODE}"
# GN out-dir per target. The dir names differ by arch (arm64 was configured as a
# plain linux release; x64 via the oscortex target), so map explicitly rather
# than assuming a single dir.
case "$ARCH-$MODE" in
  arm64-release) OUTDIR="out/linux_release_arm64" ;;
  x64-release)   OUTDIR="out/oscortex_release_x64" ;;
  x64-debug)     OUTDIR="out/oscortex_debug_unopt_x64" ;;
  *)             OUTDIR="out/oscortex_${MODE}" ;;
esac
R2_BUCKET="${R2_BUCKET:-oscortex-engine}"
KEY="$ARTIFACT_VERSION/${NAME}.tar.gz"

# 1. Collect + tar the artifacts inside the build container.
docker exec "$CONTAINER" bash -c "
  set -e
  S=/work/_publish/$NAME
  rm -rf \$S && mkdir -p \$S
  cd /work/engine/engine/src
  cp $OUTDIR/libflutter_engine.so \$S/
  [ -f $OUTDIR/gen_snapshot ] && cp $OUTDIR/gen_snapshot \$S/ || true
  if [ -f $OUTDIR/icudtl.dat ]; then cp $OUTDIR/icudtl.dat \$S/;
  elif [ -f flutter/third_party/icu/flutter/icudtl.dat ]; then cp flutter/third_party/icu/flutter/icudtl.dat \$S/; fi
  cat > \$S/MANIFEST.txt <<EOF
artifact_version=$ARTIFACT_VERSION
target=$NAME
flutter_version=$FLUTTER_VERSION
flutter_rev=$FLUTTER_REV
engine_hash=$ENGINE_HASH
EOF
  cd /work/_publish
  tar -czf ${NAME}.tar.gz -C $NAME .
  sha256sum ${NAME}.tar.gz | awk '{print \$1}' > ${NAME}.tar.gz.sha256
  echo '[publish] packaged' \$(ls -lah /work/_publish/${NAME}.tar.gz | awk '{print \$5}')
"

# Resolve the host path bind-mounted to the container's /work (robust regardless
# of the workspace dir name).
WS="$(docker inspect "$CONTAINER" --format '{{ range .Mounts }}{{ if eq .Destination "/work" }}{{ .Source }}{{ end }}{{ end }}' 2>/dev/null)"
[ -n "$WS" ] || WS="$(dirname "$REPO_ROOT")/oscortex-engine-build"
TARBALL="$WS/_publish/${NAME}.tar.gz"
echo "[publish] tarball: $TARBALL  (key: $KEY)"

# 2. Upload to the artifact host. Prefer GitHub Releases (public repo, free, no
# egress) -> then GCS -> then R2/S3.
if [ -n "${GITHUB_REPO:-}" ] && command -v gh >/dev/null && gh auth status >/dev/null 2>&1; then
  TAG="$ARTIFACT_VERSION"
  gh release view "$TAG" -R "$GITHUB_REPO" >/dev/null 2>&1 || \
    gh release create "$TAG" -R "$GITHUB_REPO" \
      --title "OSCortex engine $ARTIFACT_VERSION" \
      --notes "Prebuilt OSCortex Flutter engine artifacts (Flutter $FLUTTER_VERSION / engine $ENGINE_HASH). Fetched by engine-port/fetch-engine.sh — see engine-port/README.md."
  gh release upload "$TAG" "$TARBALL" "$TARBALL.sha256" -R "$GITHUB_REPO" --clobber
  echo "[publish] uploaded via gh release -> $GITHUB_REPO @ $TAG / ${NAME}.tar.gz"
  echo "[publish] public: $ARTIFACT_BASE_URL/$ARTIFACT_VERSION/${NAME}.tar.gz"
  exit 0
fi
if [ -n "${GCS_BUCKET:-}" ] && command -v gcloud >/dev/null && gcloud storage ls "gs://$GCS_BUCKET" >/dev/null 2>&1; then
  gcloud storage cp "$TARBALL"        "gs://$GCS_BUCKET/$KEY"
  gcloud storage cp "$TARBALL.sha256" "gs://$GCS_BUCKET/$KEY.sha256"
  echo "[publish] uploaded via gcloud storage -> gs://$GCS_BUCKET/$KEY"
  echo "[publish] public: $ARTIFACT_BASE_URL/$KEY"
  exit 0
fi

# (R2/S3 fallbacks) Prefer wrangler (native R2, via npx — no install needed).
WR="npx --yes wrangler@latest"
if $WR whoami >/dev/null 2>&1; then
  $WR r2 object put "${R2_BUCKET}/${KEY}"        --file "$TARBALL"        --remote
  $WR r2 object put "${R2_BUCKET}/${KEY}.sha256" --file "$TARBALL.sha256" --remote
  echo "[publish] uploaded via wrangler -> ${R2_BUCKET}/${KEY}"
elif command -v rclone >/dev/null && rclone listremotes 2>/dev/null | grep -q '^r2:'; then
  rclone copyto "$TARBALL"         "r2:${R2_BUCKET}/${KEY}"
  rclone copyto "$TARBALL.sha256"  "r2:${R2_BUCKET}/${KEY}.sha256"
  echo "[publish] uploaded via rclone -> r2:${R2_BUCKET}/${KEY}"
elif [ -n "${R2_ENDPOINT:-}" ] && command -v aws >/dev/null; then
  aws s3 cp "$TARBALL"        "s3://${R2_BUCKET}/${KEY}"        --endpoint-url "$R2_ENDPOINT"
  aws s3 cp "$TARBALL.sha256" "s3://${R2_BUCKET}/${KEY}.sha256" --endpoint-url "$R2_ENDPOINT"
  echo "[publish] uploaded via aws/S3 -> ${KEY}"
else
  cat >&2 <<EOF
[publish] Tarball ready, but no R2 uploader configured.
  Option A: rclone config a remote named 'r2' (S3-compatible to your R2 bucket), then re-run.
  Option B: export R2_ENDPOINT=https://<acct>.r2.cloudflarestorage.com + AWS_* creds, then re-run.
  Manual:   upload $TARBALL (+ .sha256) to R2 at object key: $KEY
  After upload, set ARTIFACT_BASE_URL (artifact.config) to the public URL so fetch-engine.sh works.
EOF
  exit 3
fi
