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
        let mut linebuf = PanicBuf::new();
        let _ = write!(linebuf, ":{}:{}\r\n", loc.line(), loc.column());
        crate::logger::early_print(linebuf.as_str());
    }
    // Best-effort backtrace: walk the saved RBP chain a few frames and print
    // each return address. Stops on a bogus pointer.
    unsafe {
        let mut rbp: u64 = crate::arch::read_frame_pointer();
        crate::logger::early_print("backtrace:\r\n");
        for _ in 0..16 {
            if rbp < 0xffff_8000_0000_0000 || (rbp & 7) != 0 { break; }
            let saved_rbp = (rbp as *const u64).read_volatile();
            let ret_addr  = ((rbp + 8) as *const u64).read_volatile();
            let mut fbuf = PanicBuf::new();
            let _ = write!(fbuf, "  rip={:#x}\r\n", ret_addr);
            crate::logger::early_print(fbuf.as_str());
            if saved_rbp <= rbp { break; }
            rbp = saved_rbp;
        }
    }
    log::error!("KERNEL PANIC at {}: {}", info.location().map_or("?", |l| l.file()), info.message());
    crate::arch::halt_forever()
}

