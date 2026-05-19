//! USB host-controller probe — Phase 41.
//!
//! Scans the PCI configuration space for XHCI (USB 3.x) host controllers
//! (class 0x0C, subclass 0x03, prog-if 0x30) using legacy I/O-port access
//! to PCI CF8/CFC (works on x86 BIOS and UEFI machines before MCFG is used).
//!
//! **This is a detection-only stub for Phase 41.**  No actual USB stack,
//! ring-buffer management, or HID report parsing is implemented yet.
//! The probe result is stored in `USB_XHCI_COUNT` and exposed via
//! `SYS_USB_CONTROLLER_COUNT` (0x376).
//!
//! Future phases will add:
//!   * MMIO BAR mapping for each XHCI controller.
//!   * USB 2.0/3.x enumeration and port reset.
//!   * HID class driver (keyboard, mouse) with report-descriptor parsing.

use core::sync::atomic::{AtomicU32, Ordering};

/// Number of XHCI controllers found during `probe()`.
pub static USB_XHCI_COUNT: AtomicU32 = AtomicU32::new(0);

/// Packed BAR0 base addresses of found controllers (up to 4).
static XHCI_BARS: [AtomicU32; 4] = [
    AtomicU32::new(0), AtomicU32::new(0),
    AtomicU32::new(0), AtomicU32::new(0),
];

// ── PCI config-space I/O port access ─────────────────────────────────────────

const PCI_CONFIG_ADDRESS: u16 = 0xCF8;
const PCI_CONFIG_DATA:    u16 = 0xCFC;

/// Read a 32-bit DWORD from PCI configuration space.
///
/// `bus`, `dev`, and `func` are the usual PCI coordinates.
/// `reg` is the DWORD register index (0..64).
#[inline]
unsafe fn pci_read32(bus: u8, dev: u8, func: u8, reg: u8) -> u32 {
    let addr: u32 = (1u32 << 31)           // enable bit
        | ((bus  as u32) << 16)
        | ((dev  as u32) << 11)
        | ((func as u32) << 8)
        | ((reg  as u32) << 2);
    let data: u32;
    core::arch::asm!(
        "out dx, eax",
        in("dx") PCI_CONFIG_ADDRESS,
        in("eax") addr,
        options(nomem, nostack),
    );
    core::arch::asm!(
        "in eax, dx",
        in("dx") PCI_CONFIG_DATA,
        out("eax") data,
        options(nomem, nostack),
    );
    data
}

// ── Probe ─────────────────────────────────────────────────────────────────────

/// Scan all 256 PCI buses × 32 devices × 8 functions for XHCI controllers.
///
/// XHCI: class=0x0C, subclass=0x03, prog-if=0x30.
///
/// Safe to call multiple times; subsequent calls are no-ops.
pub fn probe() {
    if USB_XHCI_COUNT.load(Ordering::Relaxed) != 0 {
        return; // already probed
    }

    let mut found = 0u32;

    'outer: for bus in 0u8..=255u8 {
        for dev in 0u8..32u8 {
            for func in 0u8..8u8 {
                let id = unsafe { pci_read32(bus, dev, func, 0) };
                if id == 0xFFFF_FFFF { continue; } // nothing here

                let class_reg = unsafe { pci_read32(bus, dev, func, 2) };
                let class    = (class_reg >> 24) as u8;
                let subclass = (class_reg >> 16) as u8;
                let progif   = (class_reg >>  8) as u8;

                if class == 0x0C && subclass == 0x03 && progif == 0x30 {
                    let bar0 = unsafe { pci_read32(bus, dev, func, 4) };
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
                    if found >= 4 { break 'outer; }
                }
            }
        }
    }

    USB_XHCI_COUNT.store(found, Ordering::Release);
    if found == 0 {
        log::info!("[USB] No XHCI controllers found — PS/2 input only");
    }
}

// ── Query helpers ─────────────────────────────────────────────────────────────

/// Return the number of XHCI controllers detected.
pub fn xhci_count() -> u32 {
    USB_XHCI_COUNT.load(Ordering::Relaxed)
}

/// Return the BAR0 value for XHCI controller `n` (0-based), or 0 if not found.
pub fn xhci_bar0(n: u32) -> u32 {
    if (n as usize) < XHCI_BARS.len() {
        XHCI_BARS[n as usize].load(Ordering::Relaxed)
    } else {
        0
    }
}
