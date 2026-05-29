---
name: oscortex-hardware
description: >
  OSCortex hardware and driver model — display/GPU path, device classes, syscalls,
  multi-arch rules, CDP WASM drivers. Use when adding or changing drivers, input,
  audio, storage, net, compositor backends, or any question about hardware support
  vs CPU porting.
---

# OSCortex Hardware & Drivers

**Read first:** [docs/hardware.txt](../../../docs/hardware.txt)  
**Product context:** [docs/arch.txt](../../../docs/arch.txt) · [oscortex-architecture](../oscortex-architecture/SKILL.md)

**Out of scope:** `landing/`

---

## One-sentence model

**Hardware stays below stable syscalls; Flutter never touches devices; each device class needs its own driver, but the userspace API is shared.**

---

## Display / GPU (most common confusion)

- **No traditional GPU driver required** for shell first paint.
- Path: Flutter CPU raster → `present_callback` → `gpu_submit_strided` → compositor CPU blit → framebuffer.
- **`gpu_submit` = pixel copy to surface**, not Mesa/NVIDIA programming.
- Future real GPU = optional compositor backend under **same** surface syscalls.

---

## Layers

```
Flutter  →  syscalls (abi.rs)  →  compositor/WM/policy  →  drivers/  →  arch/
```

---

## What transfers vs what does not

| Transfers everywhere | Per device / per class |
|---------------------|-------------------------|
| Surface + WM syscalls | NVMe, audio, touchpad drivers |
| Flutter render contract | PCI ID-specific quirks |
| CDP WASM on all CPUs | Probe + init for each bus |
| compositor logic | Machine QEMU profile |

---

## Adding hardware (short checklist)

1. Driver in `kernel/src/drivers/` (native) or CDP WASM via `driver_gen.rs`.
2. MMIO/PCI only through `kernel/src/arch/` (`pci`, `port_io`, `mmio`, `cpu`).
3. Expose via **syscalls** or WM events — not userspace MMIO.
4. Update status in `docs/hardware.txt`.
5. Flutter via platform channel or existing WM — no parallel render path.
6. **Run tests** — `tests/run_all.sh` (see oscortex-kernel-tests skill).

---

## Testing (2026-05-27)

| Layer | Command | Covers |
|-------|---------|--------|
| Unit | `tests/run_all.sh` | vring, PCI BAR, xHCI caps, NVMe doorbells, net frames, manifest |
| Integration | `tests/run_all.sh --qemu` | virtio-blk/net, NVMe, PS/2 serial markers |

Skill: `.cursor/skills/oscortex-kernel-tests/SKILL.md`

---

## Canonical paths

| Path | Role |
|------|------|
| `docs/hardware.txt` | Canonical hardware document + compliance table |
| `kernel/src/arch/pci.rs` | PCI facade (all driver probing) |
| `kernel/src/arch/port_io.rs` | Legacy I/O port facade |
| `kernel/src/arch/mmio.rs` | MMIO load/store facade |
| `kernel/src/drivers/platform.rs` | Single driver init entry from main |
| `kernel/src/drivers/registry.rs` | Native + WASM driver registry |
| `kernel/src/embedder/abi.rs` | Stable syscall numbers |
| `kernel/src/drivers/` | Native drivers |
| `kernel/src/compositor/` | Surfaces, gpu_submit, vsync |
| `kernel/src/wm/` | Input routing, focus |
| `kernel/src/cortex/driver_gen.rs` | CDP WASM protocol (runtime AI drivers) |
| `kernel/src/arch/` | CPU porting |

## Compliance (2026-05-27)

| Rule | Status |
|------|--------|
| Syscalls-only userspace | ✅ |
| PCI/port/MMIO via `arch/` | ✅ |
| `drivers/platform.rs` init | ✅ |
| Native drivers in registry at boot | ✅ |
| Compositor canonical render | ✅ (`fb_map` no longer bypasses) |
| CDP WASM runtime load/heal | ✅ sandbox + quarantine; AI gen TBD |
| All device classes on all CPUs | x86 full profile; other arches skip PCI cleanly |

---

## Non-negotiable rules

1. **One render path** — compositor + Flutter only (see architecture skill).
2. **No userspace drivers** — no MMIO in embedder or apps.
3. **Pivot = delete** — no duplicate driver or syscall paths.
4. **Do not block shell bring-up** on GPU vendor stacks or unimplemented device classes.

---

## When implementing

- Extend existing driver modules; do not fork parallel ioctl/syscall domains.
- Align with `docs/hardware.txt` device table — mark ✅ / ⏳ when done.
- Shell paint issues are often **scheduler/compositor**, not missing GPU — verify before adding drivers.
