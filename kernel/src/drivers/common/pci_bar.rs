//! PCI config-space decoding helpers (no port I/O).

/// Decode BAR0 low/high dwords into a memory-mapped physical base.
/// Returns `None` for I/O-space BARs or invalid types.
pub fn decode_bar0_mmio(bar0_lo: u32, bar0_hi: u32) -> Option<u64> {
    if bar0_lo & 1 != 0 {
        return None;
    }
    let bar0_type = (bar0_lo >> 1) & 0x3;
    let phys_lo = (bar0_lo & !0xF) as u64;
    match bar0_type {
        0x0 => Some(phys_lo),
        0x2 => Some(((bar0_hi as u64) << 32) | phys_lo),
        _ => None,
    }
}

/// Decode BAR0 as a legacy I/O port base (bit 0 set).
pub fn decode_bar0_io(bar0_lo: u32) -> Option<u16> {
    if bar0_lo & 1 == 0 {
        return None;
    }
    Some((bar0_lo & 0xFFFC) as u16)
}

/// Extract class / subclass / prog-if from the PCI class-code config dword.
pub fn pci_class_fields(class_reg: u32) -> (u8, u8, u8) {
    (
        ((class_reg >> 24) & 0xFF) as u8,
        ((class_reg >> 16) & 0xFF) as u8,
        ((class_reg >> 8) & 0xFF) as u8,
    )
}

/// True when the class-code dword matches the given PCI class tuple.
pub fn pci_class_matches(class_reg: u32, class: u8, subclass: u8, prog_if: u8) -> bool {
    let (c, sc, pi) = pci_class_fields(class_reg);
    c == class && sc == subclass && pi == prog_if
}

/// XHCI PCI class tuple (0x0C / 0x03 / 0x30).
pub fn is_xhci(class_reg: u32) -> bool {
    pci_class_matches(class_reg, 0x0C, 0x03, 0x30)
}

/// NVMe PCI class tuple (0x01 / 0x08 / 0x02).
pub fn is_nvme(class_reg: u32) -> bool {
    pci_class_matches(class_reg, 0x01, 0x08, 0x02)
}
