//! USB XHCI host controller — PCI probe, MMIO map, and controller reset.
//!
//! Full HID → WM routing is a separate milestone; this module brings the host
//! controller to a known-good halted state via `arch::pci` + `arch::mmio`.

use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use crate::arch::{mmio, pci};
use crate::drivers::common::pci_bar;
use crate::drivers::common::xhci_caps::{
    halted_after_reset, parse_capability, reset_complete, USBCMD_HCRST, USBSTS_HCH,
};

const MAX_CONTROLLERS: usize = 4;
const XHCI_MMIO_SIZE: usize = 0x10000;

/// Number of XHCI controllers successfully probed and reset.
pub static USB_XHCI_COUNT: AtomicU32 = AtomicU32::new(0);

static USB_XHCI_READY: AtomicBool = AtomicBool::new(false);

pub(crate) struct XhciController {
    bus: u8,
    dev: u8,
    func: u8,
    bar_phys: u64,
    pub(crate) bar_virt: u64,
    cap_length: u8,
    hci_version: u16,
    pub(crate) max_slots: u8,
    pub(crate) max_ports: u8,
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

/// Poll xHCI event rings and route live HID keyboard reports to WM.
pub fn poll() {
    super::xhci_runtime::poll();
}

/// Scan PCI, map BAR0, and reset each XHCI controller (class 0x0C / 0x03 / 0x30).
pub fn probe_and_init() {
    if USB_XHCI_COUNT.load(Ordering::Relaxed) != 0 {
        return;
    }
    if !pci::PCI_AVAILABLE {
        log::info!("[USB] XHCI skipped — PCI unavailable on this arch");
        return;
    }

    let hhdm = crate::mm::frame_allocator::hhdm_offset();
    let mut found = 0u32;

    // QEMU `virt` (aarch64) exposes a single PCIe bus; scanning all 256 buses
    // over ECAM MMIO is slow and pointless. x86 keeps the full legacy scan.
    let bus_limit: u8 = if cfg!(target_arch = "aarch64") { 1 } else { 255 };

    for bus in 0u8..=bus_limit {
        for dev in 0u8..32u8 {
            for func in 0u8..8u8 {
                let id = pci::config_read32(bus, dev, func, 0x00);
                if id == 0xFFFF_FFFF {
                    continue;
                }
                let class_reg = pci::config_read32(bus, dev, func, 0x08);
                if !pci_bar::is_xhci(class_reg) {
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

                // On aarch64 the QEMU virt 32-bit MMIO window (where the assigned
                // BAR lands) is already covered by the boot identity map's Device
                // entry 0, so the BAR is reachable at bar_phys directly (hhdm = 0).
                // Do NOT re-map it — double-mapping the 1GiB block faults. On x86
                // the BAR is reached via the HHDM after an explicit map_mmio.
                #[cfg(target_arch = "aarch64")]
                let bar_virt = bar_phys;
                #[cfg(not(target_arch = "aarch64"))]
                let bar_virt = {
                    let v = bar_phys + hhdm;
                    unsafe { crate::mm::paging::map_mmio(bar_phys, v, XHCI_MMIO_SIZE); }
                    v
                };

                // Enabling memory-space decode makes QEMU rebuild the guest memory
                // map for the BAR; under HVF a timer-ISR MMIO access racing that
                // rebuild hangs the vCPU. Mask interrupts across the enable AND the
                // first controller MMIO (init_controller) so nothing else touches
                // device memory mid-remap.
                let irq = crate::arch::interrupts_save_and_disable();
                pci::enable_io_and_busmaster(bus, dev, func);
                let init = init_controller(bus, dev, func, bar_phys, bar_virt, vendor, device);
                crate::arch::interrupts_restore(irq);

                match init {
                    Ok(ctrl) => {
                        if (found as usize) < MAX_CONTROLLERS {
                            if let Err(e) = super::xhci_runtime::start(&ctrl) {
                                log::warn!("[USB] XHCI runtime start failed: {}", e);
                            }
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
        // HCSPARAMS1 is at the fixed capability-register offset 0x04, NOT at
        // CAPLENGTH (that's where the operational registers begin).
        let hcsparams1 = mmio::read32(bar_virt, 0x04);
        let caps = parse_capability(cap, hcsparams1)?;

        let usbcmd_off = caps.usbcmd_off;
        let usbsts_off = caps.usbsts_off;

        let mut usbcmd = mmio::read32(bar_virt, usbcmd_off);
        usbcmd |= USBCMD_HCRST;
        mmio::write32(bar_virt, usbcmd_off, usbcmd);

        for _ in 0..1_000_000 {
            usbcmd = mmio::read32(bar_virt, usbcmd_off);
            if reset_complete(usbcmd) {
                break;
            }
            crate::arch::spin_pause();
        }
        if !reset_complete(usbcmd) {
            return Err("controller reset timeout");
        }

        let usbsts = mmio::read32(bar_virt, usbsts_off);
        if !halted_after_reset(usbsts) {
            log::warn!(
                "[USB] XHCI {:02X}:{:02X}.{} not halted after reset (USBSTS={:#x}) — continuing",
                bus, dev, func, usbsts
            );
        }

        log::info!(
            "[USB] XHCI ready: {:02X}:{:02X}.{} vendor={:#06X} device={:#06X} BAR={:#x} HCI={:#06X} slots={} ports={}",
            bus,
            dev,
            func,
            vendor,
            device,
            bar_phys,
            caps.hci_version,
            caps.max_slots,
            caps.max_ports
        );

        Ok(XhciController {
            bus,
            dev,
            func,
            bar_phys,
            bar_virt,
            cap_length: caps.cap_length,
            hci_version: caps.hci_version,
            max_slots: caps.max_slots,
            max_ports: caps.max_ports,
        })
    }
}
