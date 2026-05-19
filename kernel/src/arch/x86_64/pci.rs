//! Minimal PCI bus scanner — x86 port I/O based (PCI Configuration Space).
//! Used by the NVMe driver to find the controller BAR0.

const CONFIG_ADDRESS: u16 = 0xCF8;
const CONFIG_DATA:    u16 = 0xCFC;

#[inline]
unsafe fn pci_read32(bus: u8, dev: u8, func: u8, off: u8) -> u32 {
    let addr: u32 = (1u32 << 31)
        | ((bus  as u32) << 16)
        | ((dev  as u32) << 11)
        | ((func as u32) <<  8)
        | ((off  as u32) & 0xFC);
    core::arch::asm!(
        "out dx, eax",
        in("dx") CONFIG_ADDRESS,
        in("eax") addr,
        options(nomem, nostack, preserves_flags)
    );
    let mut val: u32;
    core::arch::asm!(
        "in eax, dx",
        in("dx") CONFIG_DATA,
        out("eax") val,
        options(nomem, nostack, preserves_flags)
    );
    val
}

/// Scan PCI for a device matching (class, subclass, prog_if) and return
/// the physical address of its BAR0 (memory-mapped, 64-bit aware).
pub fn find_device_bar0(class: u8, subclass: u8, prog_if: u8) -> Option<u64> {
    for bus in 0u8..=255 {
        for dev in 0u8..32 {
            for func in 0u8..8 {
                let id = unsafe { pci_read32(bus, dev, func, 0x00) };
                if id == 0xFFFF_FFFF { continue; }
                let cc = unsafe { pci_read32(bus, dev, func, 0x08) };
                let dev_class    = ((cc >> 24) & 0xFF) as u8;
                let dev_subclass = ((cc >> 16) & 0xFF) as u8;
                let dev_progif   = ((cc >>  8) & 0xFF) as u8;
                if dev_class == class && dev_subclass == subclass && dev_progif == prog_if {
                    // Read BAR0 — may be 64-bit.
                    let bar0_lo = unsafe { pci_read32(bus, dev, func, 0x10) };
                    if bar0_lo & 0x1 != 0 { continue; } // I/O space BAR, skip
                    let bar0_type = (bar0_lo >> 1) & 0x3;
                    let phys_lo = (bar0_lo & !0xF) as u64;
                    if bar0_type == 0x2 {
                        // 64-bit BAR
                        let bar0_hi = unsafe { pci_read32(bus, dev, func, 0x14) } as u64;
                        return Some((bar0_hi << 32) | phys_lo);
                    } else {
                        return Some(phys_lo);
                    }
                }
            }
        }
    }
    None
}
