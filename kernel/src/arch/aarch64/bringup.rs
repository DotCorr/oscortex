//! aarch64 bare-metal bring-up sequence.
//!
//! Driven from the assembly `_start` (boot.rs) via `aarch64_start`. Runs the
//! platform-primitive milestones in dependency order, printing progress over
//! PL011 serial at every step:
//!
//!   1. SERIAL   — PL011 UART online (the debug lifeline)
//!   2. MMU      — ARMv8 translation tables, MAIR/TCR, SCTLR_EL1.M=1
//!   3. VECTORS  — VBAR_EL1 → 16-entry EL1 vector table, trap frame save/restore
//!   4. GIC      — distributor + CPU interface, EOI, timer IRQ routing
//!   5. TIMER    — CNTP/CNTV periodic tick → scheduler-tick analogue
//!   6. SVC      — EL0 `svc #0` → capture user GPRs → dispatch → ERET
//!   7. EL0      — enter a user process and service its syscalls
//!
//! This is a self-contained proof of the ARM platform layer: it exercises every
//! arch primitive the shared kernel needs, without depending on the Limine boot
//! protocol (which the x86 production path uses). Once these primitives are
//! proven, the shared scheduler/mm/syscall layers can be brought up on top.

use crate::arch::aarch64::uart;
use crate::arch::aarch64::uart::a64println;

/// Entry from `aarch64_start`. Never returns.
pub fn run() -> ! {
    // ── Milestone 1: SERIAL ────────────────────────────────────────────────
    uart::init();
    a64println!();
    a64println!("========================================");
    a64println!("  OSCortex aarch64 bring-up");
    a64println!("========================================");
    a64println!("[boot] PL011 UART online @ 0x09000000");

    // Report the Exception Level we landed at and key system registers, so the
    // serial log alone tells us the CPU state we're bringing up from.
    let current_el: u64;
    let sctlr: u64;
    let mpidr: u64;
    let cntfrq: u64;
    unsafe {
        core::arch::asm!("mrs {}, CurrentEL", out(reg) current_el, options(nomem, nostack));
        core::arch::asm!("mrs {}, sctlr_el1", out(reg) sctlr, options(nomem, nostack));
        core::arch::asm!("mrs {}, mpidr_el1", out(reg) mpidr, options(nomem, nostack));
        core::arch::asm!("mrs {}, cntfrq_el0", out(reg) cntfrq, options(nomem, nostack));
    }
    a64println!("[boot] CurrentEL = EL{}", (current_el >> 2) & 0x3);
    uart::puts("[boot] SCTLR_EL1 = ");
    uart::puthex_full(sctlr);
    uart::puts(" (MMU ");
    uart::puts(if sctlr & 1 != 0 { "ON" } else { "OFF" });
    uart::puts(")\n");
    uart::puts("[boot] MPIDR_EL1 = ");
    uart::puthex_full(mpidr);
    uart::puts("\n");
    uart::puts("[boot] CNTFRQ_EL0 = ");
    uart::puthex(cntfrq);
    a64println!(" ({} Hz timer)", cntfrq);

    a64println!("[boot] Milestone 1 (SERIAL) complete.");

    // Remaining milestones are wired in subsequent steps.
    crate::arch::aarch64::bringup_mmu::run();
}
