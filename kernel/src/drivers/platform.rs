//! Platform driver bring-up — single entry from kernel main.
//!
//! All PCI/port/MMIO access stays in `arch/`; drivers register here and in the
//! CDP registry when probe succeeds.

use core::sync::atomic::Ordering;

use crate::arch::pci;

/// Driver registry + input devices (before compositor).
pub fn init_early(qemu_like: bool) {
    super::registry::init();

    if cfg!(target_arch = "x86_64") && qemu_like {
        super::ps2::init();
        unsafe { super::ps2::enable_pic_irqs() };
        log::info!("[Input] PS/2 enabled (QEMU/KVM profile)");
    } else if cfg!(target_arch = "x86_64") {
        log::warn!("[Input] PS/2 skipped (bare-metal safe mode)");
    }

    if pci::LEGACY_IO_AVAILABLE {
        super::usb::probe_and_init();
    }

    register_input_natives(qemu_like);
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
        log::info!(
            "[Drivers] PCI profile unavailable on this arch — block/NVMe/net skipped"
        );
    }

    register_block_natives();

    // HID→WM path self-test (software route; independent of xHCI ring bring-up).
    if cfg!(target_arch = "x86_64") {
        super::usb_hid::wm_route_self_test();
    }

    crate::app_store::init();
    crate::net::init();
}

fn register_input_natives(qemu_like: bool) {
    if qemu_like && super::ps2::PS2_READY.load(Ordering::Acquire) {
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
