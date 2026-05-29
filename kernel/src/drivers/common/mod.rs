//! Pure, host-testable helpers shared by native drivers.
//!
//! Unit tests live in `tests/kernel/unit/` and include these sources directly.

pub mod nvme_regs;
pub mod pci_bar;
pub mod usb_hid;
pub mod vring;
pub mod virtio_net_frame;
pub mod xhci_caps;
