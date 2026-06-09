//! aarch64 direct-boot entry path (QEMU `-M virt -kernel`).
//!
//! This is the bare-metal bring-up entry for the ARM port. QEMU loads the
//! kernel ELF at its physical link address (0x4008_0000 on `-M virt`) and jumps
//! to `_start` with the MMU OFF, caches off, and (for `-cpu cortex-a72` without
//! `virtualization=on`) at EL1. `_start`:
//!
//!   1. Parks all but the primary CPU (MPIDR affinity 0).
//!   2. If we entered at EL2, drops to EL1 via `eret`.
//!   3. Sets up the boot stack.
//!   4. Zeroes the BSS.
//!   5. Branches to `aarch64_start` (Rust), which brings up serial → MMU →
//!      exceptions → GIC → timer → SVC → an EL0 user process.
//!
//! The whole bring-up runs identity-mapped low-physical until the MMU is on.

use core::arch::global_asm;

global_asm!(
    r#"
.section .text.boot, "ax"
.globl _start
_start:
    // QEMU `-kernel` passes the Flattened Device Tree (DTB) pointer in x0 at
    // entry. Preserve it in x20 (callee-saved, untouched by the bring-up asm)
    // so we can stash it once the BSS — which holds the DTB-pointer static — is
    // zeroed. The MPIDR check below clobbers x0.
    mov     x20, x0

    // ── Park secondary CPUs: only MPIDR.Aff0==0 proceeds ──────────────────
    mrs     x0, mpidr_el1
    and     x0, x0, #0xFF
    cbz     x0, 1f
0:  wfe
    b       0b
1:
    // ── If we booted at EL2, drop to EL1 ──────────────────────────────────
    mrs     x0, CurrentEL
    lsr     x0, x0, #2
    cmp     x0, #2
    b.ne    2f                      // already at EL1 (or EL3 — unhandled)

    // Configure EL2 so EL1 runs in AArch64 with sane state, then eret to EL1.
    mov     x0, #(1 << 31)          // HCR_EL2.RW = 1 → EL1 is AArch64
    msr     hcr_el2, x0
    // SCTLR_EL1: reserved-1 bits set, MMU/caches off for now.
    mov     x0, #0x0800
    movk    x0, #0x30d0, lsl #16
    msr     sctlr_el1, x0
    // SPSR_EL2: mask DAIF, return to EL1h (use SP_EL1).
    mov     x0, #0x3c5              // D,A,I,F masked + mode EL1h(0b0101)
    msr     spsr_el2, x0
    adr     x0, 2f
    msr     elr_el2, x0
    eret

2:  // ── Now at EL1 ────────────────────────────────────────────────────────
    // Mask all interrupts during bring-up.
    msr     daifset, #0xf

    // Enable FP/AdvSIMD at EL1/EL0 (CPACR_EL1.FPEN = 0b11). The Rust codegen
    // emits SIMD/FP instructions even in early bring-up (e.g. alignment
    // ub-checks use `fmov`), which trap to EL1 unless FPEN is cleared.
    mrs     x0, cpacr_el1
    orr     x0, x0, #(3 << 20)
    msr     cpacr_el1, x0
    isb

    // Set up the boot stack (top of a BSS-resident stack region).
    adrp    x0, {boot_stack_top}
    add     x0, x0, :lo12:{boot_stack_top}
    mov     sp, x0

    // ── Zero BSS ──────────────────────────────────────────────────────────
    adrp    x1, {bss_start}
    add     x1, x1, :lo12:{bss_start}
    adrp    x2, {bss_end}
    add     x2, x2, :lo12:{bss_end}
3:  cmp     x1, x2
    b.hs    4f
    str     xzr, [x1], #8
    b       3b
4:
    // ── Into Rust ─────────────────────────────────────────────────────────
    // AAPCS64: pass the saved DTB pointer (x20) to aarch64_start as arg0 (x0).
    mov     x0, x20
    bl      {aarch64_start}
    // aarch64_start never returns; park if it does.
5:  wfe
    b       5b
"#,
    boot_stack_top = sym BOOT_STACK_TOP,
    bss_start      = sym crate::arch::aarch64::boot::__bss_start_sym,
    bss_end        = sym crate::arch::aarch64::boot::__bss_end_sym,
    aarch64_start  = sym aarch64_start,
);

// ── Boot stack (BSS-resident, 64 KiB) ───────────────────────────────────────
//
// Defined in assembly so the "top" label sits exactly one-past-the-end of the
// reserved region (the stack grows down from BOOT_STACK_TOP).
core::arch::global_asm!(
    r#"
.section .bss.boot_stack, "aw", @nobits
.balign 16
BOOT_STACK_BOTTOM:
    .skip 65536
.globl BOOT_STACK_TOP
BOOT_STACK_TOP:
"#
);

extern "C" {
    static BOOT_STACK_TOP: u8;
}

// Re-export the linker BSS bounds as Rust symbols the global_asm can reference.
extern "C" {
    #[link_name = "__bss_start"]
    pub static __bss_start_sym: u8;
    #[link_name = "__bss_end"]
    pub static __bss_end_sym: u8;
}

/// First Rust function on the aarch64 boot path. Runs identity-mapped, MMU off,
/// at EL1, on the boot stack. `dtb_ptr` is the value QEMU placed in x0 (the
/// Flattened Device Tree blob's physical address).
///
/// Hands off to the production boot path (`aarch64_kernel_boot`), which records
/// the DTB pointer, runs the minimal ARM early-init (serial + MMU), then enters
/// the SHARED `kernel_main`. The self-contained bring-up demo is still reachable
/// via [`crate::arch::aarch64::bringup::run`] for platform-primitive validation.
#[no_mangle]
pub extern "C" fn aarch64_start(dtb_ptr: u64) -> ! {
    // Record the DTB pointer now that BSS is zeroed (the static lives in BSS).
    crate::arch::aarch64::fdt::set_dtb_ptr(dtb_ptr);
    crate::arch::aarch64::boot_prod::aarch64_kernel_boot(dtb_ptr)
}
