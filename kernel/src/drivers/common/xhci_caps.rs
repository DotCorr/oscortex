//! xHCI capability register parsing (pure, no MMIO).

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct XhciCapability {
    pub cap_length: u8,
    pub hci_version: u16,
    pub max_slots: u8,
    pub max_ports: u8,
    pub usbcmd_off: usize,
    pub usbsts_off: usize,
}

/// Parse CAP + HCSPARAMS1 values read from mapped xHCI MMIO.
pub fn parse_capability(cap: u32, hcsparams1: u32) -> Result<XhciCapability, &'static str> {
    let cap_length = (cap & 0xFF) as u8;
    if cap_length == 0 {
        return Err("invalid CAPLENGTH");
    }
    let hci_version = ((cap >> 16) & 0xFFFF) as u16;
    let max_slots = (hcsparams1 & 0xFF) as u8;
    let max_ports = ((hcsparams1 >> 24) & 0xFF) as u8;
    Ok(XhciCapability {
        cap_length,
        hci_version,
        max_slots,
        max_ports,
        usbcmd_off: cap_length as usize,
        usbsts_off: cap_length as usize + 4,
    })
}

pub const USBCMD_HCRST: u32 = 1 << 1;
pub const USBSTS_HCH: u32 = 1 << 12;

/// True when controller reset bit has cleared.
pub fn reset_complete(usbcmd: u32) -> bool {
    usbcmd & USBCMD_HCRST == 0
}

/// True when host controller is halted after reset (expected idle state).
pub fn halted_after_reset(usbsts: u32) -> bool {
    usbsts & USBSTS_HCH != 0
}
