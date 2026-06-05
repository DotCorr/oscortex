---
applyTo: "**"
---

Read these first before structural work:

- `docs/arch.txt`
- `docs/hardware.txt`
- `docs/ci-cd.txt`
- `.agents/rules/oscortex-core.md`
- `.agents/skills/oscortex-architecture/SKILL.md`
- `.agents/skills/oscortex-hardware/SKILL.md`
- `.agents/skills/oscortex-kernel-tests/SKILL.md`
- `.agents/skills/oscortex-ci-cd/SKILL.md`
- `.agents/skills/codebase-audit-cleanup/SKILL.md`

Repository rules:

- Keep one canonical path only; delete old paths after a pivot.
- Reuse existing modules and tools; do not duplicate patch/embedder/syscall flows.
- Use `tools/flutter-engine/engine_patch.py` as the only engine patch entry point.
- Do not modify `landing/` unless explicitly requested.

