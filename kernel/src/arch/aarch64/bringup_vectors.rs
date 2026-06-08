//! aarch64 bring-up milestone 3: exception vectors (placeholder).

use crate::arch::aarch64::uart::a64println;

pub fn run() -> ! {
    a64println!("[boot] (VECTORS milestone not yet wired — parking)");
    loop {
        unsafe { core::arch::asm!("wfe", options(nomem, nostack)) };
    }
}
