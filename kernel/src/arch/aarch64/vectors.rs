//! EL1 exception vector table (VBAR_EL1) + trap-frame entry/exit.
//!
//! AArch64 dispatches all exceptions through a 16-entry, 2 KiB-aligned vector
//! table. Each entry is 0x80 bytes (32 instructions) and corresponds to one of
//! 4 groups × 4 kinds:
//!
//!   group 0: Current EL with SP_EL0   (we never run on SP_EL0 → unused)
//!   group 1: Current EL with SP_ELx   (kernel-mode exceptions/IRQs)
//!   group 2: Lower EL using AArch64    (EL0 syscalls/IRQs — the user path)
//!   group 3: Lower EL using AArch32    (unused — we are AArch64-only)
//!
//!   kinds:   +0x000 Synchronous, +0x080 IRQ, +0x100 FIQ, +0x180 SError
//!
//! Each vector saves the full integer register file (x0–x30) plus SP_EL0,
//! ELR_EL1 and SPSR_EL1 into a [`TrapFrame`] on the current stack, calls the
//! Rust dispatcher with a pointer to it, then restores and `eret`s. The frame
//! layout is shared with `enter_user` so user-entry and exception-return use one
//! ABI.

use core::arch::global_asm;
use core::sync::atomic::{AtomicU64, Ordering};

/// Full integer register context saved on every EL1 exception entry.
///
/// Layout is fixed and matched byte-for-byte by the asm save/restore in
/// `vectors.S`-equivalent `global_asm!` below. Total size: 36 × 8 = 288 bytes
/// (34 fields + ELR/SPSR), kept 16-byte aligned.
#[repr(C)]
#[derive(Debug, Default, Clone, Copy)]
pub struct TrapFrame {
    pub x: [u64; 31], // x0..x30 (x30 = LR)
    pub sp_el0: u64,  // user stack pointer (SP_EL0)
    pub elr: u64,     // ELR_EL1 (return address)
    pub spsr: u64,    // SPSR_EL1 (saved PSTATE)
    pub esr: u64,     // ESR_EL1 (syndrome — filled by the dispatcher path)
}

/// Count of each exception kind seen, for bring-up visibility.
pub static SYNC_EL1: AtomicU64 = AtomicU64::new(0);
pub static IRQ_COUNT: AtomicU64 = AtomicU64::new(0);
pub static SYNC_EL0: AtomicU64 = AtomicU64::new(0);

extern "C" {
    /// Vector table base symbol defined in the `global_asm!` below.
    static __vectors_el1: u8;
}

/// Install the vector table into VBAR_EL1 for the current core.
pub fn install() {
    let base = unsafe { core::ptr::addr_of!(__vectors_el1) as u64 };
    unsafe {
        core::arch::asm!(
            "msr vbar_el1, {}",
            "isb",
            in(reg) base,
            options(nostack, preserves_flags),
        );
    }
}

/// Read the installed VBAR_EL1 (for verification).
pub fn vbar() -> u64 {
    let v: u64;
    unsafe { core::arch::asm!("mrs {}, vbar_el1", out(reg) v, options(nomem, nostack)) };
    v
}

// ── Vector table + common save/restore (assembly) ───────────────────────────
//
// The macro `VENTRY` emits one vector slot that jumps to a shared save stub,
// tagged with the exception "kind" so the Rust dispatcher knows the source.
global_asm!(
    r#"
.section .text
.balign 2048
.globl __vectors_el1
__vectors_el1:

    // ── Group 0: Current EL, SP_EL0 (unused) ────────────────────────────────
    .balign 0x80
    b   __vec_curr_sync         // +0x000 Sync
    .balign 0x80
    b   __vec_curr_irq          // +0x080 IRQ
    .balign 0x80
    b   __vec_curr_fiq          // +0x100 FIQ
    .balign 0x80
    b   __vec_curr_serror       // +0x180 SError

    // ── Group 1: Current EL, SP_ELx (kernel) ────────────────────────────────
    .balign 0x80
    b   __vec_curr_sync         // +0x200 Sync
    .balign 0x80
    b   __vec_curr_irq          // +0x280 IRQ
    .balign 0x80
    b   __vec_curr_fiq          // +0x300 FIQ
    .balign 0x80
    b   __vec_curr_serror       // +0x380 SError

    // ── Group 2: Lower EL, AArch64 (EL0 user) ───────────────────────────────
    .balign 0x80
    b   __vec_lower_sync        // +0x400 Sync (SVC syscalls, user faults)
    .balign 0x80
    b   __vec_lower_irq         // +0x480 IRQ
    .balign 0x80
    b   __vec_lower_fiq         // +0x500 FIQ
    .balign 0x80
    b   __vec_lower_serror      // +0x580 SError

    // ── Group 3: Lower EL, AArch32 (unused) ─────────────────────────────────
    .balign 0x80
    b   __vec_lower_sync
    .balign 0x80
    b   __vec_lower_irq
    .balign 0x80
    b   __vec_lower_fiq
    .balign 0x80
    b   __vec_lower_serror

// ── Save macro: push a TrapFrame onto the current stack ─────────────────────
.macro SAVE_FRAME
    sub     sp, sp, #(36 * 8)               // room for x0..x30, sp_el0, elr, spsr, esr
    stp     x0,  x1,  [sp, #(0  * 8)]
    stp     x2,  x3,  [sp, #(2  * 8)]
    stp     x4,  x5,  [sp, #(4  * 8)]
    stp     x6,  x7,  [sp, #(6  * 8)]
    stp     x8,  x9,  [sp, #(8  * 8)]
    stp     x10, x11, [sp, #(10 * 8)]
    stp     x12, x13, [sp, #(12 * 8)]
    stp     x14, x15, [sp, #(14 * 8)]
    stp     x16, x17, [sp, #(16 * 8)]
    stp     x18, x19, [sp, #(18 * 8)]
    stp     x20, x21, [sp, #(20 * 8)]
    stp     x22, x23, [sp, #(22 * 8)]
    stp     x24, x25, [sp, #(24 * 8)]
    stp     x26, x27, [sp, #(26 * 8)]
    stp     x28, x29, [sp, #(28 * 8)]
    str     x30,      [sp, #(30 * 8)]
    mrs     x0, sp_el0
    str     x0,       [sp, #(31 * 8)]
    mrs     x0, elr_el1
    mrs     x1, spsr_el1
    stp     x0,  x1,  [sp, #(32 * 8)]       // elr, spsr
    mrs     x0, esr_el1
    str     x0,       [sp, #(34 * 8)]       // esr
.endm

// ── Restore macro: pop a TrapFrame and eret ─────────────────────────────────
.macro RESTORE_FRAME
    ldp     x0,  x1,  [sp, #(32 * 8)]       // elr, spsr
    msr     elr_el1, x0
    msr     spsr_el1, x1
    ldr     x0,       [sp, #(31 * 8)]
    msr     sp_el0, x0
    ldp     x0,  x1,  [sp, #(0  * 8)]
    ldp     x2,  x3,  [sp, #(2  * 8)]
    ldp     x4,  x5,  [sp, #(4  * 8)]
    ldp     x6,  x7,  [sp, #(6  * 8)]
    ldp     x8,  x9,  [sp, #(8  * 8)]
    ldp     x10, x11, [sp, #(10 * 8)]
    ldp     x12, x13, [sp, #(12 * 8)]
    ldp     x14, x15, [sp, #(14 * 8)]
    ldp     x16, x17, [sp, #(16 * 8)]
    ldp     x18, x19, [sp, #(18 * 8)]
    ldp     x20, x21, [sp, #(20 * 8)]
    ldp     x22, x23, [sp, #(22 * 8)]
    ldp     x24, x25, [sp, #(24 * 8)]
    ldp     x26, x27, [sp, #(26 * 8)]
    ldp     x28, x29, [sp, #(28 * 8)]
    ldr     x30,      [sp, #(30 * 8)]
    add     sp, sp, #(36 * 8)
.endm

// ── Per-kind entry stubs: save, call dispatcher (x0=frame, x1=kind), restore ─
__vec_curr_sync:
    SAVE_FRAME
    mov     x0, sp
    mov     x1, #1                          // KIND_CURR_SYNC
    bl      {dispatch}
    RESTORE_FRAME
    eret

__vec_curr_irq:
    SAVE_FRAME
    mov     x0, sp
    mov     x1, #2                          // KIND_CURR_IRQ
    bl      {dispatch}
    RESTORE_FRAME
    eret

__vec_curr_fiq:
    SAVE_FRAME
    mov     x0, sp
    mov     x1, #3                          // KIND_CURR_FIQ
    bl      {dispatch}
    RESTORE_FRAME
    eret

__vec_curr_serror:
    SAVE_FRAME
    mov     x0, sp
    mov     x1, #4                          // KIND_CURR_SERROR
    bl      {dispatch}
    RESTORE_FRAME
    eret

__vec_lower_sync:
    SAVE_FRAME
    mov     x0, sp
    mov     x1, #5                          // KIND_LOWER_SYNC (EL0)
    bl      {dispatch}
    RESTORE_FRAME
    eret

__vec_lower_irq:
    SAVE_FRAME
    mov     x0, sp
    mov     x1, #6                          // KIND_LOWER_IRQ
    bl      {dispatch}
    RESTORE_FRAME
    eret

__vec_lower_fiq:
    SAVE_FRAME
    mov     x0, sp
    mov     x1, #7                          // KIND_LOWER_FIQ
    bl      {dispatch}
    RESTORE_FRAME
    eret

__vec_lower_serror:
    SAVE_FRAME
    mov     x0, sp
    mov     x1, #8                          // KIND_LOWER_SERROR
    bl      {dispatch}
    RESTORE_FRAME
    eret
"#,
    dispatch = sym dispatch,
);

// Exception "kind" tags passed from the asm stubs.
pub const KIND_CURR_SYNC: u64 = 1;
pub const KIND_CURR_IRQ: u64 = 2;
pub const KIND_CURR_FIQ: u64 = 3;
pub const KIND_CURR_SERROR: u64 = 4;
pub const KIND_LOWER_SYNC: u64 = 5;
pub const KIND_LOWER_IRQ: u64 = 6;
pub const KIND_LOWER_FIQ: u64 = 7;
pub const KIND_LOWER_SERROR: u64 = 8;

// ESR_EL1 exception classes we care about.
const EC_SVC64: u64 = 0x15; // SVC instruction from AArch64
const EC_DABT_LOWER: u64 = 0x24; // Data abort, lower EL
const EC_IABT_LOWER: u64 = 0x20; // Instruction abort, lower EL

/// Optional override for synchronous-from-EL0 handling, installed by the
/// syscall layer once SVC dispatch is wired (milestone 6). Returns true if it
/// fully handled the exception.
pub static SVC_HANDLER: AtomicU64 = AtomicU64::new(0);
/// Optional override for IRQ handling, installed by the timer/GIC layer
/// (milestone 4/5).
pub static IRQ_HANDLER: AtomicU64 = AtomicU64::new(0);

type SvcFn = fn(&mut TrapFrame);
type IrqFn = fn(&mut TrapFrame);

/// Install the SVC (EL0 synchronous) handler.
pub fn set_svc_handler(f: SvcFn) {
    SVC_HANDLER.store(f as usize as u64, Ordering::Release);
}

/// Install the IRQ handler.
pub fn set_irq_handler(f: IrqFn) {
    IRQ_HANDLER.store(f as usize as u64, Ordering::Release);
}

/// The C-ABI exception dispatcher called by every vector stub.
///
/// `frame` points at the saved [`TrapFrame`] on the trapping stack; `kind` is
/// the exception group/kind tag from the stub.
#[no_mangle]
extern "C" fn dispatch(frame: *mut TrapFrame, kind: u64) {
    let f = unsafe { &mut *frame };
    let ec = (f.esr >> 26) & 0x3f;

    match kind {
        KIND_LOWER_IRQ | KIND_CURR_IRQ => {
            IRQ_COUNT.fetch_add(1, Ordering::Relaxed);
            let h = IRQ_HANDLER.load(Ordering::Acquire);
            if h != 0 {
                let func: IrqFn = unsafe { core::mem::transmute(h as usize) };
                func(f);
            }
        }
        KIND_LOWER_SYNC => {
            SYNC_EL0.fetch_add(1, Ordering::Relaxed);
            if ec == EC_SVC64 {
                let h = SVC_HANDLER.load(Ordering::Acquire);
                if h != 0 {
                    let func: SvcFn = unsafe { core::mem::transmute(h as usize) };
                    func(f);
                } else {
                    report_unhandled(f, kind, ec);
                }
            } else {
                // User fault (data/instruction abort etc.) — report and, for
                // bring-up, advance past it would be wrong; surface it.
                report_unhandled(f, kind, ec);
            }
        }
        KIND_CURR_SYNC => {
            SYNC_EL1.fetch_add(1, Ordering::Relaxed);
            if ec == EC_SVC64 {
                // Recoverable kernel-side SVC (used by the bring-up self-test
                // to prove the vector save/dispatch/restore/eret round-trip).
                let h = SVC_HANDLER.load(Ordering::Acquire);
                if h != 0 {
                    let func: SvcFn = unsafe { core::mem::transmute(h as usize) };
                    func(f);
                }
            } else {
                report_unhandled(f, kind, ec);
            }
        }
        _ => report_unhandled(f, kind, ec),
    }
}

/// Print an unhandled-exception report over serial. For bring-up we park after
/// printing so the cause is visible (rather than looping in the vector).
fn report_unhandled(f: &TrapFrame, kind: u64, ec: u64) -> ! {
    use crate::arch::aarch64::uart;
    uart::puts("\n[EXC] UNHANDLED exception: kind=");
    uart::puthex(kind);
    uart::puts(" EC=");
    uart::puthex(ec);
    uart::puts("\n      ESR_EL1=");
    uart::puthex_full(f.esr);
    uart::puts(" ELR=");
    uart::puthex_full(f.elr);
    uart::puts(" SPSR=");
    uart::puthex_full(f.spsr);
    uart::puts("\n      SP_EL0=");
    uart::puthex_full(f.sp_el0);
    uart::puts(" x0=");
    uart::puthex_full(f.x[0]);
    uart::puts("\n");
    loop {
        unsafe { core::arch::asm!("wfe", options(nomem, nostack)) };
    }
}
