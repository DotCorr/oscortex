//! Interrupt-controller + tick/vsync scaffolding — aarch64 (GIC + generic timer).
//!
//! On x86 this is the local APIC; on aarch64 the equivalent responsibilities are
//! split between the GIC (CPU interface: EOI, IPIs, routing) and the ARM generic
//! timer (CNTP/CNTV, the periodic tick that drives the vsync cadence). These are
//! compilable stubs preserving the public surface the shared kernel calls; real
//! GICv2/GICv3 + CNTV_CTL programming comes later.

use core::sync::atomic::{AtomicU32, Ordering};

/// Approximate timer ticks per millisecond (placeholder; real value is read
/// from `CNTFRQ_EL0` once the generic timer is programmed).
pub static APIC_TICKS_PER_MS: AtomicU32 = AtomicU32::new(62500);

/// Requested vsync cadence in Hz (0 = off).
static VSYNC_HZ: AtomicU32 = AtomicU32::new(0);

/// Initialise the interrupt controller + timer on the BSP.
///
/// TODO(arm): probe + init the GIC distributor/CPU interface and program the
/// generic timer (CNTV_TVAL_EL0 / CNTV_CTL_EL0).
pub fn init_bsp() {}

/// Finish any deferred MMIO mapping after `mm::init()`.
///
/// On x86 this maps the xAPIC MMIO window; on aarch64 the GIC MMIO regions would
/// be mapped here. No-op stub.
pub fn finish_xapic_init() {}

/// Per-AP interrupt-controller init.
///
/// TODO(arm): enable this core's GIC redistributor/CPU interface.
pub fn init_ap() {}

/// Signal End-Of-Interrupt to the controller.
///
/// TODO(arm): write the interrupt id to GICC_EOIR / ICC_EOIR1_EL1.
pub fn eoi() {}

/// Route the legacy serial/ExtINT line.
///
/// x86-specific (LINT0 in ExtINT mode). No aarch64 equivalent — no-op.
pub fn configure_lint0_for_extint() {}

/// Return this core's interrupt-controller id (x86 LAPIC-id analogue).
///
/// Derived from MPIDR_EL1 affinity bits.
pub fn local_apic_id() -> u32 {
    let v: u64;
    unsafe { core::arch::asm!("mrs {}, mpidr_el1", out(reg) v, options(nomem, nostack)) };
    (v & 0x00FF_FFFF) as u32
}

/// Send a reschedule IPI to the given target core.
///
/// TODO(arm): raise a GIC Software-Generated Interrupt (SGI) to `_target`.
pub fn send_resched_ipi(_target_lapic_id: u32) {}

// ── Vsync cadence ───────────────────────────────────────────────────────────

/// Set the desired vsync cadence (Hz). 0 disables it.
pub fn set_vsync_hz(hz: u32) {
    VSYNC_HZ.store(hz, Ordering::Release);
}

/// Returns true when a vsync interval has elapsed since the last reset.
///
/// TODO(arm): compare CNTVCT_EL0 against the last fired timestamp scaled by
/// CNTFRQ_EL0 / VSYNC_HZ. Always returns false in the scaffold.
pub fn vsync_due() -> bool {
    false
}

/// Reset the vsync interval reference timestamp.
///
/// TODO(arm): snapshot CNTVCT_EL0. No-op stub.
pub fn reset_vsync_last_tsc() {}
