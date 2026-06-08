//! Early kernel logger — serial port (COM1) + framebuffer.

use core::fmt::Write;
use spin::Mutex;
use log::{Log, Metadata, Record, Level};

static LOGGER: KernelLogger = KernelLogger;
static SERIAL: Mutex<SerialPort> = Mutex::new(SerialPort::new(0x3F8)); // COM1

struct KernelLogger;
impl Log for KernelLogger {
    fn enabled(&self, meta: &Metadata) -> bool { meta.level() <= Level::Debug }
    fn log(&self, record: &Record) {
        // Format once into a stack buffer, write to both serial and framebuffer.
        use core::fmt::Write;
        let mut buf = FmtBuf::new();
        let _ = write!(buf, "[{}] {}: {}\n", record.level(), record.target(), record.args());
        let s = buf.as_str();

        let rflags = crate::arch::interrupts_save_and_disable();
        SERIAL.lock().write_str(s).ok();
        crate::drivers::fb::write_str(s);
        crate::arch::interrupts_restore(rflags);
    }
    fn flush(&self) {}
}

/// Tiny stack-allocated write buffer for log formatting (no heap needed).
struct FmtBuf {
    buf: [u8; 2048],
    len: usize,
}
impl FmtBuf {
    fn new() -> Self { Self { buf: [0u8; 2048], len: 0 } }
    fn as_str(&self) -> &str {
        core::str::from_utf8(&self.buf[..self.len]).unwrap_or("(fmt err)")
    }
}
impl core::fmt::Write for FmtBuf {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        let bytes = s.as_bytes();
        let space = self.buf.len() - self.len;
        let n = bytes.len().min(space);
        self.buf[self.len..self.len + n].copy_from_slice(&bytes[..n]);
        self.len += n;
        Ok(())
    }
}

pub fn init(fb: Option<&limine::request::FramebufferResponse>) {
    // Initialize COM1 UART (115200 baud, 8N1).
    SERIAL.lock().init_uart();
    // Initialize framebuffer text console if Limine provided one.
    if let Some(fb_resp) = fb {
        crate::drivers::fb::init(fb_resp);
    }
    log::set_logger(&LOGGER).ok();
    // Hot per-syscall traces (epoll_ctl, mprotect, cond-signal, keypress) at
    // Debug/Warn flood the synchronous COM1 UART *and* re-render the framebuffer
    // text console for every line, with interrupts disabled — tens of seconds of
    // warmup on emulated bare metal. Default to Error; raise for deep debugging.
    log::set_max_level(log::LevelFilter::Error);
}

/// Write a raw string to COM1 without going through the log framework.
/// Safe to call before logger::init().
pub fn early_print(s: &str) {
    let mut port = SerialPort::new(0x3F8);
    port.init_uart();
    let _ = port.write_str(s);
}

// ── Serial port ───────────────────────────────────────────────────────────────

struct SerialPort {
    #[cfg(target_arch = "x86_64")]
    port: u16,
}

impl SerialPort {
    #[cfg(target_arch = "x86_64")]
    const fn new(port: u16) -> Self { Self { port } }
    #[cfg(not(target_arch = "x86_64"))]
    const fn new(_port: u16) -> Self { Self {} }

    /// Initialize UART: 115200 baud, 8-N-1, disable interrupts (x86_64 only).
    fn init_uart(&mut self) {
        #[cfg(target_arch = "x86_64")]
        unsafe {
            cpu_out8(self.port + 1, 0x00); // Disable all interrupts
            cpu_out8(self.port + 3, 0x80); // Enable DLAB (set baud rate divisor)
            cpu_out8(self.port + 0, 0x01); // Divisor lo byte: 1 = 115200 baud
            cpu_out8(self.port + 1, 0x00); // Divisor hi byte
            cpu_out8(self.port + 3, 0x03); // 8 bits, no parity, 1 stop bit
            cpu_out8(self.port + 2, 0xC7); // Enable FIFO, clear, 14-byte threshold
            cpu_out8(self.port + 4, 0x0B); // IRQs enabled, RTS/DSR set
        }
    }

    fn write_byte(&mut self, byte: u8) {
        #[cfg(target_arch = "x86_64")]
        unsafe {
            // Wait until transmit holding register is empty.
            while (cpu_in8(self.port + 5) & 0x20) == 0 {}
            cpu_out8(self.port, byte);
        }
        // On non-x86 arches the serial stub is a no-op until a PL011/UART driver
        // is implemented. The framebuffer console still receives all output.
        #[cfg(not(target_arch = "x86_64"))]
        let _ = byte;
    }
}

impl Write for SerialPort {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        for b in s.bytes() {
            if b == b'\n' { self.write_byte(b'\r'); }
            self.write_byte(b);
        }
        Ok(())
    }
}

#[cfg(target_arch = "x86_64")]
#[inline] unsafe fn cpu_out8(port: u16, val: u8) {
    core::arch::asm!("out dx, al", in("dx") port, in("al") val);
}
#[cfg(target_arch = "x86_64")]
#[inline] unsafe fn cpu_in8(port: u16) -> u8 {
    let val: u8;
    core::arch::asm!("in al, dx", in("dx") port, out("al") val);
    val
}

