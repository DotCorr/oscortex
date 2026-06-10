//! Exception-vector scaffolding — aarch64 (EL1 vector table / VBAR_EL1).
//!
//! AArch64 has no IDT; synchronous/IRQ/FIQ/SError exceptions are dispatched
//! through a 16-entry, 2 KiB-aligned vector table pointed to by VBAR_EL1. This
//! module names the table-install entry points the way the x86 `idt` module
//! does (`init`/`load`) and re-exports the cross-arch [`InterruptFrame`] saved
//! by the (future) vector handlers.

pub use super::InterruptFrame;

/// Install the EL1 exception vector table on the BSP.
///
/// TODO(arm): build the vector table and write its base to VBAR_EL1.
pub fn init() {}

/// Re-load the (already-built) vector table on an AP.
///
/// TODO(arm): write the shared vector-table base to this core's VBAR_EL1.
pub fn load() {}
