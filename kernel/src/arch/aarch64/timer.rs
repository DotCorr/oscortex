//! ARM generic timer — periodic EL1 VIRTUAL-timer tick (CNTV_*).
//!
//! Drives the same scheduler tick the x86 APIC timer drives. We use the EL1
//! VIRTUAL timer (CNTV_CTL_EL0 / CNTV_TVAL_EL0 / CNTVCT_EL0), PPI 27.
//!
//! Why virtual, not physical: OSCortex runs the kernel at EL1. Under a
//! hypervisor (Apple's HVF — what UTM uses — KVM, Xen, …) EL2 owns the PHYSICAL
//! timer (CNTP, PPI 30): EL1 accesses are trapped and its interrupt is NOT
//! delivered to the guest, so a CNTP-based tick simply never fires under HVF →
//! no vsync baton → frames schedule but never present (the engine reaches the
//! event loop and stalls). The VIRTUAL timer is the architected EL1-guest timer
//! and is delivered on HVF, on bare metal (CNTVOFF=0 → virtual time == physical
//! time), AND on QEMU `-M virt` TCG — so it is strictly more portable. CNTFRQ_EL0
//! gives the tick frequency (62.5 MHz on TCG virt, ~24 MHz on Apple Silicon).

use core::sync::atomic::{AtomicU64, Ordering};

/// EL1 VIRTUAL-timer PPI (architected; delivered under HVF and on bare metal).
pub const TIMER_PPI: u32 = 27;

/// Timer frequency in Hz (read from CNTFRQ_EL0 at init).
static FREQ_HZ: AtomicU64 = AtomicU64::new(0);
/// Reload interval in timer ticks (set from the requested Hz).
static INTERVAL_TICKS: AtomicU64 = AtomicU64::new(0);
/// Number of timer interrupts serviced.
pub static TICKS: AtomicU64 = AtomicU64::new(0);

#[inline]
fn cntfrq() -> u64 {
    let v: u64;
    unsafe { core::arch::asm!("mrs {}, cntfrq_el0", out(reg) v, options(nomem, nostack)) };
    v
}

/// Current virtual count (CNTVCT_EL0 — matches the virtual timer we tick on,
/// and the same counter vsync cadence is measured against).
#[inline]
pub fn count() -> u64 {
    let v: u64;
    unsafe { core::arch::asm!("mrs {}, cntvct_el0", out(reg) v, options(nomem, nostack)) };
    v
}

/// Program the timer for a periodic tick at `hz`, register it with the GIC, and
/// arm the first interval. Interrupts must still be unmasked by the caller.
pub fn init_periodic(hz: u64) {
    let freq = cntfrq();
    FREQ_HZ.store(freq, Ordering::Release);
    let interval = if hz == 0 { freq } else { freq / hz };
    INTERVAL_TICKS.store(interval, Ordering::Release);

    // Route + enable the timer PPI in the GIC.
    super::gic::set_priority(TIMER_PPI, 0x80);
    super::gic::enable_irq(TIMER_PPI);

    arm_next(interval);

    unsafe {
        // CNTV_CTL_EL0: ENABLE=1, IMASK=0 (bit0=enable, bit1=imask).
        core::arch::asm!("msr cntv_ctl_el0, {}", in(reg) 1u64, options(nomem, nostack));
        core::arch::asm!("isb", options(nomem, nostack));
    }
}

#[inline]
fn arm_next(interval: u64) {
    unsafe {
        // TVAL counts down from `interval`; the interrupt fires at zero.
        core::arch::asm!("msr cntv_tval_el0, {}", in(reg) interval, options(nomem, nostack));
    }
}

/// Called from the IRQ handler when the timer PPI fires: bump the tick count
/// and re-arm the next interval.
pub fn on_tick() {
    TICKS.fetch_add(1, Ordering::Relaxed);
    let interval = INTERVAL_TICKS.load(Ordering::Acquire);
    arm_next(interval);
}

/// Timer frequency in Hz (0 until init).
pub fn freq_hz() -> u64 {
    FREQ_HZ.load(Ordering::Acquire)
}
