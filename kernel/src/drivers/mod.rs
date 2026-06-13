//! Driver subsystem root.

#[cfg(target_arch = "x86_64")]
pub mod beep;
pub mod bootscreen;
pub mod common;
pub mod fb;
pub mod nvme;
pub mod platform;
pub mod ps2;
pub mod pty;
pub mod registry;
pub mod uart;
pub mod usb;
pub mod usb_hid;
pub mod xhci_runtime;
pub mod virtio_blk;
#[cfg(target_arch = "aarch64")]
pub mod virtio_input;
pub mod virtio_net;
pub mod wasm_sandbox;
