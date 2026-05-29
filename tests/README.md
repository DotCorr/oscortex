# OSCortex Tests

Kernel driver and integration tests live here — **not** in `landing/`.

## Layout

| Path | Kind | Runs on |
|------|------|---------|
| `tests/kernel/` | Host unit tests (pure driver logic) | Any host (`cargo test`) |
| `tests/integration/qemu_drivers.sh` | Boot ISO in QEMU, grep serial for driver markers | x86_64 + QEMU |
| `tests/run_all.sh` | Runner for unit (+ optional `--qemu`) | See above |

## Quick start

```bash
# Fast — no QEMU (run after any kernel driver change)
tests/run_all.sh

# Full — build ISO + boot QEMU driver smoke test
tests/run_all.sh --qemu
```

## What unit tests cover

Sources under test are **`kernel/src/drivers/common/`** — included directly so tests stay synced with the kernel:

- `vring.rs` — virtio split-ring layout
- `pci_bar.rs` — BAR decode, PCI class match (XHCI, NVMe)
- `xhci_caps.rs` — xHCI CAP/HCSPARAMS1 parse
- `nvme_regs.rs` — doorbell stride/offset
- `virtio_net_frame.rs` — frame length / TX validation
- `driver_manifest.rs` — docs + `platform.rs` register_native compliance

## What integration tests cover

`tests/integration/qemu_drivers.sh` expects serial markers:

- `[virtio-blk] ready`
- `[virtio-net] ready` + `self-test TX completion OK`
- `[NVMe]`
- `[Input] PS/2 enabled`

USB XHCI is probed when `-device qemu-xhci` is present (`scripts/run-qemu.sh`).

## Agent skill

See `.cursor/skills/oscortex-kernel-tests/SKILL.md` — update tests when adding drivers or changing `drivers/common/`.
