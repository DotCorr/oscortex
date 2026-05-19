//! UART 16550 driver — Phase 48.
//!
//! Provides non-blocking byte-level read/write on COM1 (I/O base 0x3F8) for
//! userspace consumption via `SYS_SERIAL_READ` / `SYS_SERIAL_WRITE`.
//!
//! The kernel logger already owns COM1 for TX (via logger::SerialPort), so we
//! ONLY do RX here to avoid conflicting TX access.  TX from userspace is
//! forwarded through the logger path so log output and userspace output are
//! correctly serialised.

const COM1: u16 = 0x3F8;

// 16550 register offsets (DLAB = 0)
const REG_RBR:  u16 = 0; // Receive Buffer Register (read)
const REG_THR:  u16 = 0; // Transmit Holding Register (write)
const REG_IER:  u16 = 1; // Interrupt Enable Register
const REG_FCR:  u16 = 2; // FIFO Control Register (write)
const REG_LCR:  u16 = 3; // Line Control Register
const REG_MCR:  u16 = 4; // Modem Control Register
const REG_LSR:  u16 = 5; // Line Status Register (read)

const LSR_DATA_READY: u8 = 1 << 0; // Bit 0: received data ready
const LSR_THR_EMPTY:  u8 = 1 << 5; // Bit 5: transmit holding register empty

// ── I/O helpers ───────────────────────────────────────────────────────────────

#[inline(always)]
unsafe fn inb(port: u16) -> u8 {
    let v: u8;
    core::arch::asm!("in al, dx", out("al") v, in("dx") port, options(nomem, nostack));
    v
}

#[inline(always)]
unsafe fn outb(port: u16, val: u8) {
    core::arch::asm!("out dx, al", in("dx") port, in("al") val, options(nomem, nostack));
}

// ── Public API ────────────────────────────────────────────────────────────────

/// Read one byte from COM1 without blocking.
///
/// Returns `Some(byte)` if data is available, `None` if the receive FIFO is
/// empty (caller should return EAGAIN to userspace).
pub fn read_byte_nonblocking() -> Option<u8> {
    let lsr = unsafe { inb(COM1 + REG_LSR) };
    if lsr & LSR_DATA_READY != 0 {
        Some(unsafe { inb(COM1 + REG_RBR) })
    } else {
        None
    }
}

/// Write a byte to COM1 (spin-wait for THR empty).
/// Exposed here so userspace serial-write doesn't need to go through the
/// logger lock chain.
pub fn write_byte(byte: u8) {
    unsafe {
        while inb(COM1 + REG_LSR) & LSR_THR_EMPTY == 0 {}
        outb(COM1 + REG_THR, byte);
    }
}

/// Write a slice of bytes to COM1.
pub fn write_bytes(data: &[u8]) {
    for &b in data {
        if b == b'\n' {
            write_byte(b'\r');
        }
        write_byte(b);
    }
}

/// Initialise the UART (115200 8-N-1, FIFO enabled).
///
/// Safe to call multiple times — re-initialisation is idempotent.
pub fn init() {
    unsafe {
        outb(COM1 + REG_IER, 0x00); // Disable interrupts
        outb(COM1 + REG_LCR, 0x80); // Enable DLAB
        outb(COM1 + REG_RBR, 0x01); // Divisor low  = 1 → 115200 baud
        outb(COM1 + REG_IER, 0x00); // Divisor high = 0
        outb(COM1 + REG_LCR, 0x03); // 8 bits, no parity, 1 stop bit (DLAB off)
        outb(COM1 + REG_FCR, 0xC7); // Enable FIFO, clear, 14-byte threshold
        outb(COM1 + REG_MCR, 0x0B); // RTS+DTR, aux out 2 (enables IRQ)
    }
    log::info!("[UART] COM1 initialised at 115200 8-N-1");
}
