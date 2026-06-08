//! aarch64 user-mode entry transitions (scaffold).
//!
//! Mirrors the x86_64 `enter_user` hooks so the shared `process` layer can
//! transfer into ring-3 (EL0) architecture-neutrally. On aarch64 both the
//! IRET and SYSRET variants collapse to an `ERET` after restoring the GPRs
//! from `regs` and setting ELR_EL1/SP_EL0/SPSR_EL1. These are stubs pending
//! the real EL1→EL0 return path; they exist so the kernel compiles for
//! `aarch64-unknown-none`.

/// Full user register state required to enter EL0.
///
/// Field names mirror the x86_64 backend so shared code stays neutral; on
/// aarch64 these map onto x0..x18 / SP_EL0 / ELR_EL1 / SPSR_EL1 in the real
/// implementation.
#[derive(Clone, Copy)]
pub struct EnterUserRegs {
    pub rip: u64,
    pub rsp: u64,
    pub rflags: u64,
    pub rax: u64,
    pub rbx: u64,
    pub rcx: u64,
    pub rdx: u64,
    pub rsi: u64,
    pub rdi: u64,
    pub rbp: u64,
    pub r8: u64,
    pub r9: u64,
    pub r10: u64,
    pub r11: u64,
    pub r12: u64,
    pub r13: u64,
    pub r14: u64,
    pub r15: u64,
}

/// ERET entry mirroring the x86_64 IRETQ path.
///
/// # Safety
/// TTBR0_EL1 must already point at the target thread's address space.
#[inline(never)]
pub unsafe fn enter_user_iret(_regs: &EnterUserRegs) -> ! {
    unimplemented!("aarch64 enter_user_iret: EL1→EL0 ERET not yet implemented")
}

/// ERET entry mirroring the x86_64 SYSRETQ path.
///
/// # Safety
/// TTBR0_EL1 must already point at the target thread's address space.
#[inline(never)]
pub unsafe fn enter_user_sysret(_regs: &EnterUserRegs) -> ! {
    unimplemented!("aarch64 enter_user_sysret: EL1→EL0 ERET not yet implemented")
}
