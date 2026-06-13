//! Architecture-neutral PCI facade — delegates to the active arch backend.

cfg_if::cfg_if! {
    if #[cfg(target_arch = "x86_64")] {
        pub use crate::arch::x86_64::pci::*;
    } else if #[cfg(target_arch = "aarch64")] {
        pub use crate::arch::aarch64::pci::*;
    } else {
        pub const LEGACY_IO_AVAILABLE: bool = false;
        pub const PCI_AVAILABLE: bool = false;

        pub fn config_read32(_bus: u8, _dev: u8, _func: u8, _offset: u8) -> u32 {
            0xFFFF_FFFF
        }

        pub fn config_write32(_bus: u8, _dev: u8, _func: u8, _offset: u8, _val: u32) {}

        pub fn enable_io_and_busmaster(_bus: u8, _dev: u8, _func: u8) {}

        pub fn bar0_io_base(_bus: u8, _dev: u8, _func: u8) -> u16 {
            0
        }

        pub fn bar0_mmio_phys(_bus: u8, _dev: u8, _func: u8) -> Option<u64> {
            None
        }

        pub fn find_device_pci(_class: u8, _subclass: u8, _prog_if: u8) -> Option<(u8, u8, u8, u64)> {
            None
        }

        pub fn find_virtio_legacy(_bus: u8, _vendor: u16, _device: u16) -> Option<(u8, u8)> {
            None
        }

        pub fn find_device_bar0(_class: u8, _subclass: u8, _prog_if: u8) -> Option<u64> {
            None
        }

        pub fn count_class(_bus_limit: u8, _class: u8, _subclass: u8, _prog_if: u8, _max: u32) -> u32 {
            0
        }
    }
}
