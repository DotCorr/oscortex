//! Platform driver bring-up — single entry from kernel main.
//!
//! PCI legacy and PS/2 drivers are x86/QEMU-profile devices today. Other
//! architectures compile stub PCI/port backends and skip probe cleanly.

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
        super::usb::probe();
    }
}

/// Storage, serial, and networking (after VFS init).
pub fn init_block_and_net() {
    super::uart::init();

    if pci::LEGACY_IO_AVAILABLE {
        super::virtio_net::init();
        super::virtio_blk::init();
        super::nvme::init();
    } else {
        log::info!("[Drivers] PCI legacy profile unavailable on this arch — block/NVMe/net skipped");
    }

    crate::app_store::init();
    crate::net::init();
}
