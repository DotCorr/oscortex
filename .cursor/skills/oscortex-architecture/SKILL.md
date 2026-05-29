---
name: oscortex-architecture
description: >
  Canonical OSCortex OS architecture — kernel vs Flutter userspace, process isolation,
  shell vs apps, runtime install model. Use when designing features, refactoring boot/app
  launch, writing docs, or any question about how OSCortex should work (not legacy hacks).
---

# OSCortex Architecture

**Read first:** [docs/arch.txt](../../../docs/arch.txt) · [docs/hardware.txt](../../../docs/hardware.txt)

**Out of scope:** `landing/` — never modify unless explicitly requested.

---

## One-sentence model

**Kernel owns hardware and policy. Flutter owns everything above the kernel — window management, shell, and applications — with one hard rule: each runnable unit that can crash is its own process.**

---

## Layers

```
USER APPS (Flutter)     — one process per app, own engine instance, own surface(s)
SYSTEM SHELL / WM (Flutter) — one system process; launcher, desktop, chrome, gen UI
INIT / SUPERVISOR       — PID 1; boot, spawn/stop/restart, reap zombies; NOT the widget tree
KERNEL                  — scheduler, memory, IPC, drivers, compositor, focus, syscalls
```

---

## Non-negotiable rules

1. **Process isolation** — user apps never share an address space with the shell or each other.
2. **Failure containment** — one app crash must not take down shell, init, or other apps.
3. **One render path** — every pixel via Flutter engine → surface → kernel compositor. No parallel shells or demo/fallback render paths left in tree.
4. **Runtime install** — apps ship as built AOT bundles (`.osx`); installed on a running OS via syscalls. **Not** “add to `apps/`, rebuild ISO, reflash.”
5. **System apps** — shell and core system UI are uninstallable; still **separate processes**, not merged into PID 1.
6. **Pivot = delete** — when architecture or approach changes, remove the old path entirely. No dual boot stacks, no “legacy for now.”

---

## App lifecycle (product)

| Step | Where |
|------|--------|
| Dev build | Flutter project → AOT artifact (pack tool → `.osx`) |
| Install | `app_install` syscall → persistent storage |
| Launch | spawn **new app host process** (embedder + engine + that app's AOT) |
| Uninstall | `app_uninstall` (reject for system apps) |
| List | `app_list` / platform channel for shell store UI |

---

## Kernel vs Flutter boundary

| Kernel | Flutter userspace |
|--------|-------------------|
| Compositor, surface table, z-order | Widgets, layout, window chrome (Dart) |
| Focus, hit-testing, input routing | Pointer/key handling inside engine |
| `surface_create` / `present` / flip | `present_callback` → pixels |
| Process spawn, mmap, capabilities | App logic, platform channels |
| Drivers, compositor blit, input HW | Never maps MMIO / PCI from userspace |

**Hardware detail:** [docs/hardware.txt](../../../docs/hardware.txt) · skill `oscortex-hardware`

---

## Hardware (summary)

- **One render path:** Flutter → `gpu_submit_strided` → compositor → framebuffer (CPU blit today).
- **`gpu_submit` ≠ GPU vendor driver** — pixel copy into surfaces; real GPU is a future compositor backend.
- **Each device class** (audio, touchpad, storage, …) needs its own kernel driver; APIs are shared via syscalls.
- **Multi-arch:** port CPU in `kernel/src/arch/`; drivers per platform; CDP WASM portable across ISAs.

---

## Canonical paths

| Path | Role |
|------|------|
| `kernel/` | Kernel — compositor, WM events, syscalls, process model |
| `tools/flutter-embedder/` | Flutter host binary (`oscortex-host`) |
| `tools/flutter-engine/engine_patch.py` | **Only** engine patch entry point |
| `apps/oscortex_app/` | System shell source (not an app manager holding all apps) |
| `scripts/build-iso.sh` | ISO build — ships kernel, init, engine, shell; not user app catalog |
| `docs/arch.txt` | Canonical architecture document |
| `docs/hardware.txt` | Drivers, display, device classes, CDP |
| `.cursor/skills/oscortex-hardware/SKILL.md` | Agent guide for driver/hardware work |
| `.cursor/skills/oscortex-ci-cd/SKILL.md` | Branching, PRs, CI, release to main |

---

## When implementing

- Align new code with **target architecture** in `docs/arch.txt`, not temporary bring-up shortcuts.
- Do not add isolate-in-PID-1 launches for user apps.
- Do not add kernel boot demos, grid splashes, synthetic cursors, or compositor placeholders.
- Extend existing modules; do not duplicate embedder/patch/syscall logic in new files.
