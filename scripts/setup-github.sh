#!/usr/bin/env bash
# setup-github.sh — one-time GitHub bootstrap for OSCortex CI/CD compliance.
#
#   1. Ensures a `develop` integration branch exists (branched from main).
#   2. Makes `develop` the default branch (day-to-day work; main = release-only).
#   3. Applies branch protection to `main` and `develop`:
#        - PR required (no direct pushes)
#        - required status checks (CI jobs) must pass + branch up to date
#        - CODEOWNERS review required
#        - linear history, no force-push, no deletion
#
# Prerequisite:  gh auth login   (token/login needs ADMIN on the repo)
# Usage:         bash scripts/setup-github.sh
#
# Note: branch protection requires a public repo or GitHub Pro/Team/Enterprise
# for private repos. The script reports per-call errors instead of dying.
set -euo pipefail

if ! command -v gh >/dev/null 2>&1; then
  echo "ERROR: GitHub CLI (gh) not installed — https://cli.github.com" >&2
  exit 1
fi
if ! gh auth status >/dev/null 2>&1; then
  echo "ERROR: not authenticated. Run: gh auth login" >&2
  exit 1
fi

REPO="$(gh repo view --json nameWithOwner -q .nameWithOwner)"
echo "Repository: $REPO"

# Required CI checks — must match `name:` fields in ci.yml. The lint job is
# advisory (continue-on-error) until the tree is fmt/clippy clean, so it is NOT
# required here; add "rustfmt + clippy (advisory)" after the cleanup pass.
CHECKS='["Kernel build + unit tests","Flutter shell analyze + test"]'

# ---------------------------------------------------------------------------
# 1+2. develop branch + default branch
# ---------------------------------------------------------------------------
if gh api "repos/$REPO/branches/develop" >/dev/null 2>&1; then
  echo "develop: already exists"
else
  echo "develop: creating from main..."
  main_sha="$(gh api "repos/$REPO/git/refs/heads/main" -q .object.sha)"
  gh api -X POST "repos/$REPO/git/refs" \
    -f ref="refs/heads/develop" -f sha="$main_sha" >/dev/null
  echo "develop: created"
fi

echo "Setting default branch to develop..."
gh api -X PATCH "repos/$REPO" -f default_branch="develop" >/dev/null \
  && echo "default branch: develop" \
  || echo "WARN: could not set default branch (insufficient perms?)"

# ---------------------------------------------------------------------------
# 3. Branch protection
# ---------------------------------------------------------------------------
protect_branch() {
  local branch="$1" reviewers="$2"
  echo "Protecting $branch..."
  if gh api -X PUT "repos/$REPO/branches/$branch/protection" \
      -H "Accept: application/vnd.github+json" --input - >/dev/null <<JSON
{
  "required_status_checks": { "strict": true, "contexts": $CHECKS },
  "enforce_admins": true,
  "required_pull_request_reviews": {
    "required_approving_review_count": $reviewers,
    "require_code_owner_reviews": true,
    "dismiss_stale_reviews": true
  },
  "restrictions": null,
  "required_linear_history": true,
  "allow_force_pushes": false,
  "allow_deletions": false
}
JSON
  then
    echo "  $branch: protected"
  else
    echo "  WARN: protection failed for $branch (private repo without Pro/Team, or missing admin)"
  fi
}

protect_branch "main" 1
protect_branch "develop" 1

echo ""
echo "Done. Flow: feature/* -> PR -> develop -> release PR -> main -> auto release."
echo "See docs/ci-cd.txt for the full model."
