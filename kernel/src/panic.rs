//! Kernel panic handler.

use core::fmt::Write;
use core::panic::PanicInfo;

/// Stack-allocated 512-byte buffer for formatting panic messages without heap.
struct PanicBuf {
    buf: [u8; 512],
    pos: usize,
}

impl PanicBuf {
    const fn new() -> Self { Self { buf: [0u8; 512], pos: 0 } }
    fn as_str(&self) -> &str {
        core::str::from_utf8(&self.buf[..self.pos]).unwrap_or("(utf8 err)")
    }
}

impl Write for PanicBuf {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        let bytes = s.as_bytes();
        let remaining = self.buf.len() - self.pos;
        let n = bytes.len().min(remaining);
        self.buf[self.pos..self.pos + n].copy_from_slice(&bytes[..n]);
        self.pos += n;
        Ok(())
    }
}

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    crate::arch::disable_interrupts();
    let mut buf = PanicBuf::new();
    let _ = write!(buf, "{}", info.message());
    crate::logger::early_print("\r\n[PANIC] ");
    crate::logger::early_print(buf.as_str());
    crate::logger::early_print("\r\n");
    if let Some(loc) = info.location() {
        crate::logger::early_print(loc.file());
        crate::logger::early_print("\r\n");
    }
    log::error!("KERNEL PANIC at {}: {}", info.location().map_or("?", |l| l.file()), info.message());
    crate::arch::halt_forever()
}
