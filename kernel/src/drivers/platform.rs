//! Platform driver bring-up — single entry from kernel main.
//!
//! All PCI/port/MMIO access stays in `arch/`; drivers register here and in the
//! CDP registry when probe succeeds.

use core::sync::atomic::Ordering;

use crate::arch::pci;

/// Driver registry + input devices (before compositor).
pub fn init_early(qemu_like: bool) {
    super::registry::init();

    if cfg!(target_arch = "x86_64") {
        // ps2::init() is fully bounded/fail-safe: it bails immediately when the i8042
        // status reads 0xFF (no controller present) and every flush/wait loop is
        // iteration-capped, so it cannot hang even on exotic firmware. The old
        // "bare-metal safe mode" skip predated that hardening (it dated from an
        // UNBOUNDED i8042 flush that hung UTM) and left REAL hardware with no input at
        // all — on a laptop the keyboard, and the trackpad as a generic PS/2 mouse, IS
        // the i8042. Run it everywhere; it no-ops cleanly when the controller is absent
        // (e.g. a USB-only board), and real USB-HID still binds via the xHCI probe below.
        super::ps2::init();
        unsafe { super::ps2::enable_pic_irqs() };
        if qemu_like {
            log::info!("[Input] PS/2 init (hypervisor profile)");
        } else {
            log::info!("[Input] PS/2 init (bare-metal)");
        }
    }

    // USB xHCI HID probe. Skipped on the aarch64 UEFI/Limine ISO build: probing
    // the firmware-configured xHCI's PCIe config space under QEMU+HVF trips a
    // QEMU host-side `assert(isv)` (hvf.c) that kills the VM — a QEMU+HVF host bug
    // on the edk2 boot path, not present on real hardware, under TCG, or on the
    // bare `-kernel` boot (which self-assigns BARs and works fully). The `-kernel`
    // artifact is the supported UTM arm64 path and keeps USB HID; x86 keeps it too.
    #[cfg(not(all(target_arch = "aarch64", feature = "limine-boot")))]
    {
        if pci::PCI_AVAILABLE {
            super::usb::probe_and_init();
        }
    }

    register_input_natives();
}

/// Storage, serial, and networking (after VFS init).
pub fn init_block_and_net() {
    super::uart::init();
    let _ = super::registry::register_native(b"uart");

    if pci::LEGACY_IO_AVAILABLE {
        super::virtio_net::init();
        super::virtio_blk::init();
        super::nvme::init();
    } else {
        log::info!("[Drivers] PCI profile unavailable on this arch — block/NVMe/net skipped");
    }

    register_block_natives();

    // HID→WM path self-test (software route; independent of xHCI ring bring-up).
    if cfg!(target_arch = "x86_64") {
        super::usb_hid::wm_route_self_test();
    }

    crate::app_store::init();
    crate::app_registry::install_system_apps();
    crate::net::init();
}

fn register_input_natives() {
    // Register whatever input actually bound, on any platform — PS/2 is no longer
    // hypervisor-gated (see init_early), so a bare-metal i8042 registers too.
    if super::ps2::PS2_READY.load(Ordering::Acquire) {
        let _ = super::registry::register_native(b"ps2");
    }
    if super::usb::is_ready() {
        let _ = super::registry::register_native(b"xhci");
    }
}

fn register_block_natives() {
    if super::virtio_net::is_ready() {
        let _ = super::registry::register_native(b"virtio-net");
    }
    if super::virtio_blk::is_ready() {
        let _ = super::registry::register_native(b"virtio-blk");
    }
    if super::nvme::is_ready() {
        let _ = super::registry::register_native(b"nvme");
    }
}
