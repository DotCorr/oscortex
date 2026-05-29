---
name: oscortex-kernel-tests
description: >
  OSCortex kernel driver tests — unit tests in tests/kernel/, QEMU integration in
  tests/integration/, run via tests/run_all.sh or cargo xtask test. Use when adding
  or changing kernel drivers, arch shims, platform init, or docs/hardware compliance.
---

# OSCortex Kernel Tests

**Read first:** [tests/README.md](../../../tests/README.md) · [docs/hardware.txt](../../../docs/hardware.txt)

**Related skills:** [oscortex-hardware](../oscortex-hardware/SKILL.md) · [oscortex-architecture](../oscortex-architecture/SKILL.md) · [codebase-audit-cleanup](../codebase-audit-cleanup/SKILL.md)

---

## Run commands

```bash
# Fast — host unit tests only (any machine)
tests/run_all.sh
cargo xtask test

# Full — unit + QEMU driver smoke (x86_64, builds ISO)
tests/run_all.sh --qemu
cargo xtask test-qemu
```

---

## Layout

| Path | Purpose |
|------|---------|
| `tests/kernel/` | Host `cargo test` — pure logic from `kernel/src/drivers/common/` |
| `tests/integration/qemu_drivers.sh` | Boot ISO, grep serial for driver ready markers |
| `tests/run_all.sh` | Unified runner |
| `kernel/src/drivers/common/` | **Testable pure helpers** — keep driver math here |

---

## When you change kernel drivers

1. **Pure logic** (BAR decode, vring layout, frame lengths, xHCI CAP parse) → put in `drivers/common/`, add/update unit test in `tests/kernel/unit/`.
2. **New native driver** → register in `drivers/platform.rs`, list in `docs/hardware.txt`, extend `tests/kernel/unit/driver_manifest.rs` if needed.
3. **QEMU-visible init** → ensure serial log line exists; add marker to `tests/integration/qemu_drivers.sh` if bring-up critical.
4. Run `tests/run_all.sh` before finishing driver work.

---

## Unit test pattern

Tests **include** kernel sources directly (no full no_std host build):

```rust
#[path = "../../../kernel/src/drivers/common/vring.rs"]
mod vring;
```

Do **not** duplicate layout math in the test crate — change `drivers/common/` and tests follow.

---

## Integration markers (current)

Expected in QEMU serial output:

- `[virtio-blk] ready`
- `[virtio-net] ready` + `[virtio-net] self-test TX completion OK`
- `[NVMe]`
- `[Input] PS/2 enabled`
- `[USB-HID] WM route OK` (HID parse → WM self-test; runs on every x86 boot)
- `[USB] XHCI ready` or `[USB] N XHCI controller(s) mapped and reset`
- `[USB] XHCI runtime started` (command/event rings programmed; enumeration polled on vsync)
- `[smoltcp] TCP/IP stack initialised`
- `[net] DHCP self-test OK` **or** `timeout (static IP fallback OK)`
- `[net] TCP self-test connect OK` **or** `connect skipped`

Also run on every CI/local check:

- `tests/integration/kernel_build.sh` — `x86_64-unknown-none` cross-build on host (works on Apple Silicon Macs)

## Not covered (yet)

- Live USB keyboard reports (`[USB-HID] live key` from `-device usb-kbd`) — xHCI runtime rings up but full EP0/HID enumeration still WIP
- Flutter paint / scheduler / cond-wait (separate agent)
- Physical bare-metal hardware (QEMU profile only; bare-metal policy covered by unit tests)

Document gaps in `docs/hardware.txt` — do not claim test coverage that does not exist.
