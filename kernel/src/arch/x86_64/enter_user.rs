//! x86_64 user-mode entry transitions.
//!
//! These are the final, no-return transfers into ring-3.  The shared
//! `process` layer prepares the full register state, switches CR3 and
//! delivers pending errno; it then calls one of these hooks to actually
//! enter user mode.  Two variants exist:
//!
//!   * [`enter_user_iret`]   — IRETQ path, used when a thread was preempted
//!     by the APIC timer at an arbitrary user instruction.  Restores every
//!     register (including the exact RCX/R11/RFLAGS) so no architectural
//!     state is lost.
//!   * [`enter_user_sysret`] — SYSRETQ path, the native user-entry used when
//!     a thread yielded via a SYSCALL (e.g. futex_wait).  Restores the full
//!     user GPR set so callee-saved registers survive the SysV-ABI call
//!     boundary of the userspace SYSCALL trampoline.
//!
//! The asm is load-bearing for the render path; it must remain byte-for-byte
//! identical to its prior inline form in `process::mod`.

use core::arch::asm;

/// Full user register state required to enter ring-3.
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

/// IRETQ entry: restore every register and return via the hardware IRET frame.
///
/// # Safety
/// CR3 must already point at the target thread's address space, and `regs`
/// must describe a valid ring-3 context (mapped RIP/RSP, sane RFLAGS).
#[inline(never)]
pub unsafe fn enter_user_iret(regs: &EnterUserRegs) -> ! {
    // Layout of iret_frame (accessed via r11 as base register):
    //   [r11 + 0x00]  RIP   ─┐
    //   [r11 + 0x08]  CS      │ hardware IRET frame pushed onto
    //   [r11 + 0x10]  RFLAGS  │ the kernel stack by the `push` sequence
    //   [r11 + 0x18]  RSP    ─┘ (ring-3 switch includes RSP+SS)
    //   [r11 + 0x20]  SS
    //   [r11 + 0x28]  RAX .. R15  (all user GPRs, r11 loaded last)
    let iret_frame: [u64; 20] = [
        /* 0x00 */ regs.rip,
        /* 0x08 */ super::gdt::USER_CS as u64,   // 0x2B
        /* 0x10 */ regs.rflags | 0x200,          // ensure IF=1
        /* 0x18 */ regs.rsp,
        /* 0x20 */ super::gdt::USER_DS as u64,   // 0x23
        /* 0x28 */ regs.rax,
        /* 0x30 */ regs.rbx,
        /* 0x38 */ regs.rcx,   // real RCX (not SYSCALL convention)
        /* 0x40 */ regs.rdx,
        /* 0x48 */ regs.rdi,
        /* 0x50 */ regs.rsi,
        /* 0x58 */ regs.rbp,
        /* 0x60 */ regs.r8,
        /* 0x68 */ regs.r9,
        /* 0x70 */ regs.r10,
        /* 0x78 */ regs.r11,   // real R11 (not RFLAGS convention)
        /* 0x80 */ regs.r12,
        /* 0x88 */ regs.r13,
        /* 0x90 */ regs.r14,
        /* 0x98 */ regs.r15,
    ];
    asm!(
        // Push the 5-word IRET frame: SS, RSP, RFLAGS, CS, RIP.
        // These land on the current kernel stack; IRETQ will consume
        // them and switch to the user stack (RSP) automatically.
        "push qword ptr [r11 + 0x20]",  // SS
        "push qword ptr [r11 + 0x18]",  // RSP
        "push qword ptr [r11 + 0x10]",  // RFLAGS
        "push qword ptr [r11 + 0x08]",  // CS
        "push qword ptr [r11 + 0x00]",  // RIP
        // Restore all user GPRs (r11 last because it is our base ptr).
        "mov rax, [r11 + 0x28]",
        "mov rbx, [r11 + 0x30]",
        "mov rcx, [r11 + 0x38]",
        "mov rdx, [r11 + 0x40]",
        "mov rdi, [r11 + 0x48]",
        "mov rsi, [r11 + 0x50]",
        "mov rbp, [r11 + 0x58]",
        "mov r8,  [r11 + 0x60]",
        "mov r9,  [r11 + 0x68]",
        "mov r10, [r11 + 0x70]",
        "mov r12, [r11 + 0x80]",
        "mov r13, [r11 + 0x88]",
        "mov r14, [r11 + 0x90]",
        "mov r15, [r11 + 0x98]",
        "mov r11, [r11 + 0x78]",   // real R11 — must be last
        "iretq",
        in("r11") iret_frame.as_ptr(),
        options(noreturn),
    )
}

/// SYSRETQ entry: restore the full user GPR set and return to ring-3.
///
/// # Safety
/// CR3 must already point at the target thread's address space, and `regs`
/// must describe a valid ring-3 context (mapped RIP/RSP, sane RFLAGS).
#[inline(never)]
pub unsafe fn enter_user_sysret(regs: &EnterUserRegs) -> ! {
    // Stage values in a fixed-layout stack array and load them inside the asm
    // block via a single pointer. RSP and the syscall-consumed regs (RCX,
    // R11) are loaded last so all intermediate reads complete first.
    let frame: [u64; 16] = [
        /* 0x00 */ regs.rip,
        /* 0x08 */ regs.rflags,
        /* 0x10 */ regs.rsp,
        /* 0x18 */ regs.rdi,
        /* 0x20 */ regs.rsi,
        /* 0x28 */ regs.rdx,
        /* 0x30 */ regs.rax,
        /* 0x38 */ regs.rbx,
        /* 0x40 */ regs.rbp,
        /* 0x48 */ regs.r8,
        /* 0x50 */ regs.r9,
        /* 0x58 */ regs.r10,
        /* 0x60 */ regs.r12,
        /* 0x68 */ regs.r13,
        /* 0x70 */ regs.r14,
        /* 0x78 */ regs.r15,
    ];
    asm!(
        // Pin the frame base in r11. r11 is consumed by SYSRET (as user
        // RFLAGS) and is the LAST register we load — every other load
        // can therefore use [r11 + disp] safely. If we let the compiler
        // pick the base register, it can alias the base with the first
        // destination (e.g. rax) and have the very first `mov` clobber
        // the pointer before any further reads happen.
        "mov rax, [r11 + 0x30]",
        "mov rdi, [r11 + 0x18]",
        "mov rsi, [r11 + 0x20]",
        "mov rdx, [r11 + 0x28]",
        "mov rbx, [r11 + 0x38]",
        "mov rbp, [r11 + 0x40]",
        "mov r8,  [r11 + 0x48]",
        "mov r9,  [r11 + 0x50]",
        "mov r10, [r11 + 0x58]",
        "mov r12, [r11 + 0x60]",
        "mov r13, [r11 + 0x68]",
        "mov r14, [r11 + 0x70]",
        "mov r15, [r11 + 0x78]",
        "mov rcx, [r11 + 0x00]",   // user RIP   → RCX (consumed by sysretq)
        "mov rsp, [r11 + 0x10]",   // switch to user stack (mapped in user PML4)
        "mov r11, [r11 + 0x08]",   // user RFL  → R11 (consumed by sysretq) — load LAST
        "sysretq",
        in("r11") frame.as_ptr(),
        options(noreturn),
    )
}
