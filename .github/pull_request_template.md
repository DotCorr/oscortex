<!--
OSCortex PRs target `develop`. Only release PRs (develop -> main) ship an ISO.
Read docs/arch.txt + docs/hardware.txt before structural changes.
Do not append tool/vendor footers (e.g. "Made with Cursor") — project content only.
-->

## What & why

<!-- One or two sentences. Link the issue if there is one. -->

## Type

- [ ] Feature
- [ ] Fix
- [ ] Refactor / cleanup
- [ ] Docs
- [ ] CI / tooling
- [ ] Release (develop -> main)

## Architecture compliance

- [ ] One canonical path — no dual boot/render stacks, deleted old code on pivot
- [ ] Hardware access stays in kernel; Flutter touches syscalls only
- [ ] Reused existing modules (no duplicated patch/embedder/syscall logic)
- [ ] Engine changes go through `tools/flutter-engine/engine_patch.py` only
- [ ] Did not touch `landing/`

## Verification

- [ ] `bash tests/run_all.sh` passes (unit + kernel cross-build)
- [ ] `cargo fmt --all -- --check` clean
- [ ] `flutter analyze` clean (if Dart/shell changed)
- [ ] QEMU smoke check if drivers/boot changed (`cargo xtask run`)
