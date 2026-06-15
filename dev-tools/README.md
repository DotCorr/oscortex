# OSCortex dev-tools

Tooling for **contributors** working on the OSCortex low level (kernel, drivers,
boot, engine bring-up). These are the harnesses and recovery scripts used to
diagnose and verify the boot/render/input stability of both architectures.

> App-developer tooling (compiling `.osx` apps, the cross-arch emulator/preview)
> lives separately — see the `osx` CLI package. This folder is for **OS/kernel
> contributors**, not app developers.

All scripts auto-detect the repo root (via `git rev-parse --show-toplevel`) and
take overridable env vars — no hard-coded paths. Run them from anywhere in the repo.

## Layout

| Path | Purpose |
|---|---|
| `test/freeze-repro.py` | aarch64 `.kernel` boot (HVF) → launch app → sweep → assert `present_callback` keeps advancing (the post-app-launch freeze regression test). Env: `OSCORTEX_KERNEL`. |
| `test/freeze-repro-x86.py` | x86_64 ISO boot (TCG, UEFI/OVMF) → render + launch + no-freeze. Env: `OSCORTEX_ISO`. |
| `test/input-test.py` | Boots with UTM-exact devices (virtio-vga + qemu-xhci + usb-tablet) + injects QMP pointer events → verifies the kernel binds + processes the pointer. |
| `build/restore-arm64.sh` | **Recover the arm64 build** after an x86 build clobbers the shared `initramfs/` (see "Build fragility" below). |
| `build/regen-x86-snapshots.sh` | Regenerate **product-mode** x86 AOT snapshots (shell + apps) from the AOT dills using the matching `gen_snapshot`. |

All scripts take env overrides (e.g. `OSCORTEX_REPO`, `OSCORTEX_KERNEL`, `OSCORTEX_ISO`,
`OSCX_ENGINE_CONTAINER`) and default to repo-relative / `~/OSCortex-run` paths. For an
arm64 **ISO** (UEFI) freeze test, run `freeze-repro.py` against a kernel built into a
Limine ISO, or boot the ISO with the `freeze-repro-x86.py` QEMU pattern adapted to
`-M virt,accel=hvf -bios <edk2-aarch64-code.fd> -cdrom <iso>`.

## Quick start

```sh
# Regression: does the freeze stay fixed on aarch64?
python3 dev-tools/test/freeze-repro.py

# Boot the x86 ISO headless and check it renders + no freeze
OSCORTEX_ISO=/path/to/oscortex-x86_64.iso python3 dev-tools/test/freeze-repro-x86.py
```

## Hard-won knowledge (read before debugging boot/input)

**Two real-hardware boot hangs that QEMU's default devices hid:**
- **Legacy PIT calibration** (x86): TSC/APIC calibration spun forever on the i8254
  PIT/speaker gate, which Apple firmware (and some UEFI) doesn't wire. Fixed by
  CPUID-derived TSC freq + bounded PIT spins (`kernel/src/arch/x86_64/apic.rs`).
  Reproduce the *path* with QEMU `-machine q35,pit=off`.
- **Legacy i8042 PS/2 drain** (x86, UTM): `ps2::init` flushed the i8042 with
  unbounded `while (status & OBF)` loops; an absent controller reads `0xFF` (OBF
  always set) → infinite spin → hang at `cortex::drivers`. Fixed with a 0xFF guard
  + bounded drains (`kernel/src/drivers/ps2.rs`). QEMU's own q35 i8042 clears OBF,
  hiding it headless — only UTM/real-HW reproduce.

**On-screen boot phases:** the boot splash shows `cortex::<phase>` (set by
`bootscreen::set_phase` at each `shared_init_and_run` step). When a boot wedges on
hardware QEMU can't reproduce, the frozen splash names the last stage reached —
your fastest localizer when there's no serial.

**Input device matrix (what the kernel binds):**
- aarch64: **virtio-input** (`virtio-tablet-device`/`virtio-keyboard-device`) on the
  `ramfb` display. USB is gated on the limine-boot ISO (QEMU-11+HVF xHCI
  `assert(isv)` crash). NOTE: adding virtio-input alongside UTM's `virtio-ramfb`
  (virtio-GPU) display breaks the display — use plain `ramfb`.
- x86_64: **USB** via xHCI. Boot keyboard/mouse + **usb-tablet** (absolute, the
  UTM/QEMU default pointer — protocol 0, parsed via the HID report descriptor).
  PS/2 is skipped when no i8042 is present.

**Serial:** the default kernel log level is `Error` (fast boot). Raise to `Info` in
`kernel/src/logger.rs` for deep probe visibility. UTM VMs have no serial port by
default — add one (or use the on-screen boot phases) to debug on-device.

## Build fragility (important)

The aarch64 and x86 builds **share the `initramfs/` directory** and clobber each
other's staged artifacts (the x86 `build-iso.sh` strips `libapp.so`; the arm build
stages its own engine/snapshots). After building one arch, **re-stage before
building the other**, or use `build/restore-arm64.sh` to recover the aarch64 tree.
The arch-specific AOT snapshots are backed up under `apps/*/build/oscortex/`
(`libapp-arm64.so` / the x86 `libapp.so`). A proper fix (separate per-arch staging
dirs) is a tracked cleanup item.
