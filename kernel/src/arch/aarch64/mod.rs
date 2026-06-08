//! aarch64 architecture backend (scaffold).
//!
//! This is the ARM port's scaffolding layer: every module and public function
//! the shared kernel reaches through `crate::arch::*` exists here with a correct
//! signature, but most bodies are stubs (`unimplemented!()`-free no-ops or sane
//! defaults) pending real implementations. The intent is that the kernel
//! *compiles and links* for `aarch64-unknown-none`; it does NOT yet boot.
//!
//! Mapping from the x86_64 backend to aarch64 primitives:
//!   * GDT/segmentation  → none (flat); `gdt` keeps nominal selector constants.
//!   * IDT               → EL1 exception vector table (VBAR_EL1), in `idt`.
//!   * Local APIC        → GIC CPU interface + ARM generic timer, in `apic`.
//!   * SYSCALL/SYSRET    → `SVC` trap to the EL1 sync vector, in `syscall`.
//!   * CR3               → TTBR0_EL1/TTBR1_EL1, in `memory`.
//!   * SMP wake          → PSCI `CPU_ON`, reschedule via GIC SGIs, in `smp`.

pub mod gdt;
pub mod idt;
pub mod interrupts;
pub mod memory;
pub mod apic;
pub mod cpu;
pub mod syscall;
pub mod acpi;
pub mod smp;
pub mod enter_user;

// ── Bare-metal bring-up (direct `-kernel` boot) ─────────────────────────────
pub mod uart;
pub mod boot;
pub mod mmu;
pub mod vectors;
pub mod bringup;
pub mod bringup_mmu;
pub mod bringup_vectors;
pub mod bringup_irq;

pub use enter_user::{enter_user_iret, enter_user_sysret, EnterUserRegs};

use core::arch::asm;

/// Cross-arch interrupt/exception frame.
///
/// `ip` mirrors the x86_64 `InterruptFrame` field name so generic kernel code
/// (the cortex observer) stays architecture-neutral. On aarch64 these fields map
/// to the exception-syndrome registers saved by the EL1 vector handler.
#[derive(Debug)]
#[repr(C)]
pub struct InterruptFrame {
    /// Instruction pointer that faulted (= ELR_EL1).
    pub ip:   u64,
    /// Saved Program Status Register (= SPSR_EL1).
    pub spsr: u64,
    /// Exception Syndrome Register (= ESR_EL1).
    pub esr:  u64,
    /// Fault Address Register (= FAR_EL1).
    pub far:  u64,
}

/// Perform early arch initialisation (BSP path).
///
/// Mirrors the x86_64 sequence; most steps are scaffold no-ops for now.
pub fn early_init() {
    crate::logger::early_print("[ARCH] assert_required_features\r\n");
    cpu::assert_required_features();
    crate::logger::early_print("[ARCH] idt::init (VBAR_EL1)\r\n");
    idt::init();
    crate::logger::early_print("[ARCH] apic::init_bsp (GIC + timer)\r\n");
    apic::init_bsp();
    crate::logger::early_print("[ARCH] cpu::enable_fpu_simd\r\n");
    cpu::enable_fpu_simd();
    crate::logger::early_print("[ARCH] syscall::init (SVC)\r\n");
    syscall::init();
    crate::logger::early_print("[ARCH] aarch64 early_init complete (scaffold)\r\n");
}

/// Per-AP init — called once each Application Processor is brought up.
pub fn ap_init(cpu_idx: u32) {
    gdt::init_ap(cpu_idx);
    idt::load();
    apic::init_ap();
    cpu::enable_fpu_simd();
    syscall::init_ap(cpu_idx);
}

/// Wake all APs and bring SMP online (single-core scaffold for now).
pub fn smp_init(resp: Option<&'static limine::request::MpResponse>) {
    smp::init(resp);
}

/// Halt the current core until an interrupt (or event) arrives.
#[inline(always)]
pub fn halt() {
    unsafe { asm!("wfi", options(nomem, nostack)) };
}

/// Unmask interrupts, wait for one (WFI), then mask again.
///
/// Mirror of the x86_64 `sti; hlt; cli` idle pattern.
#[inline(always)]
pub fn enable_and_halt() {
    unsafe {
        asm!(
            "msr daifclr, #0b0011",
            "wfi",
            "msr daifset, #0b0011",
            options(nomem, nostack),
        )
    };
}

/// Permanently halt this core (used in panic).
pub fn halt_forever() -> ! {
    loop {
        unsafe { asm!("msr daifset, #0b0011", "wfi", options(nomem, nostack)) };
    }
}

/// Enable IRQ/FIQ delivery on this core.
#[inline(always)]
pub fn enable_interrupts() {
    unsafe { asm!("msr daifclr, #0b0011", options(nomem, nostack)) };
}

/// Disable IRQ/FIQ delivery on this core.
#[inline(always)]
pub fn disable_interrupts() {
    unsafe { asm!("msr daifset, #0b0011", options(nomem, nostack)) };
}

/// Read a free-running cycle/time counter (x86 `rdtsc` analogue).
///
/// Uses the virtual count register `CNTVCT_EL0` from the ARM generic timer.
#[inline(always)]
pub fn rdtsc() -> u64 {
    let cnt: u64;
    unsafe { asm!("mrs {}, cntvct_el0", out(reg) cnt, options(nomem, nostack)) };
    cnt
}

// ── Context switch ────────────────────────────────────────────────────────────

/// Switch from the current task to a new one.
///
/// Saves the AAPCS64 callee-saved registers (x19–x30, including the frame
/// pointer x29 and link register x30) of the old task onto its kernel stack,
/// stores SP into `*old_sp`, loads `new_sp`, restores the new task's saved
/// registers, and returns into it.
///
/// # Safety
/// `old_sp` must point to the `kernel_sp` field of the currently running task;
/// `new_sp` must be a kernel stack prepared by `spawn_kernel_task` or a prior
/// call to this function. Must be called with interrupts disabled.
#[unsafe(naked)]
pub unsafe extern "C" fn context_switch(old_sp: *mut u64, new_sp: u64) {
    // AAPCS64: x0 = old_sp, x1 = new_sp.
    core::arch::naked_asm!(
        // Save callee-saved registers of the old task (x19..x30) as pairs.
        "stp x29, x30, [sp, #-16]!",
        "stp x27, x28, [sp, #-16]!",
        "stp x25, x26, [sp, #-16]!",
        "stp x23, x24, [sp, #-16]!",
        "stp x21, x22, [sp, #-16]!",
        "stp x19, x20, [sp, #-16]!",
        // Save current SP into *old_sp.
        "mov x2, sp",
        "str x2, [x0]",
        // Load new SP.
        "mov sp, x1",
        // Restore new task's callee-saved registers.
        "ldp x19, x20, [sp], #16",
        "ldp x21, x22, [sp], #16",
        "ldp x23, x24, [sp], #16",
        "ldp x25, x26, [sp], #16",
        "ldp x27, x28, [sp], #16",
        "ldp x29, x30, [sp], #16",
        // Return — branches to the new task's saved link register.
        "ret",
    )
}

/// Trampoline entered by a freshly spawned task on its very first schedule.
///
/// The task function pointer is the top-of-stack word left by
/// `spawn_kernel_task`. We pop it, enable interrupts, then call it. If it ever
/// returns (it should not for kernel tasks), we park.
#[unsafe(naked)]
pub unsafe extern "C" fn task_entry() {
    core::arch::naked_asm!(
        "ldr x9, [sp], #16",            // task fn pointer pushed by spawn_kernel_task
        "msr daifclr, #0b0011",         // enable IRQ+FIQ for this task
        "blr x9",                       // run the task function
        // Task returned — should not happen for kernel tasks; park.
        "0:",
        "msr daifset, #0b0011",
        "wfi",
        "b 0b",
    )
}
