//! Minimal ACPI table lookup — RSDP → RSDT/XSDT → MADT physical address.
//!
//! We use the Limine ACPI base address request so we never have to scan
//! memory ranges ourselves.  All table pointers are physical; callers must
//! add the HHDM offset to dereference them.

// Limine ACPI RSDP request.
static ACPI_REQUEST: limine::request::RsdpRequest = limine::request::RsdpRequest::new();

/// Return the physical address of the RSDP, or 0 if Limine did not provide it.
pub fn rsdp_address() -> u64 {
    let hhdm = crate::mm::frame_allocator::hhdm_offset();
    match ACPI_REQUEST.response() {
        Some(resp) => {
            let virt = resp.address as u64;
            if virt >= hhdm { virt - hhdm } else { virt }
        }
        None => 0,
    }
}

/// Walk RSDT or XSDT (detected by RSDP revision) to find the MADT.
/// Returns the physical address of the MADT, or 0 if not found.
pub fn find_madt(rsdp_phys: u64) -> u64 {
    let hhdm = crate::mm::frame_allocator::hhdm_offset();

    // RSDP structure:
    //   +0  signature[8]   "RSD PTR "
    //   +8  checksum
    //   +9  OEM ID[6]
    //   +15 revision
    //   +16 rsdt_address (u32)
    //   +20 length        (v2+)
    //   +24 xsdt_address  (u64, v2+)
    let rsdp = (hhdm + rsdp_phys) as *const u8;
    let revision = unsafe { *rsdp.add(15) };

    if revision >= 2 {
        // Use XSDT (64-bit pointers).
        let xsdt_phys = unsafe { core::ptr::read_unaligned(rsdp.add(24) as *const u64) };
        find_table_in_xsdt(xsdt_phys, hhdm, b"APIC")
    } else {
        // Use RSDT (32-bit pointers).
        let rsdt_phys = unsafe { core::ptr::read_unaligned(rsdp.add(16) as *const u32) } as u64;
        find_table_in_rsdt(rsdt_phys, hhdm, b"APIC")
    }
}

fn find_table_in_rsdt(rsdt_phys: u64, hhdm: u64, sig: &[u8; 4]) -> u64 {
    let base = (hhdm + rsdt_phys) as *const u8;
    // SDT header length at offset 4 (u32)
    let length = unsafe { core::ptr::read_unaligned(base.add(4) as *const u32) } as usize;
    if length < 36 { return 0; }
    let entries = (length - 36) / 4;
    for i in 0..entries {
        let phys = unsafe {
            core::ptr::read_unaligned(base.add(36 + i * 4) as *const u32)
        } as u64;
        if phys == 0 { continue; }
        let table = (hhdm + phys) as *const u8;
        let s = unsafe { core::slice::from_raw_parts(table, 4) };
        if s == sig { return phys; }
    }
    0
}

fn find_table_in_xsdt(xsdt_phys: u64, hhdm: u64, sig: &[u8; 4]) -> u64 {
    let base = (hhdm + xsdt_phys) as *const u8;
    let length = unsafe { core::ptr::read_unaligned(base.add(4) as *const u32) } as usize;
    if length < 36 { return 0; }
    let entries = (length - 36) / 8;
    for i in 0..entries {
        let phys = unsafe {
            core::ptr::read_unaligned(base.add(36 + i * 8) as *const u64)
        };
        if phys == 0 { continue; }
        let table = (hhdm + phys) as *const u8;
        let s = unsafe { core::slice::from_raw_parts(table, 4) };
        if s == sig { return phys; }
    }
    0
}

// ── ACPI S5 shutdown ──────────────────────────────────────────────────────────

/// Locate the FADT via RSDP/XSDT and issue an ACPI S5 soft-off.
///
/// Falls back to the QEMU/KVM "magic" I/O port (0x604, value 0x2000) if
/// the FADT lookup fails so that QEMU exits cleanly even without a full
/// ACPI implementation.
pub fn shutdown() -> ! {
    log::info!("[ACPI] Initiating S5 power-off…");

    let hhdm = crate::mm::frame_allocator::hhdm_offset();
    let rsdp_phys = rsdp_address();

    if rsdp_phys != 0 {
        if let Some(pm1a_cnt_blk) = fadt_pm1a_cnt(rsdp_phys, hhdm) {
            // SLP_TYP for S5 is 7 (standard, from \_S5 object).
            // SLP_EN = bit 13.  PM1a_CNT write triggers sleep.
            let slp_val: u16 = (7 << 10) | (1 << 13);
            unsafe { core::arch::asm!("out dx, ax", in("dx") pm1a_cnt_blk as u16, in("ax") slp_val, options(nomem, nostack)); }
        }
    }

    // Fallback: QEMU ISA debug exit / Bochs shutdown port.
    unsafe {
        // QEMU -device isa-debug-exit default port 0x501 triggers exit.
        core::arch::asm!("out 0xf4, al", in("al") 0u8, options(nomem, nostack));
        // Bochs / older QEMU shutdown port.
        core::arch::asm!("out dx, ax", in("dx") 0x604u16, in("ax") 0x2000u16, options(nomem, nostack));
        // VirtualBox ACPI power-off
        core::arch::asm!("out dx, ax", in("dx") 0x4004u16, in("ax") 0x3400u16, options(nomem, nostack));
    }

    // If nothing worked, hang.
    loop { unsafe { core::arch::asm!("hlt", options(nomem, nostack)); } }
}

/// Walk RSDP → XSDT/RSDT → FADT and return PM1a_CNT_BLK I/O port.
fn fadt_pm1a_cnt(rsdp_phys: u64, hhdm: u64) -> Option<u64> {
    let rsdp = (hhdm + rsdp_phys) as *const u8;
    let revision = unsafe { *rsdp.add(15) };

    let fadt_phys = if revision >= 2 {
        let xsdt_phys = unsafe { core::ptr::read_unaligned(rsdp.add(24) as *const u64) };
        find_table_in_xsdt(xsdt_phys, hhdm, b"FACP")
    } else {
        let rsdt_phys = unsafe { core::ptr::read_unaligned(rsdp.add(16) as *const u32) } as u64;
        find_table_in_rsdt(rsdt_phys, hhdm, b"FACP")
    };

    if fadt_phys == 0 { return None; }

    // FADT: PM1a_CNT_BLK at offset 64 (u32 I/O port).
    let fadt = (hhdm + fadt_phys) as *const u8;
    let pm1a_cnt = unsafe { core::ptr::read_unaligned(fadt.add(64) as *const u32) } as u64;
    if pm1a_cnt == 0 { None } else { Some(pm1a_cnt) }
}

