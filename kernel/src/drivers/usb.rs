//! USB host-controller probe — Phase 41.
//!
//! Detection-only stub: counts XHCI controllers via PCI class probe.
//! Full USB stack + HID → WM path is future CDP / native driver work.

use core::sync::atomic::{AtomicU32, Ordering};

use crate::arch::pci;

/// Number of XHCI controllers found during `probe()`.
pub static USB_XHCI_COUNT: AtomicU32 = AtomicU32::new(0);

/// Packed BAR0 base addresses of found controllers (up to 4).
static XHCI_BARS: [AtomicU32; 4] = [
    AtomicU32::new(0), AtomicU32::new(0),
    AtomicU32::new(0), AtomicU32::new(0),
];

/// Scan PCI for XHCI controllers (class 0x0C / 0x03 / 0x30).
pub fn probe() {
    if USB_XHCI_COUNT.load(Ordering::Relaxed) != 0 {
        return;
    }
    if !pci::LEGACY_IO_AVAILABLE {
        log::info!("[USB] probe skipped — no legacy PCI on this arch");
        return;
    }

    let mut found = 0u32;
    for bus in 0u8..=255u8 {
        for dev in 0u8..32u8 {
            for func in 0u8..8u8 {
                let id = pci::config_read32(bus, dev, func, 0x00);
                if id == 0xFFFF_FFFF {
                    continue;
                }
                let class_reg = pci::config_read32(bus, dev, func, 0x08);
                let class = (class_reg >> 24) as u8;
                let subclass = (class_reg >> 16) as u8;
                let progif = (class_reg >> 8) as u8;
                if class == 0x0C && subclass == 0x03 && progif == 0x30 {
                    let bar0 = pci::config_read32(bus, dev, func, 0x10);
                    let vendor = (id & 0xFFFF) as u16;
                    let device = (id >> 16) as u16;
                    log::info!(
                        "[USB] XHCI found: bus={:02X} dev={:02X} fn={} vendor={:#06X} device={:#06X} BAR0={:#010X}",
                        bus, dev, func, vendor, device, bar0
                    );
                    if (found as usize) < XHCI_BARS.len() {
                        XHCI_BARS[found as usize].store(bar0, Ordering::Relaxed);
                    }
                    found += 1;
                    if found >= 4 {
                        USB_XHCI_COUNT.store(found, Ordering::Release);
                        return;
                    }
                }
            }
        }
    }

    USB_XHCI_COUNT.store(found, Ordering::Release);
    if found == 0 {
        log::info!("[USB] No XHCI controllers found — PS/2 input only");
    }
}

pub fn xhci_count() -> u32 {
    USB_XHCI_COUNT.load(Ordering::Relaxed)
}

pub fn xhci_bar0(n: u32) -> u32 {
    if (n as usize) < XHCI_BARS.len() {
        XHCI_BARS[n as usize].load(Ordering::Relaxed)
    } else {
        0
    }
}
