# OSCortex UTM Bring-up Guide

This guide details how to configure, deploy, and boot **OSCortex** on **UTM** (the GUI frontend for QEMU on macOS) for both emulated `x86_64` and native virtualised `aarch64` architectures. Testing under UTM simulates physical hardware characteristics more closely than standard command-line QEMU.

---

## 1. x86_64 Emulation on M-series/Intel Macs

To test the full graphical shell and persistence on UTM:

1. **Build the ISO**:
   Ensure you have built the latest bootable ISO:
   ```bash
   bash scripts/build-iso.sh
   ```
   This generates `oscortex.iso` in the project root.

2. **Create the VM in UTM**:
   * Open UTM, click **+** (Create a New Virtual Machine).
   * Select **Emulate** (performs instruction translation on M-series Macs).
   * Choose **Other** as the platform.
   * Under **Boot ISO Image**, browse and select `oscortex.iso`.

3. **Configure VM Settings**:
   * **System**:
     * Hardware: Allocate **2048 MB** of RAM and **2 CPU Cores**.
     * Target: **Standard PC (Q35 + ICH9, 2009) (q35)**.
     * CPU: **qemu64** or **host** (Intel). Ensure x2apic is enabled.
   * **QEMU**:
     * Add the following arguments under QEMU settings if you want custom serial redirection:
       `-serial file:/tmp/osc_serial.log`
   * **Drives**:
     * The CD/DVD drive is automatically mounted with `oscortex.iso`.
     * Click **New Drive** to add a data drive for package persistence:
       * Size: **8 MB** (or import `vdisk.img`).
       * Interface: **VirtIO** (matches the `virtio-blk-pci` driver).
     * Click **New Drive** to add an NVMe test drive:
       * Size: **16 MB** (or import `nvme.img`).
       * Interface: **NVMe** (matches the NVMe controller).

4. **Launch the VM**:
   * Save settings and click the **Play** button.
   * The Limine bootloader will hand off execution to the kernel, bringing up the Flutter graphics shell.

---

## 2. AArch64 (ARM64) Native Virtualisation on M-series Macs

To run the ARM64 kernel natively with hypervisor acceleration (hypervisor.framework):

1. **Build the ARM64 Kernel**:
   ```bash
   cargo build --target aarch64-unknown-none -p oscortex-kernel \
       --no-default-features --features arch-aarch64 \
       -Z build-std=core,compiler_builtins,alloc \
       -Z build-std-features=compiler-builtins-mem
   ```
   This generates the ELF kernel binary at `target/aarch64-unknown-none/debug/kernel`.

2. **Create the VM in UTM**:
   * Click **+** (Create a New Virtual Machine).
   * Select **Virtualize** (uses native Apple Silicon virtualization).
   * Choose **Other** as the platform.
   * Check **Skip ISO boot**.

3. **Configure VM Settings**:
   * **System**:
     * Target: **QEMU Virtual Machine (virt)**.
     * Boot: Select **Direct Kernel Boot**.
     * Kernel Image: Select the built ELF kernel `target/aarch64-unknown-none/debug/kernel`.
     * RAM: Allocate **2048 MB**.
   * **Display & Devices**:
     * Add a **Virtual Display** backed by QEMU's **RAMFB** (corresponds to the guest `-device ramfb` driver).
     * Add a **Serial Console** interface (handles the `PL011` serial output).

4. **Launch the VM**:
   * Save settings and click the **Play** button.
   * The kernel will boot natively via the hypervisor, initializing the MMU, GIC, and showing the boot frame buffer.
