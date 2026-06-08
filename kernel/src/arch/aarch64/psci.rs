//! PSCI (Power State Coordination Interface) — QEMU `-M virt`.
//!
//! QEMU exposes PSCI via the SMC conduit (HVC when virtualization is on). We
//! use it for clean SYSTEM_OFF (test-harness exit) and, later, CPU_ON for SMP.

/// PSCI function IDs (SMC64 calling convention).
const PSCI_SYSTEM_OFF: u32 = 0x8400_0008;
const PSCI_CPU_ON: u64 = 0xC400_0003; // SMC64 variant

/// Power the machine off via PSCI SYSTEM_OFF. Never returns.
///
/// QEMU `-M virt` exposes PSCI over the conduit selected by the firmware/boot
/// EL: HVC when an EL2 is present, SMC when an EL3 (secure firmware) is. When
/// the kernel is `-kernel`-booted straight into EL1 with neither EL2 nor EL3,
/// *neither* conduit is reachable (both trap as Undefined at EL1), so we mask
/// interrupts and park instead — the test harness sees the kernel idle.
pub fn system_off() -> ! {
    // Only attempt the SMC/HVC call if we are at EL2+ (where a conduit exists).
    let el: u64;
    unsafe { core::arch::asm!("mrs {}, CurrentEL", out(reg) el, options(nomem, nostack)) };
    if (el >> 2) & 0x3 >= 2 {
        unsafe {
            core::arch::asm!(
                "mov w0, {fn_id:w}",
                "hvc #0",
                fn_id = in(reg) PSCI_SYSTEM_OFF,
                out("x0") _, out("x1") _, out("x2") _, out("x3") _,
                options(nostack),
            );
        }
    }
    // Park with interrupts masked.
    unsafe { core::arch::asm!("msr daifset, #0xf", options(nomem, nostack)) };
    loop {
        unsafe { core::arch::asm!("wfi", options(nomem, nostack)) };
    }
}

/// Bring a secondary CPU online at `entry` (physical) with context `ctx`.
/// Returns the PSCI status (0 = success). SMP milestone helper.
pub fn cpu_on(target_mpidr: u64, entry: u64, ctx: u64) -> i64 {
    let ret: i64;
    unsafe {
        core::arch::asm!(
            "mov x0, {fn_id}",
            "mov x1, {mpidr}",
            "mov x2, {entry}",
            "mov x3, {ctx}",
            "smc #0",
            "mov {ret}, x0",
            fn_id = in(reg) PSCI_CPU_ON,
            mpidr = in(reg) target_mpidr,
            entry = in(reg) entry,
            ctx = in(reg) ctx,
            ret = out(reg) ret,
            out("x0") _, out("x1") _, out("x2") _, out("x3") _,
            options(nostack),
        );
    }
    ret
}
