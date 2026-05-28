//! USB XHCI host controller — PCI probe, MMIO map, and controller reset.
//!
//! Full HID → WM routing is a separate milestone; this module brings the host
//! controller to a known-good halted state via `arch::pci` + `arch::mmio`.

use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use crate::arch::{mmio, pci};

const MAX_CONTROLLERS: usize = 4;
const XHCI_MMIO_SIZE: usize = 0x10000;

/// Number of XHCI controllers successfully probed and reset.
pub static USB_XHCI_COUNT: AtomicU32 = AtomicU32::new(0);

static USB_XHCI_READY: AtomicBool = AtomicBool::new(false);

struct XhciController {
    bus: u8,
    dev: u8,
    func: u8,
    bar_phys: u64,
    bar_virt: u64,
    cap_length: u8,
    hci_version: u16,
    max_slots: u8,
    max_ports: u8,
}

static mut CONTROLLERS: [Option<XhciController>; MAX_CONTROLLERS] =
    [const { None }; MAX_CONTROLLERS];

/// Legacy BAR0 query for syscalls that only need the physical base.
pub fn xhci_bar0(n: u32) -> u32 {
    unsafe {
        if (n as usize) < MAX_CONTROLLERS {
            if let Some(ref c) = CONTROLLERS[n as usize] {
                return c.bar_phys as u32;
            }
        }
    }
    0
}

pub fn xhci_count() -> u32 {
    USB_XHCI_COUNT.load(Ordering::Acquire)
}

pub fn is_ready() -> bool {
    USB_XHCI_READY.load(Ordering::Acquire)
}

/// Scan PCI, map BAR0, and reset each XHCI controller (class 0x0C / 0x03 / 0x30).
pub fn probe_and_init() {
    if USB_XHCI_COUNT.load(Ordering::Relaxed) != 0 {
        return;
    }
    if !pci::LEGACY_IO_AVAILABLE {
        log::info!("[USB] XHCI skipped — PCI unavailable on this arch");
        return;
    }

    let hhdm = crate::mm::frame_allocator::hhdm_offset();
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
                if class != 0x0C || subclass != 0x03 || progif != 0x30 {
                    continue;
                }

                let bar_phys = match pci::bar0_mmio_phys(bus, dev, func) {
                    Some(b) => b,
                    None => {
                        log::warn!(
                            "[USB] XHCI at {:02X}:{:02X}.{} has no MMIO BAR0 — skipped",
                            bus, dev, func
                        );
                        continue;
                    }
                };

                let vendor = (id & 0xFFFF) as u16;
                let device = (id >> 16) as u16;
                pci::enable_io_and_busmaster(bus, dev, func);

                let bar_virt = bar_phys + hhdm;
                unsafe {
                    crate::mm::paging::map_mmio(bar_phys, bar_virt, XHCI_MMIO_SIZE);
                }

                match init_controller(bus, dev, func, bar_phys, bar_virt, vendor, device) {
                    Ok(ctrl) => {
                        if (found as usize) < MAX_CONTROLLERS {
                            unsafe {
                                CONTROLLERS[found as usize] = Some(ctrl);
                            }
                            found += 1;
                        }
                    }
                    Err(msg) => {
                        log::warn!(
                            "[USB] XHCI {:02X}:{:02X}.{} init failed: {}",
                            bus, dev, func, msg
                        );
                    }
                }

                if found >= MAX_CONTROLLERS as u32 {
                    break;
                }
            }
            if found >= MAX_CONTROLLERS as u32 {
                break;
            }
        }
        if found >= MAX_CONTROLLERS as u32 {
            break;
        }
    }

    USB_XHCI_COUNT.store(found, Ordering::Release);
    USB_XHCI_READY.store(found > 0, Ordering::Release);

    if found == 0 {
        log::info!("[USB] No XHCI controllers found — PS/2 input only");
    } else {
        log::info!("[USB] {} XHCI controller(s) mapped and reset", found);
    }
}

fn init_controller(
    bus: u8,
    dev: u8,
    func: u8,
    bar_phys: u64,
    bar_virt: u64,
    vendor: u16,
    device: u16,
) -> Result<XhciController, &'static str> {
    unsafe {
        let cap = mmio::read32(bar_virt, 0);
        let cap_length = (cap & 0xFF) as u8;
        if cap_length == 0 {
            return Err("invalid CAPLENGTH");
        }

        let hci_version = ((cap >> 16) & 0xFFFF) as u16;
        let hcsparams1 = mmio::read32(bar_virt, cap_length as usize);
        let max_slots = (hcsparams1 & 0xFF) as u8;
        let max_ports = ((hcsparams1 >> 24) & 0xFF) as u8;

        let usbcmd_off = cap_length as usize;
        let usbsts_off = usbcmd_off + 4;

        const USBCMD_HCRST: u32 = 1 << 1;
        const USBSTS_HCH: u32 = 1 << 12;

        let mut usbcmd = mmio::read32(bar_virt, usbcmd_off);
        usbcmd |= USBCMD_HCRST;
        mmio::write32(bar_virt, usbcmd_off, usbcmd);

        for _ in 0..1_000_000 {
            usbcmd = mmio::read32(bar_virt, usbcmd_off);
            if usbcmd & USBCMD_HCRST == 0 {
                break;
            }
            crate::arch::spin_pause();
        }
        if usbcmd & USBCMD_HCRST != 0 {
            return Err("controller reset timeout");
        }

        let usbsts = mmio::read32(bar_virt, usbsts_off);
        if usbsts & USBSTS_HCH == 0 {
            return Err("controller not halted after reset");
        }

        log::info!(
            "[USB] XHCI ready: {:02X}:{:02X}.{} vendor={:#06X} device={:#06X} BAR={:#x} HCI={:#06X} slots={} ports={}",
            bus,
            dev,
            func,
            vendor,
            device,
            bar_phys,
            hci_version,
            max_slots,
            max_ports
        );

        Ok(XhciController {
            bus,
            dev,
            func,
            bar_phys,
            bar_virt,
            cap_length,
            hci_version,
            max_slots,
            max_ports,
        })
    }
}
