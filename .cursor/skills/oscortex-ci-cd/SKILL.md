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

**All code starts on `feature/*`; merges to `develop` only after green CI + human PR review (light gate); ships to `main` only after a separate release PR + stricter human review (heavy gate) — then auto `vX.Y.Z` + ISO.**

---

## Three-tier promotion (memorize)

```
┌─────────────────────────────────────────────────────────────────┐
│  TIER 1 — WORK        feature/*  fix/*                          │
│  Where agents & devs edit code. Push freely to remote feat/*.   │
└───────────────────────────────┬─────────────────────────────────┘
                                │  PR → develop
                                │  ✅ CI green (required)
                                │  ✅ Human clicks Merge (light review)
                                ▼
┌─────────────────────────────────────────────────────────────────┐
│  TIER 2 — INTEGRATE   develop  (default branch)                 │
│  Integration / next release. NEVER push directly.               │
│  Lighter protection: CI + PR + any write-access reviewer.       │
└───────────────────────────────┬─────────────────────────────────┘
                                │  Release PR → main  (explicit only)
                                │  ✅ CI green (required)
                                │  ✅ CODEOWNERS + human release review (strict)
                                ▼
┌─────────────────────────────────────────────────────────────────┐
│  TIER 3 — PRODUCTION  main                                      │
│  Shipped code only. NEVER push directly. NEVER agent-merge.     │
│  Merge triggers release.yml → vX.Y.Z + oscortex ISO.            │
└─────────────────────────────────────────────────────────────────┘
```

| Tier | Branch | Who edits | How code arrives | Merge who | Gate weight |
|------|--------|-----------|------------------|-----------|-------------|
| 1 | `feature/*`, `fix/*` | Agents + devs | `git push origin feat/...` | — | None (open) |
| 2 | `develop` | Nobody direct | PR from `feature/*` | **Human** on GitHub | **Light** — CI + 1 review |
| 3 | `main` | Nobody direct | Release PR from `develop` | **Human** on GitHub | **Strict** — CI + CODEOWNERS review |

**Agents never merge PRs and never push to `develop` or `main`** unless the user explicitly asks for a specific merge/push command.

---

## Tier 1 — Feature branch workflow (default for all work)

### Start

```bash
git fetch origin
git checkout develop && git pull origin develop   # sync integration base
git checkout -b feat/short-description            # ALWAYS branch for new work
```

Work and commit on `feat/*` (or locally on `develop`, then publish — see below).

### Publish (never `git push origin develop`)

```bash
git push -u origin feat/short-description
gh pr create --base develop --head feat/short-description \
  --title "..." --body "..."    # fill PR template checklist
```

Direct push to `develop` **always fails** (`GH006: Protected branch update failed`). That is correct.

### Before asking human to merge

Run locally (same as required CI):

```bash
bash tests/run_all.sh
cd apps/oscortex_app && flutter analyze && flutter test
gh pr checks    # both required jobs must pass
```

Then **stop**. Tell the user: PR is ready for their manual merge into `develop`.

---

## Tier 2 — develop (light gate)

**Purpose:** Integration branch — always green, holds the next release.

**Protection (light):**
- PR required — no direct pushes
- Required CI: `Kernel build + unit tests`, `Flutter shell analyze + test`
- 1 approving review (any reviewer with write access)
- CODEOWNERS **not** required on develop PRs (faster iteration)
- Linear history, branch must be up to date

**Human merge only:** After CI is green and reviewer approves, a **person** merges on GitHub. Agents do not click Merge.

**After merge:** delete or abandon the feature branch; pull `develop` locally:

```bash
git checkout develop && git pull origin develop
```

---

## Tier 3 — main / production (strict gate)

**Purpose:** Exactly what has shipped. Every merge auto-releases.

**Protection (strict):**
- PR required — release PRs only (`develop` → `main`)
- Same required CI checks must pass on the release PR
- **CODEOWNERS review required** (`.github/CODEOWNERS`)
- 1+ approving review — treat as a release sign-off, not a rubber stamp
- Linear history, no force-push

**Human merge only:** Only when the user explicitly requests a production release:

```bash
gh pr create --base main --head develop \
  --title "Release: v0.0.x" --body "Release notes..."
# Human reviews + merges → release.yml → vX.Y.Z + oscortex-vX.Y.Z.iso
```

Agents **must not** open or merge release PRs unless the user explicitly asks.

**On merge to `main`:** `release.yml` patch-bumps tag, builds `cargo xtask iso`, publishes GitHub Release.

---

## CI checks (both tiers 2 and 3)

**Workflow:** `.github/workflows/ci.yml`

| Job | Required for merge? | Local command |
|-----|---------------------|---------------|
| Kernel build + unit tests | ✅ | `bash tests/run_all.sh` |
| Flutter shell analyze + test | ✅ | `cd apps/oscortex_app && flutter analyze && flutter test` |
| rustfmt + clippy (advisory) | ⚠️ reports only | `cargo fmt --all --check` |

---

## Agent rules (non-negotiable)

1. **Always work on `feature/*`** (branch off `develop`) for new changes.
2. **Never `git push origin develop`** or **`git push origin main`**.
3. **Never merge PRs** — prepare PR, verify CI, hand off to human.
4. **Never open `develop` → `main` release PR** unless user explicitly requests production ship.
5. **Commit only when user asks** (repo rule); when they do, publish via `feat/*` + PR to `develop`.
6. Fill [.github/pull_request_template.md](../../../.github/pull_request_template.md) on every PR.

---

## PR compliance checklist

- One canonical path — delete old code on pivot
- Hardware in kernel only; Flutter via syscalls
- Reuse existing modules; `engine_patch.py` only for engine patches
- No `landing/` changes
- Verification: `tests/run_all.sh`, analyze/test as applicable

---

## Hotfix (production emergency)

```
fix/*  ← branch from main
  │  PR → main  (strict gate + human merge → immediate release)
  ▼
back-merge into develop  (PR, so fix is not lost)
```

---

## Common mistakes

| Mistake | Why it fails | Correct action |
|---------|--------------|----------------|
| `git push origin develop` | Protected branch | `git push origin feat/...` + PR |
| Agent merges PR | Policy — human gate | Notify user PR is ready |
| Work on `main` | Production only | `git checkout develop` → `feat/*` |
| Skip CI before merge | Branch protection blocks | Fix failures, re-push feat branch |
| Release without user ask | Prod is deliberate | Wait for explicit release request |

---

## Key paths

| Path | Role |
|------|------|
| `docs/ci-cd.txt` | Canonical CI/CD doc |
| `.github/workflows/ci.yml` | Integration gate (develop + main PRs) |
| `.github/workflows/release.yml` | Production release + ISO (main only) |
| `scripts/setup-github.sh` | Branch protection (light develop / strict main) |
| `tests/run_all.sh` | Local + CI test entry |
| `tools/xtask/src/main.rs` | `cargo xtask iso` (released ISO) |

---

## Quick commands

```bash
# Start work (always feature branch)
git fetch origin && git checkout develop && git pull origin develop
git checkout -b feat/my-change

# Publish
git push -u origin feat/my-change
gh pr create --base develop --head feat/my-change

# Verify before human merge
bash tests/run_all.sh
cd apps/oscortex_app && flutter analyze && flutter test
gh pr checks

# After human merged your PR — sync local develop
git checkout develop && git pull origin develop

# Production (human + explicit request only)
gh pr create --base main --head develop --title "Release: ..."
```
