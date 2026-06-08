//! aarch64 bring-up milestones 4+5: GIC + generic timer (placeholder).

use crate::arch::aarch64::uart::a64println;

pub fn run() -> ! {
    a64println!("[boot] (GIC/TIMER milestone not yet wired — parking)");
    loop {
        unsafe { core::arch::asm!("wfe", options(nomem, nostack)) };
    }
}
