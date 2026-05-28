# AI-Generated Code Smell Catalog (OSCortex)

Use during Phase 2 manual audit.

## OSCortex-specific smells

| Smell | Example | Fix |
|-------|---------|-----|
| Scratch sprawl | `scratch/patch_engine.py` | Merge into `engine_patch.py`, delete scratch |
| Pivot shadow | `main_minimal.dart`, `userspace/init/` | Delete; one shell entry |
| Boot demo layer | kernel grid/banner/moving box before Flutter | Delete; compositor waits for real surface |
| Same-PID app launch | `app_launch` → isolate in shell PID | Target: spawn app host process (see arch.txt) |
| ISO app catalog | `apps/*` loop baking user apps into image | Shell only in ISO; runtime `app_install` |
| Diagnostic permanence | `[embedder]` spam, `present_callback` p0 logs | Remove after fix verified once |

## Generic smells (still apply)

See conversation skill source for: Utility Sprawl, Copy-Paste Validation, Parallel Interfaces, Magic Strings, God Utility File, Dead Feature Shadows, Error Handling Chaos.

**Severity:** Copy-paste tooling and dual boot paths are **Critical** on an OS — they cause wrong-runtime behavior, not just style issues.
