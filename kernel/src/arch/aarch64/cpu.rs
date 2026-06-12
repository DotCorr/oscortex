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

/// Enable EL0/EL1 hardware that the Flutter engine + its libc need directly:
/// FP/AdvSIMD and unprivileged access to the architectural counters.
///
/// - `CPACR_EL1.FPEN = 0b11`: allow FP/SIMD at EL0 and EL1 without trapping. The
///   BSP also sets this in `boot.rs` before any Rust runs (codegen emits `fmov`);
///   doing it here too keeps it correct on every CPU that runs `early_init`
///   (previously a no-op stub, so an AP would have run with FP traps enabled).
/// - `CNTKCTL_EL1.EL0VCTEN | EL0PCTEN`: let EL0 read the virtual/physical counters
///   (`CNTVCT_EL0`/`CNTPCT_EL0`) directly. liboscortex_libc reads `CNTVCT_EL0`
///   inline for monotonic time (vDSO-style); without these bits that `mrs` traps
///   to EL1 (EC=0x18, "trapped system instruction") and the engine faults right
///   after scheduling its first frame.
pub fn enable_fpu_simd() {
    unsafe {
        let mut cpacr: u64;
        asm!("mrs {}, cpacr_el1", out(reg) cpacr, options(nomem, nostack));
        cpacr |= 0b11 << 20; // FPEN = 0b11
        asm!("msr cpacr_el1, {}", in(reg) cpacr, options(nomem, nostack));

        // EL0PCTEN (bit 0) + EL0VCTEN (bit 1): unprivileged counter reads.
        let mut cntkctl: u64;
        asm!("mrs {}, cntkctl_el1", out(reg) cntkctl, options(nomem, nostack));
        cntkctl |= 0b11; // EL0PCTEN | EL0VCTEN
        asm!("msr cntkctl_el1, {}", in(reg) cntkctl, options(nomem, nostack));
        asm!("isb", options(nomem, nostack));
    }
}

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

/// Save the current FP/SIMD register state (v0–v31 + FPSR + FPCR) to `ptr`.
///
/// The aarch64 `TrapFrame` saves only the GPRs (x0–x30, SP_EL0, ELR, SPSR), so
/// these two functions are the ONLY mechanism that preserves the 128-bit v
/// registers and the FP status/control words across a context switch. They were
/// no-op stubs, which silently dropped the entire FP/SIMD file on every worker
/// preemption/resume — the Dart VM/engine workers keep unboxed doubles, SIMD
/// lanes and spilled values in v0–v31 and round-trip them through `fmov x,d`, so
/// a clobbered lane fed into an address calc produced wild pointers (observed:
/// EL0 data abort FAR=0x1_0000_0000) nondeterministically. (Same class of bug as
/// the interrupts_save_and_disable no-op stub.) The per-process XState buffer is
/// 4096 bytes; this writes 512 (v0–v31) + 8 (FPSR/FPCR) = 520.
///
/// # Safety
/// `ptr` must point to a ≥520-byte, 16-byte-aligned buffer.
#[inline]
pub unsafe fn save_xstate_to(ptr: *mut u8) {
    asm!(
        "st1 {{v0.2d,  v1.2d,  v2.2d,  v3.2d}},  [{p}], #64",
        "st1 {{v4.2d,  v5.2d,  v6.2d,  v7.2d}},  [{p}], #64",
        "st1 {{v8.2d,  v9.2d,  v10.2d, v11.2d}}, [{p}], #64",
        "st1 {{v12.2d, v13.2d, v14.2d, v15.2d}}, [{p}], #64",
        "st1 {{v16.2d, v17.2d, v18.2d, v19.2d}}, [{p}], #64",
        "st1 {{v20.2d, v21.2d, v22.2d, v23.2d}}, [{p}], #64",
        "st1 {{v24.2d, v25.2d, v26.2d, v27.2d}}, [{p}], #64",
        "st1 {{v28.2d, v29.2d, v30.2d, v31.2d}}, [{p}], #64",
        "mrs {t}, fpsr",
        "str {t:w}, [{p}], #4",
        "mrs {t}, fpcr",
        "str {t:w}, [{p}]",
        p = inout(reg) ptr => _,
        t = out(reg) _,
        options(nostack),
    );
}

/// Restore the FP/SIMD register state (v0–v31 + FPSR + FPCR) from `ptr`.
///
/// # Safety
/// `ptr` must point to a buffer previously written by [`save_xstate_to`].
#[inline]
pub unsafe fn restore_xstate_from(ptr: *const u8) {
    asm!(
        "ld1 {{v0.2d,  v1.2d,  v2.2d,  v3.2d}},  [{p}], #64",
        "ld1 {{v4.2d,  v5.2d,  v6.2d,  v7.2d}},  [{p}], #64",
        "ld1 {{v8.2d,  v9.2d,  v10.2d, v11.2d}}, [{p}], #64",
        "ld1 {{v12.2d, v13.2d, v14.2d, v15.2d}}, [{p}], #64",
        "ld1 {{v16.2d, v17.2d, v18.2d, v19.2d}}, [{p}], #64",
        "ld1 {{v20.2d, v21.2d, v22.2d, v23.2d}}, [{p}], #64",
        "ld1 {{v24.2d, v25.2d, v26.2d, v27.2d}}, [{p}], #64",
        "ld1 {{v28.2d, v29.2d, v30.2d, v31.2d}}, [{p}], #64",
        "ldr {t:w}, [{p}], #4",
        "msr fpsr, {t}",
        "ldr {t:w}, [{p}]",
        "msr fpcr, {t}",
        p = inout(reg) ptr => _,
        t = out(reg) _,
        options(nostack),
    );
}

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
