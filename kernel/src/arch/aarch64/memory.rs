//! aarch64 memory helpers (translation-table base, etc.)
//!
//! The x86 `read_cr3`/`write_cr3` map to the EL1 translation-table base
//! registers `TTBR0_EL1` (user/low half) and `TTBR1_EL1` (kernel/high half).
//! The shared kernel only needs a single "current address-space root" value;
//! we expose `TTBR0_EL1` under the historical `read_cr3` name.

use core::arch::asm;

/// Read the current low-half translation-table base (x86 CR3 analogue).
pub fn read_cr3() -> u64 {
    let v: u64;
    unsafe { asm!("mrs {}, ttbr0_el1", out(reg) v, options(nomem, nostack)) };
    v
}

/// Write the low-half translation-table base and synchronise the TLB.
pub fn write_cr3(val: u64) {
    unsafe {
        asm!(
            "msr ttbr0_el1, {}",
            "dsb ish",
            "tlbi vmalle1is",
            "dsb ish",
            "isb",
            in(reg) val,
            options(nostack),
        );
    }
}
