//! aarch64 bring-up milestone 3: EL1 exception vectors.

use crate::arch::aarch64::uart;
use crate::arch::aarch64::uart::a64println;
use crate::arch::aarch64::vectors;
use core::sync::atomic::{AtomicU64, Ordering};

/// Test SVC handler: bumps a counter and returns (the vector restores+erets).
static TEST_SVC_HITS: AtomicU64 = AtomicU64::new(0);

fn test_svc_handler(_f: &mut vectors::TrapFrame) {
    TEST_SVC_HITS.fetch_add(1, Ordering::Relaxed);
}

pub fn run() -> ! {
    a64println!("[boot] Milestone 3 (VECTORS): installing VBAR_EL1...");
    vectors::install();
    uart::puts("[boot] VBAR_EL1 = ");
    uart::puthex_full(vectors::vbar());
    uart::puts("\n");

    // Prove the full vector round-trip with a *recoverable* trap: issue
    // `svc #0` from EL1. The CPU vectors to Current-EL Synchronous, the asm
    // stub saves a TrapFrame, the dispatcher sees EC=SVC64 and runs our test
    // handler (which bumps a counter), then the stub restores every register
    // and `eret`s back to the instruction after the `svc`. If the save/restore
    // were wrong we'd fault here; instead execution continues normally.
    vectors::set_svc_handler(test_svc_handler);

    let before = vectors::SYNC_EL1.load(Ordering::Relaxed);
    // Mark a register before the trap and check it survives the round-trip.
    let sentinel_in: u64 = 0xA5A5_1234_DEAD_BEEF;
    let sentinel_out: u64;
    unsafe {
        core::arch::asm!(
            "mov x9, {inv}",
            "svc #0",
            "mov {outv}, x9",
            inv = in(reg) sentinel_in,
            outv = out(reg) sentinel_out,
            out("x9") _,
            options(nomem, nostack),
        );
    }
    let after = vectors::SYNC_EL1.load(Ordering::Relaxed);
    let hits = TEST_SVC_HITS.load(Ordering::Relaxed);

    a64println!(
        "[boot] svc#0 round-trip: SYNC_EL1 {} -> {}, handler hits = {}",
        before, after, hits
    );
    uart::puts("[boot] callee reg preserved across trap: ");
    uart::puthex_full(sentinel_out);
    uart::puts(if sentinel_out == sentinel_in { " OK\n" } else { " CORRUPT\n" });

    if after == before + 1 && hits == 1 && sentinel_out == sentinel_in {
        a64println!("[boot] Milestone 3 (VECTORS) complete — trap save/dispatch/restore/eret OK.");
    } else {
        a64println!("[boot] Milestone 3 (VECTORS) FAILED — trap round-trip mismatch.");
    }

    crate::arch::aarch64::bringup_irq::run();
}
