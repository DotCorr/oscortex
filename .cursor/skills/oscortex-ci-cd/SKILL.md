---
name: oscortex-ci-cd
description: >
  OSCortex Git branching, CI gates, PR compliance, and production release flow.
  Use when committing, pushing, opening PRs, merging to develop/main, releasing
  ISOs, setting up GitHub Actions, or any question about which branch to work on.
---

# OSCortex CI/CD & Branching

**Read first:** [docs/ci-cd.txt](../../../docs/ci-cd.txt) · [.github/pull_request_template.md](../../../.github/pull_request_template.md)

**Related skills:** [oscortex-architecture](../oscortex-architecture/SKILL.md) · [oscortex-kernel-tests](../oscortex-kernel-tests/SKILL.md)

**Repo:** `DotCorr/oscortex` · default branch: **`develop`** · production: **`main`**

---

## One-sentence model

**Day-to-day work lands on `develop` behind CI; `main` is production-only — every merge to `main` auto-cuts `vX.Y.Z` and publishes `oscortex-vX.Y.Z.iso`.**

---

## Branch map (memorize)

```
feature/*  fix/*     branch OFF develop (or main for hotfix only)
    │  PR + CI must pass
    ▼
develop              DEFAULT — integration, always green
    │  release PR (reviewed)
    ▼
main                 PRODUCTION — protected, release-only
    │  push/merge → release.yml
    ▼
GitHub Release       vX.Y.Z + oscortex-vX.Y.Z.iso + .sha256
```

| Branch | Role | Direct push? |
|--------|------|--------------|
| `develop` | Integration / daily dev target | **No** — PR only |
| `main` | Shipped production | **No** — release PR only |
| `feature/*`, `fix/*` | Short-lived work branches | Yes (push freely) |

---

## Agent start-of-task checklist

Before editing code:

1. **Confirm branch:** `git branch --show-current`
   - ✅ `develop` or `feature/*` / `fix/*` branched from `develop`
   - ❌ `main` — switch off immediately unless doing a hotfix PR
2. **Sync:** `git fetch origin && git checkout develop && git pull origin develop` (when starting fresh)
3. **New work:** `git checkout develop && git checkout -b feat/short-name`

Never begin feature work on `main`.

---

## Agent end-of-task checklist (commit → remote)

When the user asks to commit or work is ready:

1. **Commit locally** on `develop` or the current `feature/*` branch (never commit straight to `main`).
2. **Protected branches block direct push** — publish via feature branch:
   ```bash
   git push origin HEAD:feat/your-branch-name
   # or, if already on feat/*:
   git push -u origin feat/your-branch-name
   ```
3. **Open or update PR** into `develop`:
   ```bash
   gh pr create --base develop --head feat/your-branch-name \
     --title "..." --body "..."   # fill PR template checklist
   ```
4. **Wait for CI** — required checks must pass before merge:
   - `Kernel build + unit tests`
   - `Flutter shell analyze + test`
5. **Do not merge to `main`** unless the user explicitly requests a production release PR (`develop` → `main`).

Only create git commits when the user asks (repo rule). When they do, follow this flow.

---

## CI (integration gate)

**Workflow:** `.github/workflows/ci.yml`

| Job | Required? | Command locally |
|-----|-----------|-----------------|
| Kernel build + unit tests | ✅ | `bash tests/run_all.sh` |
| Flutter shell analyze + test | ✅ | `cd apps/oscortex_app && flutter analyze && flutter test` |
| rustfmt + clippy (advisory) | ⚠️ reports only | `cargo fmt --all --check` |

Run the required checks locally before opening a PR when kernel or shell changed.

---

## Production release

**Workflow:** `.github/workflows/release.yml` · triggers on **push/merge to `main`**

- Version: auto patch-bump from latest `v*.*.*` tag (started at `v0.0.1`).
- ISO: `cargo xtask iso` → `oscortex-vX.Y.Z.iso` (kernel-only Limine ISO).
- Full Flutter ISO: `scripts/build-iso.sh` — local/dev only until engine artifact is CI-ready; **do not add a second ISO recipe**.

**To ship develop → prod:**

```bash
gh pr create --base main --head develop --title "Release: ..." --body "..."
# merge after CI + review → release.yml runs automatically
```

Manual re-release: Actions → Release → Run workflow.

---

## PR compliance (fill every PR)

Use [.github/pull_request_template.md](../../../.github/pull_request_template.md):

- One canonical path — delete old code on pivot
- Hardware in kernel only; Flutter via syscalls
- Reuse existing modules; `engine_patch.py` only for engine patches
- No `landing/` changes
- Verification: `tests/run_all.sh`, fmt/analyze as applicable

CODEOWNERS (`.github/CODEOWNERS`) — required review on sensitive paths.

---

## Hotfix (production emergency)

```
fix/*  ← branch from main
  │  PR → main  (releases immediately on merge)
  ▼
back-merge fix into develop  (don't lose the fix)
```

---

## Common agent mistakes (reject)

| Mistake | Correct action |
|---------|----------------|
| Work on `main` | `git checkout develop` |
| `git push origin develop` | Push `feat/*`, open PR |
| Commit without user ask | Ask first (repo rule) |
| Merge to `main` for "finishing work" | Merge to `develop`; release PR is separate |
| Add second ISO build path in CI | Extend `cargo xtask iso` or `release.yml` only |
| Skip PR template / CI | Fill checklist; wait for green checks |

---

## Key paths

| Path | Role |
|------|------|
| `docs/ci-cd.txt` | Canonical CI/CD doc |
| `.github/workflows/ci.yml` | Integration gate |
| `.github/workflows/release.yml` | Production release + ISO |
| `scripts/setup-github.sh` | One-time branch protection bootstrap |
| `tests/run_all.sh` | Local + CI test entry |
| `tools/xtask/src/main.rs` | `cargo xtask iso` (released ISO) |

---

## Quick commands

```bash
# Where am I?
git branch --show-current
git status

# Start feature
git fetch origin && git checkout develop && git pull origin develop
git checkout -b feat/my-change

# Verify before PR
bash tests/run_all.sh
cd apps/oscortex_app && flutter analyze && flutter test

# Publish (develop is protected)
git push -u origin feat/my-change
gh pr create --base develop --head feat/my-change

# Check PR CI
gh pr checks

# Production release (explicit user request only)
gh pr create --base main --head develop --title "Release: ..."
```
