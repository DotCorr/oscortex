//! PCI configuration space — x86 legacy I/O ports CF8/CFC.
//!
//! All kernel PCI probing goes through this module. Drivers must not embed
//! config-space port I/O directly.

use super::port_io;

const CONFIG_ADDRESS: u16 = 0xCF8;
const CONFIG_DATA: u16 = 0xCFC;

/// True when this architecture exposes legacy PCI config I/O ports.
pub const LEGACY_IO_AVAILABLE: bool = true;

/// Read a 32-bit PCI config DWORD. `offset` is the byte offset (0, 4, 8, …).
pub fn config_read32(bus: u8, dev: u8, func: u8, offset: u8) -> u32 {
    let addr: u32 = (1u32 << 31)
        | ((bus as u32) << 16)
        | ((dev as u32) << 11)
        | ((func as u32) << 8)
        | ((offset as u32) & 0xFC);
    unsafe {
        port_io::outl(CONFIG_ADDRESS, addr);
        port_io::inl(CONFIG_DATA)
    }
}

/// Write a 32-bit PCI config DWORD.
pub fn config_write32(bus: u8, dev: u8, func: u8, offset: u8, val: u32) {
    let addr: u32 = (1u32 << 31)
        | ((bus as u32) << 16)
        | ((dev as u32) << 11)
        | ((func as u32) << 8)
        | ((offset as u32) & 0xFC);
    unsafe {
        port_io::outl(CONFIG_ADDRESS, addr);
        port_io::outl(CONFIG_DATA, val);
    }
}

/// Enable I/O space + bus mastering on a PCI function (virtio legacy bring-up).
pub fn enable_io_and_busmaster(bus: u8, dev: u8, func: u8) {
    let cmd = config_read32(bus, dev, func, 0x04);
    config_write32(bus, dev, func, 0x04, cmd | 0x05);
}

/// BAR0 as a legacy I/O port base (bit 0 set). Returns 0 if MMIO or absent.
pub fn bar0_io_base(bus: u8, dev: u8, func: u8) -> u16 {
    let bar0 = config_read32(bus, dev, func, 0x10);
    crate::drivers::common::pci_bar::decode_bar0_io(bar0).unwrap_or(0)
}

/// BAR0 as a memory-mapped physical base (I/O-space BARs return None).
pub fn bar0_mmio_phys(bus: u8, dev: u8, func: u8) -> Option<u64> {
    let bar0_lo = config_read32(bus, dev, func, 0x10);
    let bar0_hi = config_read32(bus, dev, func, 0x14);
    crate::drivers::common::pci_bar::decode_bar0_mmio(bar0_lo, bar0_hi)
}

/// Locate a PCI function by class code; returns BDF + MMIO BAR0 physical base.
pub fn find_device_pci(class: u8, subclass: u8, prog_if: u8) -> Option<(u8, u8, u8, u64)> {
    for bus in 0u8..=255 {
        for dev in 0u8..32 {
            for func in 0u8..8 {
                let id = config_read32(bus, dev, func, 0x00);
                if id == 0xFFFF_FFFF {
                    continue;
                }
                let cc = config_read32(bus, dev, func, 0x08);
                let dev_class = ((cc >> 24) & 0xFF) as u8;
                let dev_subclass = ((cc >> 16) & 0xFF) as u8;
                let dev_progif = ((cc >> 8) & 0xFF) as u8;
                if dev_class == class && dev_subclass == subclass && dev_progif == prog_if {
                    let bar = bar0_mmio_phys(bus, dev, func)?;
                    return Some((bus, dev, func, bar));
                }
            }
        }
    }
    None
}

/// Scan bus 0 for a VirtIO device with the given vendor/device ID.
pub fn find_virtio_legacy(bus: u8, vendor: u16, device: u16) -> Option<(u8, u8)> {
    for dev in 0u8..32 {
        let id = config_read32(bus, dev, 0, 0x00);
        if id == 0xFFFF_FFFF {
            continue;
        }
        if (id as u16) == vendor && ((id >> 16) as u16) == device {
            return Some((bus, dev));
        }
    }
    None
}

/// Scan all buses for a device matching PCI class / subclass / prog-if;
/// return the physical address of BAR0 (memory-mapped, 64-bit aware).
pub fn find_device_bar0(class: u8, subclass: u8, prog_if: u8) -> Option<u64> {
    find_device_pci(class, subclass, prog_if).map(|(_, _, _, bar)| bar)
}

/// Count devices matching class / subclass / prog-if (up to `max` results).
pub fn count_class(bus_limit: u8, class: u8, subclass: u8, prog_if: u8, max: u32) -> u32 {
    let mut found = 0u32;
    'outer: for bus in 0u8..=bus_limit {
        for dev in 0u8..32 {
            for func in 0u8..8 {
                let id = config_read32(bus, dev, func, 0x00);
                if id == 0xFFFF_FFFF {
                    continue;
                }
                let class_reg = config_read32(bus, dev, func, 0x08);
                let c = (class_reg >> 24) as u8;
                let sc = (class_reg >> 16) as u8;
                let pi = (class_reg >> 8) as u8;
                if c == class && sc == subclass && pi == prog_if {
                    found += 1;
                    if found >= max {
                        break 'outer;
                    }
                }
            }
        }
    }
    found
}
