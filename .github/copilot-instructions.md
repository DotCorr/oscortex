# OSCortex Copilot Instructions

This repository uses project-specific guidance in two places:

- `.agents/rules/oscortex-core.md`
- `.agents/skills/*/SKILL.md`

When helping in this repo:

1. Align with target architecture in `docs/arch.txt` and hardware policy in `docs/hardware.txt`.
2. Keep one canonical implementation path; do not leave parallel legacy paths after pivots.
3. Reuse existing modules; avoid duplicate engine patch, syscall, or embedder logic.
4. Exclude `landing/` unless the user explicitly asks to modify it.
5. For kernel/driver changes, run `tests/run_all.sh` (and `--qemu` when relevant).
6. For CI/release and branch behavior, follow `docs/ci-cd.txt` and `.agents/skills/oscortex-ci-cd/SKILL.md`.

If a task involves architecture, hardware, CI/CD, or cleanup, consult the matching skill in `.agents/skills/` before making structural changes.

