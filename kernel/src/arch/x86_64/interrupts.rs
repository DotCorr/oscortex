//! x86_64 interrupt helpers.

/// Remap and disable the legacy 8259 PIC so it doesn't send spurious IRQs
/// that would conflict with APIC.
pub unsafe fn disable_pic() {
    use core::arch::asm;

    // ICW1: initialise both PICs.
    out8(0x20, 0x11);
    out8(0xA0, 0x11);
    // ICW2: remap to vectors 0x20-0x2F (won't matter — we mask them all).
    out8(0x21, 0x20);
    out8(0xA1, 0x28);
    // ICW3.
    out8(0x21, 0x04);
    out8(0xA1, 0x02);
    // ICW4: 8086 mode.
    out8(0x21, 0x01);
    out8(0xA1, 0x01);
    // Mask all IRQs on both PICs.
    out8(0x21, 0xFF);
    out8(0xA1, 0xFF);
}

#[inline]
unsafe fn out8(port: u16, val: u8) {
    core::arch::asm!("out dx, al", in("dx") port, in("al") val);
}

/// Memory barriers.
#[inline]
pub fn memory_fence() {
    unsafe { core::arch::asm!("mfence") }
}

pub mod interrupts {
    pub use super::disable_pic;
}
