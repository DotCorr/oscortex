#!/usr/bin/env bash
#
# setup-r2.sh — create + configure the Cloudflare R2 bucket that hosts the
# prebuilt OSCortex engine artifacts, then wire its public URL into
# engine-port/artifact.config. Run ONCE (idempotent).
#
# PREREQUISITE — authenticate wrangler first (your Cloudflare account):
#     npx wrangler login            # browser OAuth, one time
#   OR export a token with R2 edit permission:
#     export CLOUDFLARE_API_TOKEN=...   (and CLOUDFLARE_ACCOUNT_ID=...)
#
# Uses npx (no global install). Bucket name: lowercase/digits/hyphens, 3-63 chars.
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
# shellcheck source=/dev/null
source "$REPO_ROOT/engine-port/artifact.config"
BUCKET="${R2_BUCKET:-oscortex-engine}"
WR="npx --yes wrangler@latest"

echo "[setup-r2] checking auth..."
if ! $WR whoami >/dev/null 2>&1; then
  echo "  Not authenticated. Run:  npx wrangler login   (or set CLOUDFLARE_API_TOKEN)" >&2
  exit 2
fi

echo "[setup-r2] creating bucket '$BUCKET' (ok if it already exists)..."
$WR r2 bucket create "$BUCKET" 2>/dev/null || echo "  (bucket already exists — fine)"

echo "[setup-r2] enabling the public r2.dev URL for '$BUCKET'..."
$WR r2 bucket dev-url enable "$BUCKET" >/dev/null 2>&1 || true
# r2.dev URL looks like https://pub-<hash>.r2.dev and serves bucket objects at its root.
URL="$($WR r2 bucket dev-url get "$BUCKET" 2>/dev/null | grep -oE 'https://[a-z0-9.-]+\.r2\.dev' | head -1 || true)"

if [ -z "$URL" ]; then
  cat >&2 <<EOF
[setup-r2] Bucket is ready but I couldn't auto-detect the public URL via the CLI.
  Get it from the dashboard: R2 > $BUCKET > Settings > Public access > r2.dev (enable),
  then set it manually in engine-port/artifact.config:
      ARTIFACT_BASE_URL="https://pub-XXXX.r2.dev"
EOF
  exit 3
fi

echo "[setup-r2] public base URL: $URL"
# Wire it into artifact.config (objects are at <URL>/<ARTIFACT_VERSION>/<target>.tar.gz).
sed -i.bak -E "s#ARTIFACT_BASE_URL=\"[^\"]*\"#ARTIFACT_BASE_URL=\"\${OSCORTEX_ENGINE_BASE_URL:-$URL}\"#" \
  "$REPO_ROOT/engine-port/artifact.config"
rm -f "$REPO_ROOT/engine-port/artifact.config.bak"

echo "[setup-r2] done. ARTIFACT_BASE_URL set. Next (maintainer): build-engine.sh oscortex && publish-engine.sh"
echo "[setup-r2] consumers can then: engine-port/fetch-engine.sh x64 release"
