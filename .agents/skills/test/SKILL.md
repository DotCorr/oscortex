---
name: test
description: >
  Internal smoke-test skill for validating that workspace skill discovery works.
  Use only when explicitly asked to test skill loading, frontmatter parsing, or
  agent customization plumbing.
---

# Test Skill (Plumbing Validation)

This skill exists to verify customization wiring and should not be selected for
normal product development tasks.

## Use this skill when

- The user explicitly asks to test whether workspace skills are discovered.
- You need to validate YAML frontmatter parsing for skills.
- You are debugging agent customization file loading behavior.

## Do not use this skill when

- Implementing OSCortex features.
- Refactoring architecture, drivers, CI/CD, or test harnesses.

## Validation checklist

1. Confirm the skill appears in the available skills list.
2. Confirm the name and description are readable by the agent runtime.
3. Confirm normal feature prompts still prefer domain skills over this one.

