//! Simple framebuffer text console — 8×8 bitmap font, 32 bpp XRGB.
//!
//! Thread-safety: all mutable state is stored as atomics; the cursor column+row
//! are protected by a spin mutex so that adjacent character writes remain
//! contiguous even on SMP.

use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use spin::Mutex;
use alloc::vec;
use alloc::vec::Vec;

// ── Font constants ────────────────────────────────────────────────────────────

/// Glyph cell width in pixels.
const CHAR_W: u32 = 8;
/// Glyph cell height in pixels.
const CHAR_H: u32 = 8;

/// Foreground pixel colour (bright white).
const FG: u32 = 0x00FF_FFFF;
/// Background pixel colour (dark charcoal).
const BG: u32 = 0x001A1A2E;

/// Classic IBM VGA 8×8 bitmap font — printable ASCII 0x20–0x7F (96 glyphs).
///
/// `FONT[ch - 0x20][row]`: each byte is one pixel row, MSB = leftmost pixel.
/// Data is in the public domain (IBM PC BIOS font, predates copyright).
#[rustfmt::skip]
static FONT: [[u8; 8]; 96] = [
/*0x20 ' ' */ [0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00],
/*0x21 '!' */ [0x18,0x3C,0x3C,0x18,0x18,0x00,0x18,0x00],
/*0x22 '"' */ [0x66,0x66,0x24,0x00,0x00,0x00,0x00,0x00],
/*0x23 '#' */ [0x6C,0x6C,0xFE,0x6C,0xFE,0x6C,0x6C,0x00],
/*0x24 '$' */ [0x18,0x3E,0x60,0x3C,0x06,0x7C,0x18,0x00],
/*0x25 '%' */ [0x00,0xC6,0xCC,0x18,0x30,0x66,0xC6,0x00],
/*0x26 '&' */ [0x38,0x6C,0x38,0x76,0xDC,0xCC,0x76,0x00],
/*0x27 '\''*/ [0x18,0x18,0x30,0x00,0x00,0x00,0x00,0x00],
/*0x28 '(' */ [0x0C,0x18,0x30,0x30,0x30,0x18,0x0C,0x00],
/*0x29 ')' */ [0x30,0x18,0x0C,0x0C,0x0C,0x18,0x30,0x00],
/*0x2A '*' */ [0x00,0x66,0x3C,0xFF,0x3C,0x66,0x00,0x00],
/*0x2B '+' */ [0x00,0x18,0x18,0x7E,0x18,0x18,0x00,0x00],
/*0x2C ',' */ [0x00,0x00,0x00,0x00,0x00,0x18,0x18,0x30],
/*0x2D '-' */ [0x00,0x00,0x00,0x7E,0x00,0x00,0x00,0x00],
/*0x2E '.' */ [0x00,0x00,0x00,0x00,0x00,0x18,0x18,0x00],
/*0x2F '/' */ [0x06,0x0C,0x18,0x30,0x60,0xC0,0x80,0x00],
/*0x30 '0' */ [0x3C,0x66,0x6E,0x76,0x66,0x66,0x3C,0x00],
/*0x31 '1' */ [0x18,0x38,0x18,0x18,0x18,0x18,0x7E,0x00],
/*0x32 '2' */ [0x3C,0x66,0x06,0x1C,0x30,0x66,0x7E,0x00],
/*0x33 '3' */ [0x3C,0x66,0x06,0x1C,0x06,0x66,0x3C,0x00],
/*0x34 '4' */ [0x06,0x1E,0x36,0x66,0x7F,0x06,0x06,0x00],
/*0x35 '5' */ [0x7E,0x60,0x7C,0x06,0x06,0x66,0x3C,0x00],
/*0x36 '6' */ [0x1C,0x30,0x60,0x7C,0x66,0x66,0x3C,0x00],
/*0x37 '7' */ [0x7E,0x66,0x0C,0x18,0x30,0x30,0x30,0x00],
/*0x38 '8' */ [0x3C,0x66,0x66,0x3C,0x66,0x66,0x3C,0x00],
/*0x39 '9' */ [0x3C,0x66,0x66,0x3E,0x06,0x0C,0x38,0x00],
/*0x3A ':' */ [0x00,0x18,0x18,0x00,0x00,0x18,0x18,0x00],
/*0x3B ';' */ [0x00,0x18,0x18,0x00,0x00,0x18,0x18,0x30],
/*0x3C '<' */ [0x06,0x0C,0x18,0x30,0x18,0x0C,0x06,0x00],
/*0x3D '=' */ [0x00,0x00,0x7E,0x00,0x00,0x7E,0x00,0x00],
/*0x3E '>' */ [0x60,0x30,0x18,0x0C,0x18,0x30,0x60,0x00],
/*0x3F '?' */ [0x3C,0x66,0x06,0x0C,0x18,0x00,0x18,0x00],
/*0x40 '@' */ [0x3E,0x63,0x6F,0x69,0x6F,0x60,0x3E,0x00],
/*0x41 'A' */ [0x18,0x3C,0x66,0x7E,0x66,0x66,0x66,0x00],
/*0x42 'B' */ [0x7C,0x66,0x66,0x7C,0x66,0x66,0x7C,0x00],
/*0x43 'C' */ [0x3C,0x66,0x60,0x60,0x60,0x66,0x3C,0x00],
/*0x44 'D' */ [0x78,0x6C,0x66,0x66,0x66,0x6C,0x78,0x00],
/*0x45 'E' */ [0x7E,0x60,0x60,0x78,0x60,0x60,0x7E,0x00],
/*0x46 'F' */ [0x7E,0x60,0x60,0x78,0x60,0x60,0x60,0x00],
/*0x47 'G' */ [0x3C,0x66,0x60,0x6E,0x66,0x66,0x3C,0x00],
/*0x48 'H' */ [0x66,0x66,0x66,0x7E,0x66,0x66,0x66,0x00],
/*0x49 'I' */ [0x3C,0x18,0x18,0x18,0x18,0x18,0x3C,0x00],
/*0x4A 'J' */ [0x1E,0x0C,0x0C,0x0C,0x0C,0x6C,0x38,0x00],
/*0x4B 'K' */ [0x66,0x6C,0x78,0x70,0x78,0x6C,0x66,0x00],
/*0x4C 'L' */ [0x60,0x60,0x60,0x60,0x60,0x60,0x7E,0x00],
/*0x4D 'M' */ [0x63,0x77,0x7F,0x6B,0x63,0x63,0x63,0x00],
/*0x4E 'N' */ [0x66,0x76,0x7E,0x7E,0x6E,0x66,0x66,0x00],
/*0x4F 'O' */ [0x3C,0x66,0x66,0x66,0x66,0x66,0x3C,0x00],
/*0x50 'P' */ [0x7C,0x66,0x66,0x7C,0x60,0x60,0x60,0x00],
/*0x51 'Q' */ [0x3C,0x66,0x66,0x66,0x66,0x3C,0x1E,0x00],
/*0x52 'R' */ [0x7C,0x66,0x66,0x7C,0x78,0x6C,0x66,0x00],
/*0x53 'S' */ [0x3C,0x66,0x60,0x3C,0x06,0x66,0x3C,0x00],
/*0x54 'T' */ [0x7E,0x18,0x18,0x18,0x18,0x18,0x18,0x00],
/*0x55 'U' */ [0x66,0x66,0x66,0x66,0x66,0x66,0x3C,0x00],
/*0x56 'V' */ [0x66,0x66,0x66,0x66,0x66,0x3C,0x18,0x00],
/*0x57 'W' */ [0x63,0x63,0x63,0x6B,0x7F,0x77,0x63,0x00],
/*0x58 'X' */ [0x66,0x66,0x3C,0x18,0x3C,0x66,0x66,0x00],
/*0x59 'Y' */ [0x66,0x66,0x66,0x3C,0x18,0x18,0x18,0x00],
/*0x5A 'Z' */ [0x7E,0x06,0x0C,0x18,0x30,0x60,0x7E,0x00],
/*0x5B '[' */ [0x3C,0x30,0x30,0x30,0x30,0x30,0x3C,0x00],
/*0x5C '\\'*/ [0xC0,0x60,0x30,0x18,0x0C,0x06,0x02,0x00],
/*0x5D ']' */ [0x3C,0x0C,0x0C,0x0C,0x0C,0x0C,0x3C,0x00],
/*0x5E '^' */ [0x10,0x38,0x6C,0xC6,0x00,0x00,0x00,0x00],
/*0x5F '_' */ [0x00,0x00,0x00,0x00,0x00,0x00,0x00,0xFF],
/*0x60 '`' */ [0x18,0x18,0x0C,0x00,0x00,0x00,0x00,0x00],
/*0x61 'a' */ [0x00,0x00,0x3C,0x06,0x3E,0x66,0x3E,0x00],
/*0x62 'b' */ [0x60,0x60,0x7C,0x66,0x66,0x66,0x7C,0x00],
/*0x63 'c' */ [0x00,0x00,0x3C,0x60,0x60,0x60,0x3C,0x00],
/*0x64 'd' */ [0x06,0x06,0x3E,0x66,0x66,0x66,0x3E,0x00],
/*0x65 'e' */ [0x00,0x00,0x3C,0x66,0x7E,0x60,0x3C,0x00],
/*0x66 'f' */ [0x1C,0x30,0x7C,0x30,0x30,0x30,0x30,0x00],
/*0x67 'g' */ [0x00,0x00,0x3E,0x66,0x66,0x3E,0x06,0x7C],
/*0x68 'h' */ [0x60,0x60,0x7C,0x66,0x66,0x66,0x66,0x00],
/*0x69 'i' */ [0x18,0x00,0x38,0x18,0x18,0x18,0x3C,0x00],
/*0x6A 'j' */ [0x06,0x00,0x0E,0x06,0x06,0x06,0x66,0x3C],
/*0x6B 'k' */ [0x60,0x60,0x66,0x6C,0x78,0x6C,0x66,0x00],
/*0x6C 'l' */ [0x38,0x18,0x18,0x18,0x18,0x18,0x3C,0x00],
/*0x6D 'm' */ [0x00,0x00,0x66,0x7F,0x7F,0x6B,0x63,0x00],
/*0x6E 'n' */ [0x00,0x00,0x7C,0x66,0x66,0x66,0x66,0x00],
/*0x6F 'o' */ [0x00,0x00,0x3C,0x66,0x66,0x66,0x3C,0x00],
/*0x70 'p' */ [0x00,0x00,0x7C,0x66,0x66,0x7C,0x60,0x60],
/*0x71 'q' */ [0x00,0x00,0x3E,0x66,0x66,0x3E,0x06,0x06],
/*0x72 'r' */ [0x00,0x00,0x7C,0x66,0x60,0x60,0x60,0x00],
/*0x73 's' */ [0x00,0x00,0x3E,0x60,0x3C,0x06,0x7C,0x00],
/*0x74 't' */ [0x30,0x30,0x7C,0x30,0x30,0x30,0x1C,0x00],
/*0x75 'u' */ [0x00,0x00,0x66,0x66,0x66,0x66,0x3E,0x00],
/*0x76 'v' */ [0x00,0x00,0x66,0x66,0x66,0x3C,0x18,0x00],
/*0x77 'w' */ [0x00,0x00,0x63,0x6B,0x7F,0x3E,0x36,0x00],
/*0x78 'x' */ [0x00,0x00,0x66,0x3C,0x18,0x3C,0x66,0x00],
/*0x79 'y' */ [0x00,0x00,0x66,0x66,0x66,0x3E,0x06,0x7C],
/*0x7A 'z' */ [0x00,0x00,0x7E,0x0C,0x18,0x30,0x7E,0x00],
/*0x7B '{' */ [0x0E,0x18,0x18,0x70,0x18,0x18,0x0E,0x00],
/*0x7C '|' */ [0x18,0x18,0x18,0x00,0x18,0x18,0x18,0x00],
/*0x7D '}' */ [0x70,0x18,0x18,0x0E,0x18,0x18,0x70,0x00],
/*0x7E '~' */ [0x76,0xDC,0x00,0x00,0x00,0x00,0x00,0x00],
/*0x7F DEL  */ [0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00],
];

// ── Framebuffer state (all atomics for safe static storage) ───────────────────

/// Virtual base address of the framebuffer (already HHDM-mapped by Limine).
static FB_ADDR:     AtomicU64 = AtomicU64::new(0);
/// Pitch in 32-bit words (= pitch_bytes / 4 for 32bpp).
static FB_PITCH_PX: AtomicU32 = AtomicU32::new(0);
/// Display width in pixels.
static FB_WIDTH:    AtomicU32 = AtomicU32::new(0);
/// Display height in pixels.
static FB_HEIGHT:   AtomicU32 = AtomicU32::new(0);
/// Text columns (= width / CHAR_W).
static FB_COLS:     AtomicU32 = AtomicU32::new(0);
/// Text rows (= height / CHAR_H).
static FB_ROWS:     AtomicU32 = AtomicU32::new(0);
/// Set to true once the framebuffer is ready.
static FB_READY:    AtomicBool = AtomicBool::new(false);
/// Set to true once framebuffer text logging should be silenced.
static FB_SILENT:   AtomicBool = AtomicBool::new(false);

/// Text cursor (col, row).
static CURSOR: Mutex<(u32, u32)> = Mutex::new((0, 0));

static DOUBLE_BUFFER: Mutex<Option<Vec<u32>>> = Mutex::new(None);
static DOUBLE_BUFFER_ACTIVE: AtomicBool = AtomicBool::new(false);

struct InterruptGuard {
    rflags: u64,
}

impl InterruptGuard {
    #[inline(always)]
    fn new() -> Self {
        Self {
            rflags: crate::arch::interrupts_save_and_disable(),
        }
    }
}

impl Drop for InterruptGuard {
    #[inline(always)]
    fn drop(&mut self) {
        crate::arch::interrupts_restore(self.rflags);
    }
}

// ── Public API ────────────────────────────────────────────────────────────────

/// Initialise the framebuffer console from a Limine framebuffer response.
///
/// Called from `logger::init()` once per boot.  Uses the first framebuffer
/// reported by the bootloader.  Clears the screen to black before returning.
pub fn init(fb_resp: &limine::request::FramebufferResponse) {
    let fbs = fb_resp.framebuffers();
    if fbs.is_empty() { return; }

    let fb = fbs[0];
    // Limine maps the framebuffer into the HHDM — the address field is already
    // a valid virtual address we can write to directly.
    let addr = fb.address() as u64;
    if addr == 0 { return; }

    // Only 32 bpp is supported by this console.
    if fb.bpp != 32 { return; }

    let pitch_px = (fb.pitch / 4) as u32;
    let width    = fb.width as u32;
    let height   = fb.height as u32;
    let cols     = width  / CHAR_W;
    let rows     = height / CHAR_H;

    FB_ADDR    .store(addr,     Ordering::Release);
    FB_PITCH_PX.store(pitch_px, Ordering::Release);
    FB_WIDTH   .store(width,    Ordering::Release);
    FB_HEIGHT  .store(height,   Ordering::Release);
    FB_COLS    .store(cols,     Ordering::Release);
    FB_ROWS    .store(rows,     Ordering::Release);

    // Black out the screen.
    clear();

    FB_READY.store(true, Ordering::Release);
}

/// Write a string to the framebuffer console.
///
/// Handles `\n` (advance to next row, scroll if needed) and
/// `\r` (return to column 0).  All other control characters are skipped.
pub fn write_str(s: &str) {
    if !FB_READY.load(Ordering::Acquire) { return; }
    if FB_SILENT.load(Ordering::Relaxed) { return; }
    for b in s.bytes() {
        write_byte(b);
    }
}

/// Disable framebuffer console text logging (silences write_str).
pub fn disable_fb_logging() {
    FB_SILENT.store(true, Ordering::Release);
}

/// Return whether the framebuffer console is initialised.
pub fn is_ready() -> bool {
    FB_READY.load(Ordering::Acquire)
}

/// Return framebuffer size in pixels `(width, height)`.
pub fn size_px() -> Option<(u32, u32)> {
    if !is_ready() {
        return None;
    }
    Some((
        FB_WIDTH.load(Ordering::Relaxed),
        FB_HEIGHT.load(Ordering::Relaxed),
    ))
}

/// Return raw framebuffer parameters needed for userspace mapping.
///
/// Returns `(hhdm_vaddr, width, height, pitch_bytes)` where `pitch_bytes` is the
/// number of bytes per row (`FB_PITCH_PX * 4` for 32 bpp), or `None` if not ready.
pub fn fb_info() -> Option<(u64, u32, u32, u32)> {
    if !is_ready() {
        return None;
    }
    let addr        = FB_ADDR    .load(Ordering::Relaxed);
    let width       = FB_WIDTH   .load(Ordering::Relaxed);
    let height      = FB_HEIGHT  .load(Ordering::Relaxed);
    let pitch_bytes = FB_PITCH_PX.load(Ordering::Relaxed) * 4; // pitch_px stored in 32-bit words
    Some((addr, width, height, pitch_bytes))
}

/// Fill a rectangle in framebuffer space. Rectangle is clipped to screen bounds.
/// Write a single pixel at `(x, y)` with `color` (0x00RRGGBB).
/// No-op if not ready or out of bounds.
pub fn set_pixel(x: u32, y: u32, color: u32) {
    if !is_ready() { return; }
    let width  = FB_WIDTH.load(Ordering::Relaxed);
    let height = FB_HEIGHT.load(Ordering::Relaxed);
    if x >= width || y >= height { return; }
    let pitch_px = FB_PITCH_PX.load(Ordering::Relaxed) as usize;
    if DOUBLE_BUFFER_ACTIVE.load(Ordering::Relaxed) {
        let _guard = InterruptGuard::new();
        if let Some(ref mut buf) = *DOUBLE_BUFFER.lock() {
            buf[y as usize * pitch_px + x as usize] = color;
            return;
        }
    }
    let addr = FB_ADDR.load(Ordering::Relaxed) as *mut u32;
    unsafe { addr.add(y as usize * pitch_px + x as usize).write_volatile(color); }
}

pub fn fill_rect(x: i32, y: i32, w: u32, h: u32, color: u32) {
    if !is_ready() || w == 0 || h == 0 {
        return;
    }

    let width = FB_WIDTH.load(Ordering::Relaxed) as i32;
    let height = FB_HEIGHT.load(Ordering::Relaxed) as i32;
    let pitch_px = FB_PITCH_PX.load(Ordering::Relaxed) as usize;

    let x0 = x.max(0).min(width);
    let y0 = y.max(0).min(height);
    let x1 = (x.saturating_add(w as i32)).max(0).min(width);
    let y1 = (y.saturating_add(h as i32)).max(0).min(height);

    if x0 >= x1 || y0 >= y1 {
        return;
    }

    if DOUBLE_BUFFER_ACTIVE.load(Ordering::Relaxed) {
        let _guard = InterruptGuard::new();
        if let Some(ref mut buf) = *DOUBLE_BUFFER.lock() {
            for py in y0 as usize..y1 as usize {
                let row_offset = py * pitch_px;
                for px in x0 as usize..x1 as usize {
                    buf[row_offset + px] = color;
                }
            }
            return;
        }
    }

    let addr = FB_ADDR.load(Ordering::Relaxed) as *mut u32;
    unsafe {
        for py in y0 as usize..y1 as usize {
            let row = addr.add(py * pitch_px);
            for px in x0 as usize..x1 as usize {
                row.add(px).write_volatile(color);
            }
        }
    }
}

/// Blit a 32-bit RGBA source image into the framebuffer at `(x, y)`.
///
/// Source is expected in little-endian RGBA packed in `u32`; alpha is ignored
/// for now (opaque copy). The blit is clipped to framebuffer bounds.
pub fn blit_rgba32(x: i32, y: i32, src_w: u32, src_h: u32, src: &[u32]) {
    if !is_ready() || src_w == 0 || src_h == 0 {
        return;
    }

    let need = src_w as usize * src_h as usize;
    if src.len() < need {
        return;
    }

    let width = FB_WIDTH.load(Ordering::Relaxed) as i32;
    let height = FB_HEIGHT.load(Ordering::Relaxed) as i32;
    let pitch_px = FB_PITCH_PX.load(Ordering::Relaxed) as usize;

    let x0 = x.max(0).min(width);
    let y0 = y.max(0).min(height);
    let x1 = (x.saturating_add(src_w as i32)).max(0).min(width);
    let y1 = (y.saturating_add(src_h as i32)).max(0).min(height);

    if x0 >= x1 || y0 >= y1 {
        return;
    }

    let clip_w = (x1 - x0) as usize;
    let clip_h = (y1 - y0) as usize;
    let src_x0 = (x0 - x) as usize;
    let src_y0 = (y0 - y) as usize;
    let src_stride = src_w as usize;

    if DOUBLE_BUFFER_ACTIVE.load(Ordering::Relaxed) {
        let _guard = InterruptGuard::new();
        if let Some(ref mut buf) = *DOUBLE_BUFFER.lock() {
            for row in 0..clip_h {
                let sy = src_y0 + row;
                let dy = y0 as usize + row;
                let src_row = sy * src_stride + src_x0;
                let dst_row = dy * pitch_px + x0 as usize;
                for col in 0..clip_w {
                    let px = src[src_row + col];
                    let r = px & 0x0000_00FF;
                    let g = (px & 0x0000_FF00) >> 8;
                    let b = (px & 0x00FF_0000) >> 16;
                    let xrgb = (r << 16) | (g << 8) | b;
                    buf[dst_row + col] = xrgb;
                }
            }
            return;
        }
    }

    let dst_base = FB_ADDR.load(Ordering::Relaxed) as *mut u32;
    unsafe {
        for row in 0..clip_h {
            let sy = src_y0 + row;
            let dy = y0 as usize + row;
            let src_row = sy * src_stride + src_x0;
            let dst_row = dy * pitch_px + x0 as usize;
            for col in 0..clip_w {
                let px = src[src_row + col];
                // Convert RGBA (little-endian bytes: R,G,B,A) -> XRGB.
                let r = px & 0x0000_00FF;
                let g = (px & 0x0000_FF00) >> 8;
                let b = (px & 0x00FF_0000) >> 16;
                let xrgb = (r << 16) | (g << 8) | b;
                dst_base.add(dst_row + col).write_volatile(xrgb);
            }
        }
    }
}

pub fn set_double_buffer(active: bool) {
    DOUBLE_BUFFER_ACTIVE.store(active, Ordering::SeqCst);
    if active {
        let _guard = InterruptGuard::new();
        let mut db = DOUBLE_BUFFER.lock();
        if db.is_none() {
            let height = FB_HEIGHT.load(Ordering::Relaxed);
            let pitch_px = FB_PITCH_PX.load(Ordering::Relaxed);
            let size = pitch_px as usize * height as usize;
            *db = Some(vec![0u32; size]);
        }
    }
}

pub fn swap_buffers() {
    if !is_ready() || !DOUBLE_BUFFER_ACTIVE.load(Ordering::Relaxed) {
        return;
    }
    let _guard = InterruptGuard::new();
    let db = DOUBLE_BUFFER.lock();
    if let Some(ref buf) = *db {
        let addr = FB_ADDR.load(Ordering::Relaxed) as *mut u32;
        let height = FB_HEIGHT.load(Ordering::Relaxed) as usize;
        let pitch_px = FB_PITCH_PX.load(Ordering::Relaxed) as usize;
        let total = height * pitch_px;
        unsafe {
            core::ptr::copy_nonoverlapping(buf.as_ptr(), addr, total);
        }
    }
}

// ── Internal helpers ──────────────────────────────────────────────────────────

fn write_byte(b: u8) {
    match b {
        b'\r' => {
            CURSOR.lock().0 = 0;
        }
        b'\n' => {
            let mut cur = CURSOR.lock();
            cur.0 = 0;
            cur.1 += 1;
            let rows = FB_ROWS.load(Ordering::Relaxed);
            if cur.1 >= rows {
                drop(cur);
                scroll_up();
                CURSOR.lock().1 = rows - 1;
            }
        }
        0x20..=0x7E => {
            let (col, row) = {
                let cur = CURSOR.lock();
                (cur.0, cur.1)
            };
            blit_char(b, col, row);
            // Advance cursor.
            let cols = FB_COLS.load(Ordering::Relaxed);
            let rows = FB_ROWS.load(Ordering::Relaxed);
            let mut cur = CURSOR.lock();
            cur.0 += 1;
            if cur.0 >= cols {
                cur.0 = 0;
                cur.1 += 1;
                if cur.1 >= rows {
                    drop(cur);
                    scroll_up();
                    CURSOR.lock().1 = rows - 1;
                }
            }
        }
        _ => {} // Skip other control/non-printable bytes.
    }
}

/// Render a single glyph at text cell (col, row).
fn blit_char(ch: u8, col: u32, row: u32) {
    let addr     = FB_ADDR    .load(Ordering::Relaxed);
    let pitch_px = FB_PITCH_PX.load(Ordering::Relaxed);
    let width    = FB_WIDTH   .load(Ordering::Relaxed);
    let height   = FB_HEIGHT  .load(Ordering::Relaxed);

    let glyph_idx = ch.wrapping_sub(0x20) as usize;
    let glyph = &FONT[glyph_idx];

    let px0 = col * CHAR_W;
    let py0 = row * CHAR_H;

    let mut gy = 0;
    while gy < CHAR_H {
        let row_bits = glyph[gy as usize];
        let mut gx = 0;
        while gx < CHAR_W {
            let color = if (row_bits >> (7 - gx)) & 1 != 0 { FG } else { BG };
            let px = px0 + gx;
            let py = py0 + gy;
            if px < width && py < height {
                let offset = (py * pitch_px + px) as usize;
                // Safety: addr is a Limine-provided virtual framebuffer address;
                // offset is guaranteed within bounds by px < width && py < height.
                unsafe {
                    let ptr = addr as *mut u32;
                    ptr.add(offset).write_volatile(color);
                }
            }
            gx += 1;
        }
        gy += 1;
    }
}

/// Scroll the console up by one character row, then clear the bottom row.
fn scroll_up() {
    let addr     = FB_ADDR    .load(Ordering::Relaxed);
    let pitch_px = FB_PITCH_PX.load(Ordering::Relaxed);
    let height   = FB_HEIGHT  .load(Ordering::Relaxed);
    let rows     = FB_ROWS    .load(Ordering::Relaxed);

    // Number of pixel rows to copy.
    let copy_rows = height - CHAR_H;
    let words_per_row = pitch_px as usize;

    unsafe {
        let base = addr as *mut u32;
        // Move rows 1..rows up to rows 0..rows-1.
        let src = base.add(CHAR_H as usize * words_per_row);
        let dst = base;
        // Use volatile copy word by word (no libc, no memcpy assumption needed).
        let total = copy_rows as usize * words_per_row;
        core::ptr::copy(src, dst, total);

        // Clear the last character row.
        let clear_start = base.add((rows - 1) as usize * CHAR_H as usize * words_per_row);
        for i in 0..(CHAR_H as usize * words_per_row) {
            clear_start.add(i).write_volatile(BG);
        }
    }
}

/// Fill the entire framebuffer with the background colour.
fn clear() {
    let addr     = FB_ADDR    .load(Ordering::Relaxed);
    let pitch_px = FB_PITCH_PX.load(Ordering::Relaxed);
    let height   = FB_HEIGHT  .load(Ordering::Relaxed);

    let total = pitch_px as usize * height as usize;
    unsafe {
        let base = addr as *mut u32;
        for i in 0..total {
            base.add(i).write_volatile(BG);
        }
    }
}

