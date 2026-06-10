//! Firmware-table lookup scaffolding — aarch64.
//!
//! On aarch64 platforms hardware topology comes from either ACPI (server-class
//! machines) or a flattened device tree (the QEMU `virt` bring-up target). The
//! shared kernel does not call these on the non-x86 path yet; they exist for
//! parity so the real port has named entry points to fill in.

/// Physical address of the RSDP, or 0 if unavailable.
///
/// TODO(arm): use the Limine RSDP request (ACPI) or parse the DTB.
pub fn rsdp_address() -> u64 {
    0
}

/// Physical address of the MADT/GICC table, or 0 if not found.
///
/// TODO(arm): walk XSDT for the MADT, or parse `/cpus` + `intc` from the DTB.
pub fn find_madt(_rsdp_phys: u64) -> u64 {
    0
}

/// Power off the machine.
///
/// TODO(arm): issue a PSCI `SYSTEM_OFF` via SMC/HVC. For now, park.
pub fn shutdown() -> ! {
    loop {
        unsafe { core::arch::asm!("wfi", options(nomem, nostack)) };
    }
}
