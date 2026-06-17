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
    // aarch64-only: the user link register (x30) at SVC entry. AArch64 has no
    // on-stack return address — a function that yields inside a syscall must
    // have its LR restored, or its later `ret` branches to whatever build_image
    // leaves in x30 (previously hardwired to 0 → branch-to-0 crash).
    lr: u64,
    // aarch64-only callee-saved regs with no x86 equivalent (x86 has only r8–r15).
    x24: u64, x25: u64, x26: u64, x27: u64, x28: u64,
}

const ZERO_SCRATCH: CpuScratch = CpuScratch {
    user_rsp: 0, user_rip: 0, syscall_stack_top: 0,
    rdi: 0, rsi: 0, rdx: 0, r10: 0, r8: 0, r9: 0,
    rbx: 0, rbp: 0, r12: 0, r13: 0, r14: 0, r15: 0,
    lr: 0,
    x24: 0, x25: 0, x26: 0, x27: 0, x28: 0,
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
    /// aarch64 user link register (x30) at SVC entry. Unused on x86.
    pub lr: u64,
    /// aarch64 callee-saved registers with no x86 equivalent (x86 has only
    /// r8–r15). The Dart VM/engine keep live pointers here across blocking
    /// syscalls; without preserving them a cooperative/re-exec resume zeroes
    /// them → near-null deref. Unused on x86.
    pub x24: u64, pub x25: u64, pub x26: u64, pub x27: u64, pub x28: u64,
}

#[inline]
fn cpu_idx() -> usize {
    crate::arch::aarch64::smp::current_cpu_id() as usize
}

/// Capture the user register snapshot from an EL0 SVC trap frame into this
/// CPU's scratch, mirroring the x86 syscall-entry GS snapshot. After this, the
/// shared accessors (`user_rip`, `user_rsp`, `user_gprs`, ...) return the
/// trapping thread's state, so the architecture-neutral `dispatch_fast` path can
/// read it.
///
/// AArch64→x86 field mapping (Linux aarch64 ABI): x0..x5 → rdi/rsi/rdx/r10/r8/r9
/// (the syscall arg registers), x8 → syscall number (handled by the caller),
/// SP_EL0 → user_rsp, ELR_EL1 → user_rip; callee-saved x19..x23/x29 fill the
/// rbx/r12..r15/rbp slots used by `save_full_user_gprs`.
pub fn capture_from_trap(frame: &super::vectors::TrapFrame) {
    let i = cpu_idx();
    unsafe {
        let s = &mut (*core::ptr::addr_of_mut!(CPU_SCRATCHES))[i];
        s.user_rsp = frame.sp_el0;
        s.user_rip = frame.elr;
        s.rdi = frame.x[0];
        s.rsi = frame.x[1];
        s.rdx = frame.x[2];
        s.r10 = frame.x[3];
        s.r8 = frame.x[4];
        s.r9 = frame.x[5];
        // Callee-saved mapping for cross-arch register preservation.
        s.rbx = frame.x[19];
        s.r12 = frame.x[20];
        s.r13 = frame.x[21];
        s.r14 = frame.x[22];
        s.r15 = frame.x[23];
        s.rbp = frame.x[29];
        s.lr  = frame.x[30];
        s.x24 = frame.x[24];
        s.x25 = frame.x[25];
        s.x26 = frame.x[26];
        s.x27 = frame.x[27];
        s.x28 = frame.x[28];
    }
}

/// Production SVC (EL0 synchronous) handler — routes a userspace `svc #0` into
/// the shared syscall dispatcher.
///
/// The Linux aarch64 syscall ABI puts the number in x8 and args in x0..x5; the
/// OSCortex shared dispatcher (`syscall::dispatch_fast`) takes (nr, a0..a4) and
/// returns the result in x0. We capture the trapping thread's user GPRs into the
/// per-CPU snapshot first (so the dispatcher's `user_rip`/`user_rsp`/`user_gprs`
/// accessors reflect the caller), dispatch, then write the return value back into
/// x0 of the saved frame so the vector stub's `eret` resumes EL0 with it.
///
/// The faulting instruction is `svc #0`; ELR_EL1 already points at the *next*
/// instruction on a synchronous SVC, so no manual advance is needed.
///
/// If the call terminated the process (exit / exit_group), the EL0 context is no
/// longer valid to resume, so we hand off to the next runnable user process via
/// the shared scheduler instead of returning to the vector stub.
fn production_svc_handler(f: &mut super::vectors::TrapFrame) {
    capture_from_trap(f);

    // Persist this thread's FP/SIMD file — captured into the trap frame by the
    // vector stub at SVC entry, BEFORE any kernel NEON ran — into its process
    // slot. A blocking syscall (epoll_wait/futex/cond) yields cooperatively and
    // is later resumed via enter_user_sysret/from_frame, which restores FP from
    // this slot. Without it the resumed Dart worker inherits kernel-clobbered
    // v0..v31 → nondeterministic wild-pointer faults.
    crate::process::aarch64_store_fp_from_frame(crate::process::current_pid(), f);

    let nr = f.x[8];
    let ret = crate::syscall::dispatch_fast(
        nr, f.x[0], f.x[1], f.x[2], f.x[3], f.x[4],
    );
    f.x[0] = ret as u64;

    // exit(60) / exit_group(231): the current process is now a zombie; do not
    // eret back into it. Pick the next runnable user thread and enter it.
    if nr == 60 || nr == 231 {
        let cur = crate::process::current_pid();
        match crate::process::next_runnable_pid(cur) {
            Some(next) => crate::process::enter_user_by_pid_noreturn(next),
            None => {
                // No more user work — park this core. The shared idle/cortex loop
                // keeps the kernel alive; an exiting init with nothing to run is a
                // clean stop for the bring-up.
                log::info!("[svc] init exited (code={}) — no runnable user process; parking", ret);
                loop {
                    crate::arch::halt();
                }
            }
        }
    }
}

/// Initialise the SVC fast path on the BSP.
///
/// Installs the production SVC handler into the EL1 vector table (VBAR_EL1 was
/// programmed earlier by `vectors::install`), so a userspace `svc #0` reaches
/// the shared syscall dispatcher.
pub fn init() {
    super::vectors::set_svc_handler(production_svc_handler);
    log::info!("[Syscall] aarch64 SVC entry ready (production dispatch_fast wired)");
}

/// Per-AP SVC init.
///
/// TODO(arm): point this core's VBAR_EL1 at the shared vector table.
pub fn init_ap(_cpu_idx: u32) {}

/// Active EL0 syscall-stack top for the current thread — PER-CPU. The
/// cooperative-yield resume (`enter_user::eret_to_el0`) resets SP_EL1 to this
/// before `eret`, reclaiming the EL1 exception frame it would otherwise abandon.
/// Without it SP_EL1 leaks ~one frame per yield and eventually underflows the
/// boot stack, corrupting kernel memory (the dispatch handler-ptr → 1 → PC=1
/// crash). This is the aarch64 analogue of x86's `mov rsp, gs:[24]`.
///
/// SMP: this MUST be per-CPU. It was a single global ("smp=1 bring-up: one slot")
/// — under SMP, core 0 (running e.g. the shell's pid 7) and core 1 (a launched
/// app's thread) clobbered each other's value, so a cooperative resume on one
/// core reset SP_EL1 to the OTHER core's stack → two cores sharing one EL1 stack
/// → nondeterministic kernel-stack corruption (the EC=0x24 data abort, FAR=-1,
/// in an engine thread when a 2nd engine launches). One slot per core fixes it.
pub static ACTIVE_EL1_SP: [core::sync::atomic::AtomicU64; MAX_CPUS] =
    [const { core::sync::atomic::AtomicU64::new(0) }; MAX_CPUS];

/// Read THIS CPU's active syscall-stack top (0 if not yet set).
#[inline]
pub fn active_el1_sp() -> u64 {
    ACTIVE_EL1_SP[cpu_idx()].load(Ordering::Acquire)
}

/// Set the active kernel syscall-stack top for the current CPU.
pub fn set_active_stack_top(stack_top: u64) {
    let i = cpu_idx();
    ACTIVE_STACK_TOP[i].store(stack_top, Ordering::Release);
    ACTIVE_EL1_SP[i].store(stack_top, Ordering::Release);
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
            lr: s.lr,
            x24: s.x24, x25: s.x25, x26: s.x26, x27: s.x27, x28: s.x28,
        }
    }
}

/// User link register (x30) captured at SVC entry. AArch64-only.
pub fn user_lr() -> u64 {
    unsafe { (*core::ptr::addr_of!(CPU_SCRATCHES))[cpu_idx()].lr }
}
