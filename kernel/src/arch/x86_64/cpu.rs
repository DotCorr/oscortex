//! CPU feature detection and setup.

use core::arch::asm;
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

static XSAVE_ENABLED: AtomicBool = AtomicBool::new(false);
static XSAVE_OPT_ENABLED: AtomicBool = AtomicBool::new(false);
static XSAVE_MASK: AtomicU64 = AtomicU64::new(0b11); // x87 + SSE fallback

#[inline]
unsafe fn save_xstate_inner(ptr: *mut u8) {
    if XSAVE_ENABLED.load(Ordering::Acquire) {
        let mask = XSAVE_MASK.load(Ordering::Acquire);
        let eax = mask as u32;
        let edx = (mask >> 32) as u32;
        if XSAVE_OPT_ENABLED.load(Ordering::Acquire) {
            asm!(
                "xsaveopt [{buf}]",
                buf = in(reg) ptr,
                in("eax") eax,
                in("edx") edx,
                options(nostack),
            );
        } else {
            asm!(
                "xsave [{buf}]",
                buf = in(reg) ptr,
                in("eax") eax,
                in("edx") edx,
                options(nostack),
            );
        }
    } else {
        asm!(
            "fxsave64 [{buf}]",
            buf = in(reg) ptr,
            options(nostack),
        );
    }
}

#[inline]
unsafe fn restore_xstate_inner(ptr: *const u8) {
    if XSAVE_ENABLED.load(Ordering::Acquire) {
        let mask = XSAVE_MASK.load(Ordering::Acquire);
        let eax = mask as u32;
        let edx = (mask >> 32) as u32;
        asm!(
            "xrstor [{buf}]",
            buf = in(reg) ptr,
            in("eax") eax,
            in("edx") edx,
            options(nostack),
        );
    } else {
        asm!(
            "fxrstor64 [{buf}]",
            buf = in(reg) ptr,
            options(nostack),
        );
    }
}

/// Assert that this CPU supports all features OSCortex requires.
/// Panics early (before heap is up) if not.
pub fn assert_required_features() {
    // Check CPUID availability (x86_64 always has it).
    // Check required leaves.
    let (_, _, _ecx, edx) = cpuid(1, 0);
    assert!(edx & (1 << 25) != 0, "SSE required");
    assert!(edx & (1 << 26) != 0, "SSE2 required");
    // SSE3/4.x are desired but not strictly required for boot.
}

/// Enable x87 FPU, SSE, and AVX for the AI inference engine.
pub fn enable_fpu_simd() {
    unsafe {
        // Enable FPU.
        let mut cr0: u64;
        asm!("mov {}, cr0", out(reg) cr0);
        cr0 &= !(1 << 2); // clear EM
        cr0 |= 1 << 1;    // set MP
        asm!("mov cr0, {}", in(reg) cr0);

        // Enable SSE.
        let mut cr4: u64;
        asm!("mov {}, cr4", out(reg) cr4);
        cr4 |= (1 << 9) | (1 << 10); // OSFXSR | OSXMMEXCPT

        // Enable XSAVE(+AVX when present) via CR4.OSXSAVE + XCR0.
        let (eax1, _, ecx1, _) = cpuid(1, 0);
        let has_xsave = (ecx1 & (1 << 26)) != 0;
        let has_avx = (ecx1 & (1 << 28)) != 0;

        if has_xsave {
            cr4 |= 1 << 18; // OSXSAVE
            asm!("mov cr4, {}", in(reg) cr4);

            let mut xcr0_lo: u32;
            let mut xcr0_hi: u32;
            asm!(
                "xgetbv",
                in("ecx") 0u32,
                out("eax") xcr0_lo,
                out("edx") xcr0_hi,
                options(nostack, preserves_flags),
            );
            let mut xcr0 = ((xcr0_hi as u64) << 32) | xcr0_lo as u64;
            // Always keep x87+SSE enabled for kernel/user context state.
            xcr0 |= 0b11;
            if has_avx {
                xcr0 |= 1 << 2;
            }
            asm!(
                "xsetbv",
                in("ecx") 0u32,
                in("eax") xcr0 as u32,
                in("edx") (xcr0 >> 32) as u32,
                options(nostack),
            );

            let (xsave_feat, _, _, _) = cpuid(0xD, 1);
            // Use plain `xsave`/`xrstor` (NOT `xsaveopt`) with the full xcr0
            // mask (x87+SSE+AVX). fxsave64 only preserves x87+SSE (XMM0-15);
            // it drops the upper YMM halves, so any cooperative context switch
            // mid-AVX corrupts Skia/Dart SIMD state and livelocks engine init.
            // `xsaveopt` is avoided because its XINUSE modified-optimization is
            // mistracked by QEMU TCG and can record XSTATE_BV[SSE]=0; plain
            // `xsave` always writes the requested components.
            let _xsave_opt_capable = (xsave_feat & 1) != 0;
            XSAVE_ENABLED.store(true, Ordering::Release);
            XSAVE_OPT_ENABLED.store(false, Ordering::Release);
            XSAVE_MASK.store(xcr0, Ordering::Release);
        } else {
            // Keep OSFXSR/OSXMMEXCPT for FXSAVE/FXRSTOR fallback path.
            asm!("mov cr4, {}", in(reg) cr4);
            XSAVE_ENABLED.store(false, Ordering::Release);
            XSAVE_OPT_ENABLED.store(false, Ordering::Release);
            XSAVE_MASK.store(0b11, Ordering::Release);
        }

        let _ = eax1;
    }
}

/// SYSCALL/SYSRET path — see syscall module.
pub fn enable_syscall() {}

/// Execute CPUID instruction.
pub fn cpuid(leaf: u32, subleaf: u32) -> (u32, u32, u32, u32) {
    let (eax, ebx, ecx, edx): (u32, u32, u32, u32);
    unsafe {
        asm!(
            "push rbx",
            "cpuid",
            "mov {ebx_out:e}, ebx",
            "pop rbx",
            inout("eax") leaf => eax,
            inout("ecx") subleaf => ecx,
            ebx_out = out(reg) ebx,
            out("edx") edx,
        );
    }
    (eax, ebx, ecx, edx)
}

/// Returns true when CPUID reports a hypervisor-present environment.
pub fn running_under_hypervisor() -> bool {
    let (_, _, ecx, _) = cpuid(1, 0);
    (ecx & (1 << 31)) != 0
}

/// Best-effort hypervisor vendor string from CPUID leaf 0x4000_0000.
///
/// Returns `None` on bare metal.
pub fn hypervisor_vendor() -> Option<[u8; 12]> {
    if !running_under_hypervisor() {
        return None;
    }
    let (_, ebx, ecx, edx) = cpuid(0x4000_0000, 0);
    let mut out = [0u8; 12];
    out[0..4].copy_from_slice(&ebx.to_le_bytes());
    out[4..8].copy_from_slice(&ecx.to_le_bytes());
    out[8..12].copy_from_slice(&edx.to_le_bytes());
    Some(out)
}

/// True when running on common QEMU-backed hypervisors.
pub fn is_qemu_like_hypervisor() -> bool {
    let Some(v) = hypervisor_vendor() else {
        return false;
    };
    // QEMU can expose itself as TCG or as KVM depending on accelerator.
    &v == b"TCGTCGTCGTCG" || &v == b"KVMKVMKVM\0\0\0"
}

/// Set userspace FS base (TLS pointer) for the current CPU context.
pub fn set_fs_base(fs_base: u64) {
    const MSR_FS_BASE: u32 = 0xC000_0100;
    unsafe {
        asm!(
            "wrmsr",
            in("ecx") MSR_FS_BASE,
            in("eax") fs_base as u32,
            in("edx") (fs_base >> 32) as u32,
            options(nostack),
        );
    }
}

/// Read userspace FS base (TLS pointer) for the current CPU context.
pub fn get_fs_base() -> u64 {
    const MSR_FS_BASE: u32 = 0xC000_0100;
    let lo: u32;
    let hi: u32;
    unsafe {
        asm!(
            "rdmsr",
            in("ecx") MSR_FS_BASE,
            out("eax") lo,
            out("edx") hi,
            options(nostack),
        );
    }
    ((hi as u64) << 32) | (lo as u64)
}

// NOTE: the former save_preempt_xstate/restore_preempt_xstate (and the single
// global PREEMPT_XSTATE scratch buffer) were dead code with no callers. They were
// removed: a single shared FPU/SSE buffer would have been an SMP hazard, and
// preemption already saves per-process xstate via save_xstate(pid). Per-process
// xstate buffers are the only correct design here.

#[inline]
pub unsafe fn save_xstate_to(ptr: *mut u8) {
    save_xstate_inner(ptr);
}

#[inline]
pub unsafe fn restore_xstate_from(ptr: *const u8) {
    restore_xstate_inner(ptr);
}

/// Full memory fence for virtio/NVMe queue publishing.
#[inline(always)]
pub fn memory_fence() {
    unsafe {
        core::arch::asm!("mfence", options(nostack, nomem));
    }
}

/// Hint the CPU during poll loops.
#[inline(always)]
pub fn spin_pause() {
    unsafe {
        core::arch::asm!("pause", options(nomem, nostack, preserves_flags));
    }
}

/// Save RFLAGS and disable interrupts; returns RFLAGS for [`interrupts_restore`].
#[inline(always)]
pub fn interrupts_save_and_disable() -> u64 {
    let rflags: u64;
    unsafe {
        core::arch::asm!(
            "pushfq",
            "pop {}",
            out(reg) rflags,
            options(nomem, preserves_flags)
        );
        core::arch::asm!("cli", options(nomem, nostack));
    }
    rflags
}

/// Restore interrupt enable state from [`interrupts_save_and_disable`].
#[inline(always)]
pub fn interrupts_restore(rflags: u64) {
    if rflags & 0x200 != 0 {
        unsafe {
            core::arch::asm!("sti", options(nomem, nostack));
        }
    }
}

