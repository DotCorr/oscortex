//! aarch64 user-mode entry transitions (EL1 → EL0 via ERET).
//!
//! Mirrors the x86_64 `enter_user` hooks so the shared `process` layer can
//! transfer into ring-3 (EL0) architecture-neutrally. On aarch64 both the IRET
//! and SYSRET variants collapse to an `ERET` after restoring the GPRs from
//! `regs` and setting ELR_EL1 (user PC), SP_EL0 (user SP) and SPSR_EL1 (EL0t,
//! interrupts unmasked).
//!
//! The shared `EnterUserRegs` uses x86 field names; on aarch64 we map them onto
//! AArch64 registers so a userspace built for the Linux aarch64 ABI sees its
//! arguments in the right places:
//!
//!   rip → ELR_EL1 (user PC)      rsp → SP_EL0 (user SP)
//!   rdi → x0   rsi → x1   rdx → x2   rcx → x3   r8 → x4   r9 → x5
//!   rax → x8  (syscall-nr / return register, matching the Linux ABI)
//!   rbx → x19  rbp → x29  r10 → x9  r11 → x10
//!   r12 → x20  r13 → x21  r14 → x22  r15 → x23
//!
//! The exact mapping only matters for the *first* entry of a fresh process
//! (where the shared layer seeds rip/rsp/args); on a syscall-return re-entry the
//! SVC handler has already written the full user frame, so the values flow
//! straight back.

/// Full user register state required to enter EL0. Field names mirror the
/// x86_64 backend so shared code stays architecture-neutral.
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
    /// aarch64 callee-saved x24–x28 (no x86 equivalent). Preserved across the
    /// cooperative/re-exec resume so the Dart VM's live pointers in these
    /// registers survive a blocking-syscall yield.
    pub x24: u64,
    pub x25: u64,
    pub x26: u64,
    pub x27: u64,
    pub x28: u64,
}

/// SPSR_EL1 value for "return to EL0t (SP_EL0), all interrupts unmasked".
///
///   M[3:0] = 0b0000 → EL0t
///   D,A,I,F = 0      → debug/SError/IRQ/FIQ all unmasked in EL0
const SPSR_EL0T: u64 = 0;

/// Build the 34-entry image the asm loads from: x0..x30, SP_EL0, PC, SPSR.
#[inline(always)]
fn build_image(regs: &EnterUserRegs) -> [u64; 34] {
    let mut img = [0u64; 34];
    img[0] = regs.rdi; // x0
    img[1] = regs.rsi; // x1
    img[2] = regs.rdx; // x2
    img[3] = regs.rcx; // x3
    img[4] = regs.r8; // x4
    img[5] = regs.r9; // x5
    // x6, x7 unused by the seed path — leave zero.
    img[8] = regs.rax; // x8 (syscall nr / return)
    img[9] = regs.r10; // x9
    img[10] = regs.r11; // x10
    // x11..x18 zero.
    img[19] = regs.rbx; // x19 (callee-saved)
    img[20] = regs.r12; // x20
    img[21] = regs.r13; // x21
    img[22] = regs.r14; // x22
    img[23] = regs.r15; // x23
    img[24] = regs.x24; // x24 (callee-saved)
    img[25] = regs.x25; // x25
    img[26] = regs.x26; // x26
    img[27] = regs.x27; // x27
    img[28] = regs.x28; // x28
    img[29] = regs.rbp; // x29 (frame pointer)
    // x30 (LR): the shared enter path stashes the user link register captured at
    // the SVC boundary in the otherwise-unused `rflags` slot (SPSR is a constant
    // here). A thread that yields mid-syscall MUST resume with x30 intact or its
    // next `ret` branches to whatever was here (was hardwired 0 → crash). On the
    // very first entry of a fresh thread user_lr is 0, which is correct (the
    // thread-return trampoline address sits at [SP], not in LR).
    img[30] = regs.rflags; // x30 (LR)
    img[31] = regs.rsp; // SP_EL0
    img[32] = regs.rip; // PC → ELR_EL1
    img[33] = SPSR_EL0T; // SPSR_EL1
    img
}

/// Restore the full user GPR file from `img`, set SP_EL0 / ELR_EL1 / SPSR_EL1,
/// and `eret` to EL0. Never returns.
///
/// # Safety
/// TTBR0_EL1 must already point at the target thread's address space and `img`
/// must describe a valid EL0 context (mapped PC/SP).
#[inline(never)]
unsafe fn eret_to_el0(img: &[u64; 34]) -> ! {
    core::arch::asm!(
        // x18 = base pointer to the image. It is the last GPR we load, so it
        // can serve as the base for every prior load without clobbering a value
        // we still need.
        "mov x18, {base}",
        // SP_EL0, ELR_EL1, SPSR_EL1 first (scratch via x16).
        "ldr x16, [x18, #(31 * 8)]",  // SP_EL0
        "msr sp_el0, x16",
        "ldr x16, [x18, #(32 * 8)]",  // PC
        "msr elr_el1, x16",
        "ldr x16, [x18, #(33 * 8)]",  // SPSR
        "msr spsr_el1, x16",
        // Restore x0..x30 from the image.
        "ldp x0,  x1,  [x18, #(0  * 8)]",
        "ldp x2,  x3,  [x18, #(2  * 8)]",
        "ldp x4,  x5,  [x18, #(4  * 8)]",
        "ldp x6,  x7,  [x18, #(6  * 8)]",
        "ldp x8,  x9,  [x18, #(8  * 8)]",
        "ldp x10, x11, [x18, #(10 * 8)]",
        "ldp x12, x13, [x18, #(12 * 8)]",
        "ldp x14, x15, [x18, #(14 * 8)]",
        "ldp x19, x20, [x18, #(19 * 8)]",
        "ldp x21, x22, [x18, #(21 * 8)]",
        "ldp x23, x24, [x18, #(23 * 8)]",
        "ldp x25, x26, [x18, #(25 * 8)]",
        "ldp x27, x28, [x18, #(27 * 8)]",
        "ldr x29,      [x18, #(29 * 8)]",
        "ldr x30,      [x18, #(30 * 8)]",
        // x16, x17 last among the temporaries (restore real values).
        "ldr x17,      [x18, #(17 * 8)]",
        "ldr x16,      [x18, #(16 * 8)]",
        // x18 itself last (base stays valid through every prior load).
        "ldr x18,      [x18, #(18 * 8)]",
        "eret",
        base = in(reg) img.as_ptr(),
        options(noreturn),
    )
}

/// ERET entry mirroring the x86_64 IRETQ path (timer-preempted re-entry).
///
/// # Safety
/// TTBR0_EL1 must already point at the target thread's address space.
#[inline(never)]
pub unsafe fn enter_user_iret(regs: &EnterUserRegs) -> ! {
    let img = build_image(regs);
    eret_to_el0(&img)
}

/// ERET entry mirroring the x86_64 SYSRETQ path (syscall-yield re-entry).
///
/// # Safety
/// TTBR0_EL1 must already point at the target thread's address space.
#[inline(never)]
pub unsafe fn enter_user_sysret(regs: &EnterUserRegs) -> ! {
    let img = build_image(regs);
    eret_to_el0(&img)
}

/// Resume a thread from its full saved trap-frame snapshot — the 36-word image
/// the timer-preempt path stores in `Process::arch_trapframe`.
///
/// Unlike [`enter_user_iret`]/[`enter_user_sysret`] (which rebuild the context
/// from the lossy x86-named `EnterUserRegs` and therefore zero x6/x7/x11..x18
/// and can carry stale x24..x28), this restores EVERY register exactly as
/// captured at the preemption point. It MUST be used whenever a timer-preempted
/// thread is resumed through the cooperative handoff path: a Dart worker
/// preempted mid heap-allocation keeps live pointers in caller-saved temporaries
/// that the `build_image` path would silently drop → near-null deref.
///
/// Frame layout (matches `apic.rs::frame_as_array`):
///   `[0..31]=x0..x30, 31=SP_EL0, 32=ELR(PC), 33=SPSR, 34=ESR, 35=pad`.
/// The first 34 words are exactly the `img` layout [`eret_to_el0`] consumes.
///
/// # Safety
/// TTBR0_EL1 must already point at the target thread's address space and the
/// frame must describe a valid EL0 context.
#[inline(never)]
pub unsafe fn enter_user_from_frame(frame: &[u64; 36]) -> ! {
    let mut img = [0u64; 34];
    img.copy_from_slice(&frame[..34]);
    eret_to_el0(&img)
}

/// Directly enter EL0 at a given PC/SP with seeded x0..x2 (bring-up helper).
///
/// The minimal path the aarch64 bring-up uses to launch a self-contained EL0
/// test program without the full shared `process` layer.
///
/// # Safety
/// `pc`/`sp` must be mapped, executable/writable EL0 addresses under the active
/// TTBR0_EL1.
#[inline(never)]
pub unsafe fn enter_el0_at(pc: u64, sp: u64, x0: u64, x1: u64, x2: u64) -> ! {
    let mut img = [0u64; 34];
    img[0] = x0;
    img[1] = x1;
    img[2] = x2;
    img[31] = sp;
    img[32] = pc;
    img[33] = SPSR_EL0T;
    eret_to_el0(&img)
}
