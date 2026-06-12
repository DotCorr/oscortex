# OSCortex — What This Is

## One sentence

**OSCortex is an AI-first operating system** where a Rust kernel owns hardware and policy, and **Flutter** owns everything on screen — shell, window management, and apps — with **one process per app** so crashes stay isolated.

**Built for connected-screen devices** — kiosks, EV chargers, head units, embedded panels — that need a full-screen UI which boots in seconds, can't be taken down by one app crashing, and updates across a whole fleet over the air. General-purpose underneath; **one source tree runs on x86_64 and aarch64 (Raspberry Pi).**

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
│  HARDWARE                x86_64 + aarch64 (QEMU / Pi / metal)│
└─────────────────────────────────────────────────────────┘
```

**Boot path today (x86_64):** Limine → kernel → init → spawn `oscortex-host` (Flutter embedder) → shell paints pixels.
**Boot path (aarch64):** `qemu-system-aarch64 -M virt -kernel` → EL1 platform → MMU → GICv2 + timer → EL0 userspace process servicing syscalls. *(Same source tree; ARM bring-up boots to userspace with RAMFB display + timer preemption.)*

---

## What you’re actually building

| Piece | What it is | Status |
|-------|------------|--------|
| **Kernel** | Bare-metal OS core: memory, processes, syscalls, compositor, drivers (virtio, NVMe, PS/2, USB/XHCI, net, etc.) | Working — **x86_64 + aarch64, both boot to userspace** |
| **Compositor** | CPU blit path: Flutter → `gpu_submit_strided` → surface → framebuffer | Working |
| **Flutter embedder** | `oscortex-host` — binds Flutter engine to kernel syscalls (surfaces, vsync, input) | Working — surfaces, vsync, **mouse/hover/click**, platform channels |
| **System shell** | `apps/oscortex_app` — the desktop UI | Working — **full Material shell renders** (title, app list, install button, icons) |
| **App model** | `.osx` bundles, install/launch via syscalls — **not** baked into ISO at dev time | Working — **install + launch proven** (one process per app); persistence next |
| **Engine patches** | `tools/flutter-engine/engine_patch.py` — patches `libflutter_engine.so` for OSCortex; **AOT execution confirmed** | Active |
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

**Delivered (running in QEMU):**
- **Full Flutter Material shell renders end-to-end** — title, "installed apps" list, install button, icons; render is reliable after the syscall-entry GPR-capture fix.
- **Mouse input working** — hover feedback on cards, click → app launch; **platform channels live** (the "install demo" flow round-trips through the shell channel and the app list refreshes).
- **Multi-app launch** — tapping a tile launches the app, loads its **own** AOT blob, and renders its **own** UI; one-process-per-app isolation proven.
- **AOT execution confirmed** — the engine runs AOT-compiled Dart (no JIT-warmup dependence).
- **aarch64 port boots to userspace** — on `qemu-system-aarch64 -M virt`: EL1 → MMU → GICv2 + generic timer → EL0 process servicing syscalls, with RAMFB display + timer preemption. **One source tree, two architectures.**
- **v0.0.1** released on `main` (kernel ISO); `develop` carries the integrated work above.

**In flight:**
- Stability hardening on the input/app-launch path (register-corruption edge cases) — actively being worked.
- Persistent `.osx` install (deliver app bundles off-ISO via disk/modules, not embedded).
- **Real-hardware bring-up** — moving off QEMU onto a physical board (Raspberry Pi 4/5 / x86 mini-PC) with a real panel and real input.

**Next milestone direction:** a single connected-screen vertical (e.g. a kiosk / charger UI) running on **physical hardware**, stable for an extended session — the first real-device proof.

---

## What it is *not*

- Not a traditional Linux distro with X11/Wayland.
- Not “Flutter as a single monolithic userspace blob in PID 1.”
- Not requiring a GPU vendor stack for first paint (CPU raster + compositor blit is the baseline).
- Not `landing/` (marketing site — out of scope for agents unless you ask).

---

**TL;DR:** A **custom OS kernel** with a **Flutter-native userspace** — one compositor, one shell process, isolated app processes, syscalls as the only hardware API, **AOT Dart**, and a path toward **AI-assisted driver healing (Cortex)**. Today it boots from one source tree on **x86_64 and aarch64**, renders the **full Material shell**, takes **mouse input**, and **launches isolated apps** in QEMU. The wedge it ships into first is **connected-screen devices** (kiosks, EV chargers, head units, embedded panels) that need a fast-booting, crash-isolated, OTA-updatable full-screen UI — next stop, **real hardware.**
