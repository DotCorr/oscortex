//! Host-side unit tests for kernel driver pure logic.
//!
//! Sources under test are included from `kernel/src/drivers/common/` so tests
//! stay in sync with the kernel without a full no_std host build.

mod bare_metal_profile;
mod driver_manifest;
mod nvme_regs;
mod pci_bar;
mod usb_hid;
mod virtio_net_frame;
mod vring;
mod xhci_caps;
