//! aarch64 bring-up milestones 6+7: SVC syscall + EL0 user process (placeholder).

use crate::arch::aarch64::uart::a64println;

pub fn run() -> ! {
    a64println!("[boot] (SVC/EL0 user milestone not yet wired — parking)");
    loop {
        unsafe { core::arch::asm!("wfi", options(nomem, nostack)) };
    }
}
