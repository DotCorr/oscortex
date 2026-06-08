//! SVC fast-path syscall scaffolding — aarch64.
//!
//! On x86 user syscalls arrive via the `SYSCALL` instruction and a per-CPU GS
//! scratch area captures the user register snapshot at entry. On aarch64 the
//! equivalent is the `SVC` instruction trapping to the EL1 synchronous exception
//! vector, which would stash the user GPRs (x0..x30, SP_EL0, ELR/SPSR) into a
//! per-CPU scratch.
//!
//! This module preserves the public surface the shared kernel calls
//! (`set_active_stack_top`, `user_rsp`/`user_rip`/`user_r9`/`user_rbp`,
//! `user_gprs`, `init`, `init_ap`). The per-CPU snapshot capture itself (the
//! exception-vector entry) is not yet implemented, so the accessors return the
//! last value written into the scratch (zero until the vector is wired up).

use core::sync::atomic::{AtomicU64, Ordering};

const MAX_CPUS: usize = 64;

/// Per-CPU snapshot of the user register file captured at SVC entry.
///
/// Field names mirror the x86_64 [`UserGprSnapshot`] so cross-arch process code
/// (e.g. `save_full_user_gprs`) is portable. On aarch64 these map to the AAPCS64
/// argument/callee-saved registers (rip→ELR_EL1, rsp→SP_EL0, etc.).
#[repr(C)]
struct CpuScratch {
    user_rsp:        u64,
    user_rip:        u64,
    syscall_stack_top: u64,
    rdi: u64, rsi: u64, rdx: u64,
    r10: u64, r8:  u64, r9:  u64,
    rbx: u64, rbp: u64,
    r12: u64, r13: u64, r14: u64, r15: u64,
}

const ZERO_SCRATCH: CpuScratch = CpuScratch {
    user_rsp: 0, user_rip: 0, syscall_stack_top: 0,
    rdi: 0, rsi: 0, rdx: 0, r10: 0, r8: 0, r9: 0,
    rbx: 0, rbp: 0, r12: 0, r13: 0, r14: 0, r15: 0,
};

static mut CPU_SCRATCHES: [CpuScratch; MAX_CPUS] = [ZERO_SCRATCH; MAX_CPUS];

/// Active kernel syscall-stack top, per CPU (set by the scheduler before
/// returning to user). Mirrors the x86 `set_active_stack_top` behaviour.
static ACTIVE_STACK_TOP: [AtomicU64; MAX_CPUS] = [const { AtomicU64::new(0) }; MAX_CPUS];

/// Snapshot of all general-purpose user registers captured at SVC entry.
///
/// Field names are kept identical to the x86_64 backend so shared code stays
/// architecture-neutral.
#[derive(Clone, Copy, Default)]
pub struct UserGprSnapshot {
    pub rdi: u64, pub rsi: u64, pub rdx: u64,
    pub r10: u64, pub r8:  u64, pub r9:  u64,
    pub rbx: u64, pub rbp: u64,
    pub r12: u64, pub r13: u64, pub r14: u64, pub r15: u64,
}

#[inline]
fn cpu_idx() -> usize {
    crate::arch::aarch64::smp::current_cpu_id() as usize
}

/// Initialise the SVC fast path on the BSP.
///
/// TODO(arm): install the EL1 exception vector table (VBAR_EL1) whose
/// synchronous handler decodes the SVC and stashes the user GPR snapshot.
pub fn init() {
    log::info!("[Syscall] aarch64 SVC scaffold (exception vector not yet installed)");
}

/// Per-AP SVC init.
///
/// TODO(arm): point this core's VBAR_EL1 at the shared vector table.
pub fn init_ap(_cpu_idx: u32) {}

/// Set the active kernel syscall-stack top for the current CPU.
pub fn set_active_stack_top(stack_top: u64) {
    let i = cpu_idx();
    ACTIVE_STACK_TOP[i].store(stack_top, Ordering::Release);
    unsafe {
        let s = &mut (*core::ptr::addr_of_mut!(CPU_SCRATCHES))[i];
        s.syscall_stack_top = stack_top;
    }
}

/// User SP_EL0 captured at SVC entry (x86 user RSP analogue).
pub fn user_rsp() -> u64 {
    unsafe { (*core::ptr::addr_of!(CPU_SCRATCHES))[cpu_idx()].user_rsp }
}

/// User ELR_EL1 (return address) captured at SVC entry (x86 user RIP analogue).
pub fn user_rip() -> u64 {
    unsafe { (*core::ptr::addr_of!(CPU_SCRATCHES))[cpu_idx()].user_rip }
}

/// Sixth user argument register captured at SVC entry (x86 R9 analogue).
pub fn user_r9() -> u64 {
    unsafe { (*core::ptr::addr_of!(CPU_SCRATCHES))[cpu_idx()].r9 }
}

/// User frame-pointer register captured at SVC entry (x86 RBP analogue).
pub fn user_rbp() -> u64 {
    unsafe { (*core::ptr::addr_of!(CPU_SCRATCHES))[cpu_idx()].rbp }
}

/// Read the full user GPR snapshot stashed by the SVC entry vector.
pub fn user_gprs() -> UserGprSnapshot {
    unsafe {
        let s = &(*core::ptr::addr_of!(CPU_SCRATCHES))[cpu_idx()];
        UserGprSnapshot {
            rdi: s.rdi, rsi: s.rsi, rdx: s.rdx,
            r10: s.r10, r8: s.r8, r9: s.r9,
            rbx: s.rbx, rbp: s.rbp,
            r12: s.r12, r13: s.r13, r14: s.r14, r15: s.r15,
        }
    }
}
