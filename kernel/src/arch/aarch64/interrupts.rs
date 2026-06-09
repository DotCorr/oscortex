//! Interrupt helpers — aarch64.
//!
//! x86 has a legacy 8259 PIC to disable; aarch64 has no such thing (the GIC is
//! configured in `apic`/GIC init), so `disable_pic` is a no-op kept for parity
//! with the x86 early-init sequence.

use core::arch::asm;

/// No-op on aarch64 — there is no legacy PIC to disable.
///
/// # Safety
/// Trivially safe; kept `unsafe` to match the x86 signature called from
/// `early_init`.
pub unsafe fn disable_pic() {}

/// Data Memory Barrier (system scope).
#[inline]
pub fn memory_fence() {
    unsafe { asm!("dmb sy", options(nomem, nostack)) };
}
