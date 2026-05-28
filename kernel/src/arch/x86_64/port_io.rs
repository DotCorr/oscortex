//! Legacy x86 I/O port access (IN/OUT instructions).
//!
//! All drivers that talk to legacy I/O BARs (virtio legacy, PS/2 8042) must use
//! these helpers — never embed port I/O asm in driver modules.

#[inline(always)]
pub unsafe fn inb(port: u16) -> u8 {
    let v: u8;
    core::arch::asm!("in al, dx", in("dx") port, out("al") v, options(nomem, nostack));
    v
}

#[inline(always)]
pub unsafe fn outb(port: u16, val: u8) {
    core::arch::asm!("out dx, al", in("dx") port, in("al") val, options(nomem, nostack));
}

#[inline(always)]
pub unsafe fn inw(port: u16) -> u16 {
    let v: u16;
    core::arch::asm!("in ax, dx", in("dx") port, out("ax") v, options(nomem, nostack));
    v
}

#[inline(always)]
pub unsafe fn outw(port: u16, val: u16) {
    core::arch::asm!("out dx, ax", in("dx") port, in("ax") val, options(nomem, nostack));
}

#[inline(always)]
pub unsafe fn inl(port: u16) -> u32 {
    let v: u32;
    core::arch::asm!("in eax, dx", in("dx") port, out("eax") v, options(nomem, nostack));
    v
}

#[inline(always)]
pub unsafe fn outl(port: u16, val: u32) {
    core::arch::asm!("out dx, eax", in("dx") port, in("eax") val, options(nomem, nostack));
}
