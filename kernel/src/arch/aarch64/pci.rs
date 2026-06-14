//! PCI configuration space — aarch64 PCIe ECAM (memory-mapped config).
//!
//! aarch64 has no legacy CF8/CFC config ports. On the QEMU `virt` machine PCIe
//! config space is exposed as a memory-mapped ECAM region. The DTB `pcie@…`
//! node gives (verified against `qemu-system-aarch64 -M virt`, QEMU 11):
//!   * ECAM `reg`    = <0x40 0x1000_0000  0x00 0x1000_0000>
//!                     → base 0x40_1000_0000 (257 GiB), size 256 MiB.
//!   * 32-bit MMIO   = bus 0x1000_0000 → cpu 0x1000_0000, size 0x2eff_0000
//!                     → window 0x1000_0000–0x3eff_0000 (BARs land here).
//!
//! The 32-bit MMIO/BAR window (0x1000_0000–0x3eff_0000) is below 0x4000_0000 and
//! is already covered by the boot identity map's Device entry 0 (mmu.rs), so a
//! decoded BAR0 is directly accessible at its physical address (hhdm = 0 on
//! aarch64). The ECAM region itself sits at 257 GiB, outside the boot map, so we
//! map its 1 GiB block on first use via `mmu::map_device_1gib`.
//!
//! ECAM address = ECAM_BASE + ((bus<<20)|(dev<<15)|(func<<12)) + (off & 0xFFC).

use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

/// Bump allocator for the QEMU `virt` 32-bit PCIe MMIO window
/// (0x1000_0000–0x3eff_0000). On `-kernel` boot there is no firmware to program
/// PCIe BARs, so the kernel assigns them itself out of this window.
static MMIO_ALLOC_NEXT: AtomicU64 = AtomicU64::new(0x1000_0000);
const MMIO_WINDOW_END: u64 = 0x3eff_0000;

use crate::arch::mmio;
use crate::drivers::common::pci_bar;

/// QEMU `virt` PCIe ECAM physical base (see module docs).
const ECAM_BASE: u64 = 0x40_1000_0000;

/// True when this architecture exposes legacy PCI config I/O ports (CF8/CFC).
/// aarch64 has none — config space is the ECAM MMIO window instead.
pub const LEGACY_IO_AVAILABLE: bool = false;

/// True when PCI config space is reachable at all on this arch (via ECAM here).
pub const PCI_AVAILABLE: bool = true;

static ECAM_MAPPED: AtomicBool = AtomicBool::new(false);

/// Ensure the ECAM 1 GiB block is identity-mapped before any config access.
fn ensure_ecam_mapped() {
    if ECAM_MAPPED.swap(true, Ordering::AcqRel) {
        return;
    }
    unsafe { crate::arch::aarch64::mmu::map_device_1gib(ECAM_BASE) };
}

#[inline]
fn cfg_addr(bus: u8, dev: u8, func: u8, offset: u8) -> u64 {
    ECAM_BASE
        + (((bus as u64) << 20) | ((dev as u64) << 15) | ((func as u64) << 12))
        + ((offset as u64) & 0xFFC)
}

/// Read a 32-bit PCI config DWORD. `offset` is the byte offset (0, 4, 8, …).
pub fn config_read32(bus: u8, dev: u8, func: u8, offset: u8) -> u32 {
    ensure_ecam_mapped();
    unsafe { mmio::read32(cfg_addr(bus, dev, func, offset), 0) }
}

/// Write a 32-bit PCI config DWORD.
pub fn config_write32(bus: u8, dev: u8, func: u8, offset: u8, val: u32) {
    ensure_ecam_mapped();
    unsafe { mmio::write32(cfg_addr(bus, dev, func, offset), 0, val) }
}

/// Enable memory space + bus mastering on a PCI function (bits 1 and 2 of the
/// command register at config offset 0x04). aarch64 PCIe BARs are memory-mapped,
/// so we set memory-space rather than the x86 legacy I/O-space bit.
pub fn enable_io_and_busmaster(bus: u8, dev: u8, func: u8) {
    let cmd = config_read32(bus, dev, func, 0x04);
    // Enable memory-space decode (bit 1), then bus-master (bit 2), with a settle
    // gap after each. Enabling memory-space remaps the BAR's MMIO region; under
    // HVF the host rebuild is async and a racing access hangs the vCPU.
    config_write32(bus, dev, func, 0x04, cmd | (1 << 1));
    pci_settle();
    config_write32(bus, dev, func, 0x04, cmd | (1 << 1) | (1 << 2));
    pci_settle();
}

/// Barrier + wall-clock gap to let QEMU's HVF-side memory-map rebuild complete
/// before the next config/MMIO access (see bar0_mmio_phys).
fn pci_settle() {
    unsafe {
        core::arch::asm!("dsb sy", "isb", options(nomem, nostack));
    }
    for _ in 0..5_000_000u64 {
        core::hint::spin_loop();
    }
}

/// BAR0 as a legacy I/O port base. Meaningless on aarch64 (no port I/O) — 0.
pub fn bar0_io_base(_bus: u8, _dev: u8, _func: u8) -> u16 {
    0
}

/// BAR0 as a memory-mapped physical base (I/O-space BARs return None).
///
/// On the QEMU `virt` machine booted via `-kernel` there is no firmware to
/// program PCIe BARs, so BAR0 reads back with its address field zeroed (only the
/// type bits are set, e.g. `0x4` = 64-bit memory). When that happens we size the
/// BAR (write all-ones, read the mask back) and assign it an aligned address out
/// of the 32-bit MMIO window before returning it.
///
/// UEFI/edk2 boot is different: the firmware DOES assign BARs, and for a 64-bit
/// BAR it places them in the high PCIe MMIO window at 512 GiB
/// (e.g. 0x80_0000_8000). That address is outside our 39-bit TTBR0 identity map
/// (and `map_device_1gib` can't reach it), so touching it faults. We only accept
/// a firmware assignment that lands in the low 32-bit window we can identity-map;
/// anything higher (or unassigned) is (re)assigned into that window.
pub fn bar0_mmio_phys(bus: u8, dev: u8, func: u8) -> Option<u64> {
    let bar0_lo = config_read32(bus, dev, func, 0x10);
    if bar0_lo & 1 != 0 {
        return None; // I/O-space BAR — no MMIO base
    }
    let is64 = (bar0_lo >> 1) & 0x3 == 0x2;
    let bar0_hi = if is64 { config_read32(bus, dev, func, 0x14) } else { 0 };
    let current = ((bar0_hi as u64) << 32) | ((bar0_lo & !0xF) as u64);
    // Accept an existing assignment only if it's inside the identity-mapped low
    // 32-bit MMIO window; otherwise fall through and re-assign it there.
    if current != 0 && current < MMIO_WINDOW_END {
        return Some(current);
    }

    // Unassigned (or assigned too high) — size the BAR (write all-ones, read the
    // mask back) and re-assign it into the low 32-bit window below.
    config_write32(bus, dev, func, 0x10, 0xFFFF_FFFF);
    let size_lo = config_read32(bus, dev, func, 0x10);
    let size_hi = if is64 {
        config_write32(bus, dev, func, 0x14, 0xFFFF_FFFF);
        config_read32(bus, dev, func, 0x14)
    } else {
        0xFFFF_FFFF
    };
    let mask = ((size_hi as u64) << 32) | ((size_lo & !0xF) as u64);
    if mask == 0 {
        // No address bits implemented — restore and bail.
        config_write32(bus, dev, func, 0x10, bar0_lo);
        if is64 {
            config_write32(bus, dev, func, 0x14, bar0_hi);
        }
        return None;
    }
    let size = (!mask).wrapping_add(1);
    let align = if size < 0x1000 { 0x1000 } else { size };

    // Bump-allocate an aligned address out of the 32-bit MMIO window.
    let base = loop {
        let next = MMIO_ALLOC_NEXT.load(Ordering::Acquire);
        let aligned = (next + align - 1) & !(align - 1);
        if aligned + size > MMIO_WINDOW_END {
            config_write32(bus, dev, func, 0x10, bar0_lo);
            if is64 {
                config_write32(bus, dev, func, 0x14, bar0_hi);
            }
            return None; // window exhausted
        }
        if MMIO_ALLOC_NEXT
            .compare_exchange(next, aligned + size, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
        {
            break aligned;
        }
    };

    // Programming the BAR triggers QEMU to (asynchronously, under HVF) rebuild the
    // guest memory map for the new MMIO region. If anything (notably the timer
    // ISR's framebuffer/GIC MMIO) accesses device memory while that rebuild is in
    // flight, the vCPU hangs hard. Mask interrupts across the write + a settle gap
    // (full completion barrier + wall-clock spin, no MMIO) so the remap finishes
    // uncontended before the next config access (e.g. enabling memory decode).
    let irq = crate::arch::interrupts_save_and_disable();
    config_write32(bus, dev, func, 0x10, (base as u32 & 0xFFFF_FFF0) | (bar0_lo & 0xF));
    if is64 {
        config_write32(bus, dev, func, 0x14, (base >> 32) as u32);
    }
    unsafe {
        core::arch::asm!("dsb sy", "isb", options(nomem, nostack));
    }
    for _ in 0..5_000_000u64 {
        core::hint::spin_loop();
    }
    crate::arch::interrupts_restore(irq);
    Some(base)
}

/// Locate a PCI function by class code; returns BDF + MMIO BAR0 physical base.
pub fn find_device_pci(class: u8, subclass: u8, prog_if: u8) -> Option<(u8, u8, u8, u64)> {
    // QEMU `virt` exposes a single PCIe bus; scan 0..=1 to stay cheap on MMIO.
    for bus in 0u8..=1 {
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

/// Scan a bus for a VirtIO device with the given vendor/device ID.
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

/// Scan for a device matching PCI class / subclass / prog-if; return BAR0 phys.
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
