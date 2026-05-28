---
name: codebase-audit-cleanup
description: >
  Audit and clean OSCortex for redundancy, AI slop, dead pivots, and missing reusability.
  Trigger on: cleanup, audit, duplicate code, technical debt, refactor, bloat, pivot,
  remove old code, one path, code reusability, or before large features. Excludes landing/.
---

# Codebase Audit & Cleanup (OSCortex)

Systematically find duplicate logic, orphaned scripts, dual code paths, and boot/render junk — then delete or consolidate. **An OS cannot carry parallel implementations.**

**Also read:** [oscortex-architecture](../oscortex-architecture/SKILL.md) · [docs/arch.txt](../../../docs/arch.txt)

**Exclude from all scans and edits:** `landing/`

---

## Core philosophy

AI generates code in isolation. On OSCortex this becomes:

- **Duplicate patch scripts** — `scratch/patch_*.py` next to `engine_patch.py`
- **Pivoted apps** — `main_minimal.dart`, `.bak`, alternate shells
- **Dual boot/render paths** — kernel demos + Flutter; FB_BYPASS hacks; moving test boxes
- **God files** — single modules doing entire syscall domains
- **Orphan harnesses** — one-off debug scripts never wired to CI

**Rule:** One canonical way per concern. On pivot, **delete the old way** — do not comment it out “for reference.”

---

## OSCortex canonical locations (do not duplicate)

| Concern | Canonical location |
|---------|---------------------|
| Engine patches | `tools/flutter-engine/engine_patch.py` |
| Patch verification | `harness/verify_engine_patches.py` |
| ISO build | `scripts/build-iso.sh` |
| Embedder | `tools/flutter-embedder/` |
| Architecture | `docs/arch.txt` |
| System shell app | `apps/oscortex_app/` |

If you find logic elsewhere that belongs here → **move or merge**, then **delete** the duplicate.

---

## Phase 1: Automated scan (OSCortex)

Run from repo root. Adapt extensions for Rust/Dart/Python/sh.

```bash
# 1A — largest files (god-file candidates)
find . -type f \( -name "*.rs" -o -name "*.dart" -o -name "*.py" -o -name "*.sh" \) \
  ! -path "./.git/*" ! -path "./target/*" ! -path "./build/*" ! -path "./landing/*" \
  | xargs wc -l 2>/dev/null | sort -rn | head -35

# 1B — debt markers
grep -rn "TODO\|FIXME\|HACK\|XXX\|TEMP\|WORKAROUND" \
  --include="*.rs" --include="*.dart" --include="*.py" --include="*.sh" . \
  | grep -v "./.git" | grep -v "./landing"

# 1C — duplicate / orphan tooling
find scratch -type f 2>/dev/null
find . -name "*.bak" -o -name "*_minimal.*" -o -name "*.bak_*" 2>/dev/null | grep -v landing

# 1D — references to deleted or legacy paths
grep -rn "scratch/\|userspace/init\|main_minimal\|STRICT_ENGINE_MODE\|early_banner\|early_probe\|test.red\|test_red" \
  --include="*.rs" --include="*.dart" --include="*.py" --include="*.sh" . \
  | grep -v "./.git" | grep -v "./landing"

# 1E — Rust unused / warnings
cd kernel && cargo check 2>&1 | grep -E "warning:|unused"

# 1F — duplicate engine patch entry points
grep -rn "va_to_file\|0x196e300\|patch_engine" --include="*.py" . | grep -v engine_patch.py
```

---

## Phase 2: Manual checklist (score 0–3)

### Single path
- [ ] Only one boot userspace stack documented in `docs/arch.txt`?
- [ ] Only one engine patch script?
- [ ] No kernel boot splash/demo surfaces before Flutter presents?
- [ ] No second Dart entry (`main_minimal`, `.bak`) for the same app?

### Reusability
- [ ] New syscall logic in focused modules under `kernel/src/syscall/`, not a new god file?
- [ ] Shared ELF/AOT parsing not copied embedder ↔ kernel?
- [ ] Build scripts call canonical tools, not inline duplicate patch bytes?

### Pivot hygiene
- [ ] Old approach files **deleted**, not deprecated in place?
- [ ] Docs updated when architecture changes (`docs/arch.txt`)?

---

## Phase 3: Audit report template

```markdown
# Codebase Audit Report — OSCortex — [Date]

## Summary
- Files scanned: X
- Critical / High / Medium counts

## Critical (fix before features)
| Issue | Location | Action | Effort |

## Deleted this pass
| Path | Reason |

## Consolidated this pass
| From | To |

## Remaining (prioritized)
...
```

---

## Phase 4: Harness

Before/after refactors:

```bash
python3 harness/verify_engine_patches.py
cd kernel && cargo build --release
bash scripts/build-iso.sh   # when boot/render touched
```

One module per change. Re-run harness after each.

---

## Phase 5: Cleanup order

1. **Dead files** — scratch orphans, `.bak`, pivoted duplicates, stale HANDOFF docs
2. **Boot/render junk** — kernel demos, fallback surfaces, verbose embedder logs
3. **Consolidate tooling** — patches, blob hash, verify scripts
4. **Split god modules** — syscall domains, compositor vs WM
5. **Architecture alignment** — remove paths that violate process isolation model

---

## Prompt template for AI cleanup tasks

```
AUDIT FINDING: [specific finding]

CANONICAL TARGET: [path or pattern from table above]

TASK:
- Consolidate or delete redundant code
- Do not leave dual paths
- Update docs/arch.txt if behavior/arch changed
- Exclude landing/

VERIFY:
python3 harness/verify_engine_patches.py
cargo build --release (if kernel touched)
```

---

## Reference

- Smell catalog and generic scan commands: [references/smell-catalog.md](references/smell-catalog.md)
- Language scan commands: [references/scan-commands.md](references/scan-commands.md)
