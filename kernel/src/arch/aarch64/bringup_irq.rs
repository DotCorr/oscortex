//! aarch64 bring-up milestones 4+5: GIC + generic timer periodic tick.

use crate::arch::aarch64::uart::a64println;
use crate::arch::aarch64::{gic, timer, vectors};
use core::sync::atomic::Ordering;

/// IRQ handler: acknowledge at the GIC, service the timer, EOI.
fn irq_handler(_f: &mut vectors::TrapFrame) {
    let intid = gic::acknowledge();
    if intid == timer::TIMER_PPI {
        timer::on_tick();
    }
    if intid < 1020 {
        gic::eoi(intid);
    }
}

pub fn run() -> ! {
    a64println!("[boot] Milestone 4 (GIC): init distributor + CPU interface...");
    gic::init_distributor();
    gic::init_cpu_interface();
    a64println!("[boot] GIC online (GICv2 @ GICD 0x08000000 / GICC 0x08010000).");

    a64println!("[boot] Milestone 5 (TIMER): arming generic-timer @ 100 Hz...");
    vectors::set_irq_handler(irq_handler);
    timer::init_periodic(100);

    // Unmask IRQ delivery and let the timer tick.
    unsafe { core::arch::asm!("msr daifclr, #0b0010", options(nomem, nostack)) };

    // Spin until we observe a handful of ticks, then report.
    let start = timer::count();
    let mut last_report = 0u64;
    loop {
        let ticks = timer::TICKS.load(Ordering::Relaxed);
        if ticks >= 5 {
            break;
        }
        if ticks != last_report {
            last_report = ticks;
        }
        // Wait for the next interrupt.
        unsafe { core::arch::asm!("wfi", options(nomem, nostack)) };
    }
    let elapsed = timer::count() - start;
    let freq = timer::freq_hz();

    a64println!(
        "[boot] timer ticks = {}, IRQs = {}, elapsed counts = {} (~{} ms @ {} Hz)",
        timer::TICKS.load(Ordering::Relaxed),
        vectors::IRQ_COUNT.load(Ordering::Relaxed),
        elapsed,
        if freq != 0 { elapsed * 1000 / freq } else { 0 },
        freq
    );

    if timer::TICKS.load(Ordering::Relaxed) >= 5 {
        a64println!("[boot] Milestone 4+5 (GIC + TIMER) complete — periodic IRQ ticking.");
    } else {
        a64println!("[boot] Milestone 4+5 FAILED — no timer ticks observed.");
    }

    crate::arch::aarch64::bringup_user::run();
}
