# OSCortex — What This Is

## One sentence

**OSCortex is an AI-first operating system** where a Rust kernel owns hardware and policy, and **Flutter** owns everything on screen — shell, window management, and apps — with **one process per app** so crashes stay isolated.

---

## The stack (top to bottom)

```
┌─────────────────────────────────────────────────────────┐
│  USER APPS (Flutter)     one process each · .osx bundles │
│  SYSTEM SHELL (Flutter)  launcher / desktop · one process│
├─────────────────────────────────────────────────────────┤
│  INIT (PID 1)            spawns shell, reaps children    │
│                          (not the widget tree)           │
├─────────────────────────────────────────────────────────┤
│  KERNEL (Rust, no_std)   compositor · WM · syscalls      │
│                          drivers · scheduler · Cortex    │
├─────────────────────────────────────────────────────────┤
│  HARDWARE                x86_64 first (QEMU / bare metal)│
└─────────────────────────────────────────────────────────┘
```

**Boot path today:** Limine → kernel → init → spawn `oscortex-host` (Flutter embedder) → shell paints pixels.

---

## What you’re actually building

| Piece | What it is | Status |
|-------|------------|--------|
| **Kernel** | Bare-metal OS core: memory, processes, syscalls, compositor, drivers (virtio, NVMe, PS/2, USB/XHCI, net, etc.) | Active — x86_64 bring-up |
| **Compositor** | CPU blit path: Flutter → `gpu_submit_strided` → surface → framebuffer | Working baseline |
| **Flutter embedder** | `oscortex-host` — binds Flutter engine to kernel syscalls (surfaces, vsync, input) | Active |
| **System shell** | `apps/oscortex_app` — the desktop UI (currently slim `dart:ui` for QEMU speed) | Active |
| **Engine patches** | `tools/flutter-engine/engine_patch.py` — patches `libflutter_engine.so` for OSCortex | Active (P9 cave / diagnostics) |
| **App model** | `.osx` bundles, install/launch via syscalls — **not** baked into ISO at dev time | Designed, partial |
| **Cortex** | Kernel AI layer: PID-0 syscalls, healing, driver gen, anomaly context | In progress |
| **CDP drivers** | Hot-loadable WASM drivers (portable across CPU arches) | Framework in place |

---

## Core design rules (non‑negotiable)

1. **One render path** — every pixel goes Flutter → compositor → screen. No parallel demo UIs or kernel splash screens left in tree.
2. **Process isolation** — shell, init, and each app are separate processes. An app crash must not kill the shell.
3. **Hardware stays in the kernel** — Flutter never touches MMIO/PCI; only syscalls.
4. **Runtime app install** — apps ship as `.osx`, installed on a running system — not “reflash ISO for every app change.”
5. **One canonical path** — when you pivot, delete the old code; no dual stacks.

---

## Key artifacts

| Artifact | Meaning |
|----------|---------|
| `oscortex.iso` | Bootable image (kernel + Limine). CI release builds this via `cargo xtask iso`. |
| Full dev ISO | `scripts/build-iso.sh` — kernel + init + engine + shell AOT/assets (local Flutter SDK + patched engine). |
| `oscortex-host` | Flutter host binary — runs shell or a user app. |
| `libflutter_engine.so` | Patched Flutter engine (large binary, gitignored). |
| `.osx` | Installable app bundle format. |

---

## Repo layout (mental map)

```
kernel/                  Rust kernel (compositor, drivers, syscalls, Cortex)
tools/flutter-embedder/  oscortex-host — Flutter ↔ kernel glue
tools/flutter-engine/    engine_patch.py + libflutter_engine.so
apps/oscortex_app/       system shell Dart source
userspace/init/          PID 1 supervisor
scripts/build-iso.sh     full Flutter ISO (dev machine)
tools/xtask/             cargo xtask build | iso | test | run
tests/                   kernel unit tests + QEMU integration
docs/arch.txt            product architecture (canonical)
docs/hardware.txt        drivers & display model
docs/ci-cd.txt             branching & release flow
```

---

## Git / shipping model

```
feature/*  →  PR  →  develop   (integration, default branch)
develop    →  release PR  →  main   (production)
main merge →  auto GitHub Release v0.0.x + oscortex.iso
```

You work on **feature branches**, merge to **`develop`** after CI + review, ship to **`main`** when ready for a versioned release.

---

## Where you are right now

- **v0.0.1** already released on `main` (kernel ISO).
- **`develop`** has merged work: Cortex PID-0 syscalls, embedder vsync fixes, dart:ui shell slimming, CI/CD skill, P9 engine patch diagnostics.
- **Next milestone direction:** reliable Flutter shell paint in QEMU → persistent `.osx` install → richer shell UI (Material/framework again when JIT/AOT path is fast enough).

---

## What it is *not*

- Not a traditional Linux distro with X11/Wayland.
- Not “Flutter as a single monolithic userspace blob in PID 1.”
- Not requiring a GPU vendor stack for first paint (CPU raster + compositor blit is the baseline).
- Not `landing/` (marketing site — out of scope for agents unless you ask).

---

**TL;DR:** You’re building a **custom OS kernel** with a **Flutter-native userspace** — one compositor, one shell process, isolated app processes, syscalls as the only hardware API, and a path toward **AI-assisted driver healing (Cortex)** on top. The repo is the kernel + embedder + shell + build/release tooling to boot and iterate in QEMU, then ship versioned ISOs from `main`.
