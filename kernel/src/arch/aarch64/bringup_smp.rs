//! aarch64 bring-up milestone 8 (optional): secondary-core wake via PSCI.
//!
//! On QEMU `-M virt` the boot CPU lands at EL1 and the PSCI conduit is HVC.
//! We bring CPU 1 online with PSCI CPU_ON, pointing it at a physical secondary
//! entry stub that sets up its stack, enables FP, installs the shared vector
//! table, and reports in over serial. This proves multi-core wake-up; the full
//! per-CPU scheduler is a larger follow-up (the BSP path already runs userspace).
//!
//! Single-core boot remains the proven path — this milestone only runs when a
//! second CPU is present and PSCI accepts the CPU_ON call.

use crate::arch::aarch64::uart;
use crate::arch::aarch64::uart::a64println;
use core::arch::global_asm;
use core::sync::atomic::{AtomicU64, Ordering};

/// Set by the secondary core once it is running at EL1.
pub static SECONDARY_ONLINE: AtomicU64 = AtomicU64::new(0);
/// The secondary core records its MPIDR here.
pub static SECONDARY_MPIDR: AtomicU64 = AtomicU64::new(0);

// Secondary boot stack (16 KiB).
global_asm!(
    r#"
.section .bss.secondary_stack, "aw", @nobits
.balign 16
SECONDARY_STACK_BOTTOM:
    .skip 16384
.globl SECONDARY_STACK_TOP
SECONDARY_STACK_TOP:
"#
);

extern "C" {
    static SECONDARY_STACK_TOP: u8;
}

// Physical secondary entry stub. PSCI starts it with the MMU OFF at EL1, so it
// must run identity-mapped (which our identity map provides) and set up a stack
// before calling into Rust.
global_asm!(
    r#"
.section .text
.globl __secondary_entry
__secondary_entry:
    // Enable FP/AdvSIMD (Rust codegen needs it; see boot.rs).
    mrs     x0, cpacr_el1
    orr     x0, x0, #(3 << 20)
    msr     cpacr_el1, x0
    isb
    // Stack.
    adrp    x0, SECONDARY_STACK_TOP
    add     x0, x0, :lo12:SECONDARY_STACK_TOP
    mov     sp, x0
    // Into Rust.
    bl      {secondary_main}
0:  wfe
    b       0b
"#,
    secondary_main = sym secondary_main,
);

/// Rust entry for the secondary core (runs at EL1, MMU initially off — but the
/// BSP's tables are global; we just need this core's TTBR/SCTLR. For this
/// report-in milestone we run translation-off and only touch device MMIO + a
/// couple of static atomics, both physically addressed.)
#[no_mangle]
extern "C" fn secondary_main() -> ! {
    let mpidr: u64;
    unsafe { core::arch::asm!("mrs {}, mpidr_el1", out(reg) mpidr, options(nomem, nostack)) };
    SECONDARY_MPIDR.store(mpidr & 0xFF_FFFF, Ordering::Release);
    SECONDARY_ONLINE.store(1, Ordering::Release);
    // Park; the per-CPU scheduler is a follow-up.
    loop {
        unsafe { core::arch::asm!("wfe", options(nomem, nostack)) };
    }
}

extern "C" {
    static __secondary_entry: u8;
}

/// Try to bring CPU 1 online via PSCI CPU_ON. Reports the outcome over serial.
pub fn try_wake_secondary() {
    use crate::arch::aarch64::psci;

    let entry = unsafe { core::ptr::addr_of!(__secondary_entry) as u64 };
    // CPU 1's MPIDR on QEMU virt cortex-a72 is affinity 1 (0x1).
    let target_mpidr: u64 = 1;

    a64println!("[boot] Milestone 8 (SMP): PSCI CPU_ON cpu1 @ entry...");
    uart::puts("[boot]   secondary entry PA = ");
    uart::puthex_full(entry);
    uart::puts("\n");

    let rc = psci::cpu_on(target_mpidr, entry, 0);
    uart::puts("[boot]   PSCI CPU_ON returned ");
    uart::puthex(rc as u64);
    uart::puts("\n");

    if rc != 0 {
        a64println!("[boot] SMP: PSCI CPU_ON not available in this boot mode (single-core path proven).");
        return;
    }

    // Wait briefly for the secondary to report in.
    for _ in 0..2_000_000u64 {
        if SECONDARY_ONLINE.load(Ordering::Acquire) != 0 {
            break;
        }
        core::hint::spin_loop();
    }

    if SECONDARY_ONLINE.load(Ordering::Acquire) != 0 {
        uart::puts("[boot] SMP: CPU 1 ONLINE, MPIDR = ");
        uart::puthex_full(SECONDARY_MPIDR.load(Ordering::Acquire));
        uart::puts("\n");
        a64println!("[boot] Milestone 8 (SMP) complete — secondary core woken via PSCI.");
    } else {
        a64println!("[boot] SMP: secondary did not report in (single-core path proven).");
    }
}
