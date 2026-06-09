//! PL011 UART driver — aarch64 serial console (QEMU `-M virt` @ 0x0900_0000).
//!
//! This is the bring-up printf/debug lifeline. QEMU's `-M virt` machine wires a
//! PrimeCell PL011 UART at physical 0x0900_0000; with `-serial mon:stdio` its
//! TX shows up on the host terminal. We only need polled TX (and optional RX)
//! for bring-up, so this is deliberately minimal.
//!
//! The driver works both before and after the MMU is enabled: pre-MMU the base
//! is a raw physical address; once the MMU is on we map the same physical page
//! at an identity/uncached VA and keep using it (the bring-up identity-maps the
//! device region, so the same address stays valid).

use core::sync::atomic::{AtomicU64, Ordering};

/// PL011 base on QEMU `-M virt`. Overridable once the MMU remaps it.
const PL011_PHYS: u64 = 0x0900_0000;

/// Active MMIO base for the UART (physical until the MMU is on; then the VA the
/// device page was identity-mapped to — same value with an identity map).
static UART_BASE: AtomicU64 = AtomicU64::new(PL011_PHYS);

// PL011 register offsets.
const UARTDR: u64 = 0x00; // Data register
const UARTFR: u64 = 0x18; // Flag register
const UARTIBRD: u64 = 0x24; // Integer baud-rate divisor
const UARTFBRD: u64 = 0x28; // Fractional baud-rate divisor
const UARTLCR_H: u64 = 0x2c; // Line control
const UARTCR: u64 = 0x30; // Control
const UARTIMSC: u64 = 0x38; // Interrupt mask set/clear

// UARTFR bits.
const FR_TXFF: u32 = 1 << 5; // transmit FIFO full
const FR_RXFE: u32 = 1 << 4; // receive FIFO empty

#[inline(always)]
fn base() -> u64 {
    UART_BASE.load(Ordering::Relaxed)
}

#[inline(always)]
unsafe fn reg_read(off: u64) -> u32 {
    core::ptr::read_volatile((base() + off) as *const u32)
}

#[inline(always)]
unsafe fn reg_write(off: u64, val: u32) {
    core::ptr::write_volatile((base() + off) as *mut u32, val);
}

/// Initialise the PL011 for polled 115200-8N1 TX. Safe to call before the MMU
/// is on. QEMU ignores the exact baud divisor, but real hardware needs it.
pub fn init() {
    unsafe {
        // Disable the UART while reconfiguring.
        reg_write(UARTCR, 0);
        // Mask all interrupts (we poll).
        reg_write(UARTIMSC, 0);
        // 115200 baud assuming a 24 MHz UARTCLK (QEMU virt): divisor = 24e6 /
        // (16*115200) = 13.0208 → IBRD=13, FBRD=round(0.0208*64)=1.
        reg_write(UARTIBRD, 13);
        reg_write(UARTFBRD, 1);
        // 8 bits, FIFO enabled, no parity.
        reg_write(UARTLCR_H, (1 << 4) | (3 << 5));
        // Enable UART + TX + RX.
        reg_write(UARTCR, (1 << 0) | (1 << 8) | (1 << 9));
    }
}

/// Point the driver at a new base address (used after the MMU remaps the device
/// page to a non-identity VA, if ever). For an identity map this is a no-op.
pub fn set_base(new_base: u64) {
    UART_BASE.store(new_base, Ordering::Release);
}

/// Write one byte, blocking while the TX FIFO is full.
#[inline]
pub fn putb(b: u8) {
    unsafe {
        while reg_read(UARTFR) & FR_TXFF != 0 {
            core::hint::spin_loop();
        }
        reg_write(UARTDR, b as u32);
    }
}

/// Write a string, expanding `\n` → `\r\n`.
pub fn puts(s: &str) {
    for &b in s.as_bytes() {
        if b == b'\n' {
            putb(b'\r');
        }
        putb(b);
    }
}

/// Write a 64-bit value as `0x`-prefixed, zero-padded hex.
pub fn puthex(mut v: u64) {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    putb(b'0');
    putb(b'x');
    let mut started = false;
    for i in (0..16).rev() {
        let nib = ((v >> (i * 4)) & 0xf) as usize;
        if nib != 0 || started || i == 0 {
            putb(HEX[nib]);
            started = true;
        }
    }
    let _ = &mut v;
}

/// Write a u64 as `0x`-prefixed, full-width (16 nibble) hex — handy for raw
/// register dumps where leading zeros matter.
pub fn puthex_full(v: u64) {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    putb(b'0');
    putb(b'x');
    for i in (0..16).rev() {
        putb(HEX[((v >> (i * 4)) & 0xf) as usize]);
    }
}

/// Non-blocking single-byte read. Returns `None` when the RX FIFO is empty.
pub fn try_getb() -> Option<u8> {
    unsafe {
        if reg_read(UARTFR) & FR_RXFE != 0 {
            None
        } else {
            Some((reg_read(UARTDR) & 0xff) as u8)
        }
    }
}

/// `core::fmt::Write` adapter so bring-up code can `write!`/`writeln!`.
pub struct Writer;

impl core::fmt::Write for Writer {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        puts(s);
        Ok(())
    }
}

/// Formatted write to the UART (the bring-up `print!` analogue).
pub fn _fmt(args: core::fmt::Arguments) {
    use core::fmt::Write as _;
    let _ = Writer.write_fmt(args);
}

/// `println!`-style helper for the bring-up path. Defined as a plain macro
/// (not `#[macro_export]`) so it stays addressable within the arch module tree
/// without tripping the absolute-path lint on macro-expanded exports.
macro_rules! a64println {
    () => { $crate::arch::aarch64::uart::puts("\n") };
    ($($arg:tt)*) => {{
        $crate::arch::aarch64::uart::_fmt(format_args!($($arg)*));
        $crate::arch::aarch64::uart::puts("\n");
    }};
}
pub(crate) use a64println;

/// `print!`-style helper for the bring-up path.
macro_rules! a64print {
    ($($arg:tt)*) => {{
        $crate::arch::aarch64::uart::_fmt(format_args!($($arg)*));
    }};
}
#[allow(unused_imports)]
pub(crate) use a64print;
