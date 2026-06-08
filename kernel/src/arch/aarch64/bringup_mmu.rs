//! aarch64 bring-up milestone 2: MMU enable (placeholder — filled in next step).

use crate::arch::aarch64::uart::a64println;

pub fn run() -> ! {
    a64println!("[boot] (MMU milestone not yet wired — parking)");
    loop {
        unsafe { core::arch::asm!("wfe", options(nomem, nostack)) };
    }
}
