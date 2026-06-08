//! CPU feature detection and setup — aarch64.
//!
//! Scaffolding stubs for the ARM port. Real implementations (FPU/SIMD enable
//! via CPACR_EL1, TLS via TPIDR_EL0, FP/SIMD context save/restore) come later.

use core::arch::asm;

/// Assert that this CPU supports all features OSCortex requires.
///
/// On aarch64 the AArch64 base profile already mandates FP+AdvSIMD, so this is
/// currently a no-op placeholder.
pub fn assert_required_features() {}

/// Enable FP / AdvSIMD (and later SVE) for the AI inference engine.
///
/// TODO(arm): clear CPACR_EL1.FPEN trap bits so EL0/EL1 may use FP/SIMD.
pub fn enable_fpu_simd() {}

/// SYSCALL path init — see syscall module. No-op on aarch64 (SVC is always on).
pub fn enable_syscall() {}

/// Returns true when running under a hypervisor.
///
/// TODO(arm): inspect MIDR_EL1 / device-tree. Conservatively returns true since
/// the bring-up target is QEMU `virt`.
pub fn running_under_hypervisor() -> bool {
    true
}

/// True when running on a QEMU-like hypervisor.
///
/// TODO(arm): real detection. The bring-up target is QEMU `virt`, so assume true.
pub fn is_qemu_like_hypervisor() -> bool {
    true
}

/// Set userspace TLS base for the current CPU context (x86 FS-base analogue).
///
/// On aarch64 user TLS lives in `TPIDR_EL0`.
pub fn set_fs_base(fs_base: u64) {
    unsafe { asm!("msr tpidr_el0, {}", in(reg) fs_base, options(nomem, nostack)) };
}

/// Read userspace TLS base for the current CPU context.
pub fn get_fs_base() -> u64 {
    let v: u64;
    unsafe { asm!("mrs {}, tpidr_el0", out(reg) v, options(nomem, nostack)) };
    v
}

/// Save the current FP/SIMD (and later SVE) register state to `ptr`.
///
/// # Safety
/// `ptr` must point to a sufficiently large, suitably aligned buffer.
///
/// TODO(arm): implement via `st1 {v0-v31}` + FPSR/FPCR. Currently a no-op.
#[inline]
pub unsafe fn save_xstate_to(_ptr: *mut u8) {}

/// Restore the FP/SIMD (and later SVE) register state from `ptr`.
///
/// # Safety
/// `ptr` must point to a buffer previously written by [`save_xstate_to`].
///
/// TODO(arm): implement via `ld1 {v0-v31}` + FPSR/FPCR. Currently a no-op.
#[inline]
pub unsafe fn restore_xstate_from(_ptr: *const u8) {}

/// Full memory fence (Data Memory Barrier, system scope).
#[inline(always)]
pub fn memory_fence() {
    unsafe { asm!("dmb sy", options(nomem, nostack)) };
}

/// Hint the CPU during poll loops.
#[inline(always)]
pub fn spin_pause() {
    unsafe { asm!("yield", options(nomem, nostack, preserves_flags)) };
}

/// Save DAIF (interrupt mask) state and disable IRQ/FIQ; returns the prior DAIF.
#[inline(always)]
pub fn interrupts_save_and_disable() -> u64 {
    let daif: u64;
    unsafe {
        asm!("mrs {}, daif", out(reg) daif, options(nomem, nostack));
        asm!("msr daifset, #0b0011", options(nomem, nostack)); // mask IRQ+FIQ
    }
    daif
}

/// Restore interrupt-enable state from [`interrupts_save_and_disable`].
#[inline(always)]
pub fn interrupts_restore(daif: u64) {
    // DAIF.I is bit 7. If it was clear (IRQs were enabled), re-enable them.
    if daif & (1 << 7) == 0 {
        unsafe { asm!("msr daifclr, #0b0010", options(nomem, nostack)) };
    }
}
