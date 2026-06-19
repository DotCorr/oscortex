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

// ── Pixel format (firmware channel order) ─────────────────────────────────────
// Colors flow through this module as 0x00RRGGBB. Real UEFI GOP framebuffers are
// not always XRGB (e.g. many Intel Macs report RGB / red_shift=0), so we read the
// firmware's channel shifts and repack at the final framebuffer write. The
// XRGB fast-path keeps the common case (ARM ramfb, x86 std-vga) byte-identical.
static FB_R_SHIFT:   AtomicU32 = AtomicU32::new(16);
static FB_G_SHIFT:   AtomicU32 = AtomicU32::new(8);
static FB_B_SHIFT:   AtomicU32 = AtomicU32::new(0);
/// True when channel order is exactly XRGB (16/8/0) → write u32 colors verbatim.
static FB_XRGB_FAST: AtomicBool = AtomicBool::new(true);

/// Repack a 0x00RRGGBB color into the firmware's channel order. Identity on the
/// XRGB fast-path (no cost on the common path).
#[inline(always)]
fn fb_pack(color: u32) -> u32 {
    if FB_XRGB_FAST.load(Ordering::Relaxed) {
        return color;
    }
    let r = (color >> 16) & 0xFF;
    let g = (color >> 8) & 0xFF;
    let b = color & 0xFF;
    (r << FB_R_SHIFT.load(Ordering::Relaxed))
        | (g << FB_G_SHIFT.load(Ordering::Relaxed))
        | (b << FB_B_SHIFT.load(Ordering::Relaxed))
}
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

/// Current framebuffer base virtual address (the write-combining VA on x86 once the
/// WC remap engages, else the bootloader's mapping). Diagnostic.
pub fn base() -> u64 {
    FB_ADDR.load(Ordering::Acquire)
}

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

    // The bootloader maps the framebuffer UNCACHED (no PAT/MTRR set up). On a large panel
    // (a 2880x1800 Retina is ~20 MB) every full-frame write is tens of MB of uncached MMIO,
    // so the engine livelocks painting at `spawn_shell` (seen on a 2016 Intel Mac; a small
    // panel like the ProBook's is cheap, which is why it booted). Remap the fb as
    // write-combining — fb writes go from ~tens-of-MB/s to GB/s, scaling away with panel
    // size. The [fb-diag] log reports the original cache bits (PCD=1 ⇒ was uncached).
    #[cfg(target_arch = "x86_64")]
    let addr = {
        let hhdm = crate::mm::frame_allocator::hhdm_offset();
        let phys = addr.wrapping_sub(hhdm);
        if let Some((e, psz)) = crate::mm::paging::leaf_entry_of(addr) {
            log::error!(
                "[fb-diag] va={:#x} phys={:#x} PWT={} PCD={} PAT={} page={}KiB",
                addr, phys, (e >> 3) & 1, (e >> 4) & 1, (e >> 7) & 1, psz / 1024
            );
        }
        crate::mm::paging::setup_pat_wc();
        let fb_bytes = (fb.pitch * fb.height) as usize;
        match unsafe { crate::mm::paging::map_write_combining(phys, fb_bytes) } {
            Some(wc) => {
                log::error!(
                    "[fb-wc] write-combining remap phys={:#x} -> va={:#x} ({} KiB)",
                    phys, wc, fb_bytes / 1024
                );
                wc
            }
            None => {
                log::warn!("[fb-wc] WC remap failed; using uncached fb (slow on large panels)");
                addr
            }
        }
    };

    // Read the firmware's channel order. Log it unconditionally so a real
    // machine's GOP format is visible on the serial console for diagnosis.
    let (rs, gs, bs) = (fb.red_mask_shift as u32, fb.green_mask_shift as u32, fb.blue_mask_shift as u32);
    log::info!(
        "[fb] GOP {}x{} bpp={} pitch={} model={} shifts r={} g={} b={}",
        fb.width, fb.height, fb.bpp, fb.pitch, fb.memory_model, rs, gs, bs
    );

    // Only 32 bpp is supported by this console (24bpp byte-packing is a TODO).
    if fb.bpp != 32 { log::warn!("[fb] unsupported bpp {} — no UI", fb.bpp); return; }

    FB_R_SHIFT.store(rs, Ordering::Release);
    FB_G_SHIFT.store(gs, Ordering::Release);
    FB_B_SHIFT.store(bs, Ordering::Release);
    // Fast-path only when the firmware is exactly XRGB. Otherwise repack at write.
    FB_XRGB_FAST.store(rs == 16 && gs == 8 && bs == 0, Ordering::Release);

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

/// Initialise the framebuffer console from raw parameters (arch-neutral).
///
/// The x86 path feeds the framebuffer from the Limine response (`init` above);
/// architectures without a bootloader framebuffer (e.g. aarch64 QEMU ramfb, set
/// up by `arch::aarch64::ramfb`) call this with the buffer they configured. The
/// `addr` must be a directly-writable virtual address (identity-mapped on ARM,
/// so VA == PA) and `pitch_bytes` is the number of bytes per row. Only 32 bpp
/// (XRGB) is supported, matching the rest of this console.
pub fn init_raw(addr: u64, width: u32, height: u32, pitch_bytes: u32) {
    if addr == 0 || width == 0 || height == 0 || pitch_bytes < width * 4 {
        return;
    }
    FB_ADDR    .store(addr,            Ordering::Release);
    FB_PITCH_PX.store(pitch_bytes / 4, Ordering::Release);
    FB_WIDTH   .store(width,           Ordering::Release);
    FB_HEIGHT  .store(height,          Ordering::Release);
    FB_COLS    .store(width / CHAR_W,  Ordering::Release);
    FB_ROWS    .store(height / CHAR_H, Ordering::Release);

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
    let _guard = InterruptGuard::new();
    for b in s.bytes() {
        write_byte(b);
    }
}

/// Disable framebuffer console text logging (silences write_str).
pub fn disable_fb_logging() {
    FB_SILENT.store(true, Ordering::Release);
}

/// Re-enable framebuffer console text logging (used by the verbose-log toggle
/// and the panic handler so failures are always visible on screen).
pub fn enable_fb_logging() {
    FB_SILENT.store(false, Ordering::Release);
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
    unsafe { addr.add(y as usize * pitch_px + x as usize).write_volatile(fb_pack(color)); }
}

/// Read a single pixel at `(x, y)` from the framebuffer or double buffer.
/// Returns 0 if not ready or out of bounds.
pub fn get_pixel(x: u32, y: u32) -> u32 {
    if !is_ready() { return 0; }
    let width  = FB_WIDTH.load(Ordering::Relaxed);
    let height = FB_HEIGHT.load(Ordering::Relaxed);
    if x >= width || y >= height { return 0; }
    let pitch_px = FB_PITCH_PX.load(Ordering::Relaxed) as usize;
    if DOUBLE_BUFFER_ACTIVE.load(Ordering::Relaxed) {
        let _guard = InterruptGuard::new();
        if let Some(ref buf) = *DOUBLE_BUFFER.lock() {
            return buf[y as usize * pitch_px + x as usize];
        }
    }
    let addr = FB_ADDR.load(Ordering::Relaxed) as *mut u32;
    unsafe { addr.add(y as usize * pitch_px + x as usize).read_volatile() }
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
    let packed = fb_pack(color);
    unsafe {
        for py in y0 as usize..y1 as usize {
            let row = addr.add(py * pitch_px);
            for px in x0 as usize..x1 as usize {
                row.add(px).write_volatile(packed);
            }
        }
    }
}

/// Draw one FONT glyph at pixel (px, py), magnified by `scale`, in `color`,
/// with a transparent background (only lit pixels are painted). Built on
/// `fill_rect` so it respects the active double buffer.
fn draw_glyph_px(ch: u8, px: i32, py: i32, scale: u32, color: u32) {
    if ch < 0x20 || ch >= 0x80 {
        return;
    }
    let glyph = &FONT[(ch - 0x20) as usize];
    let mut gy = 0u32;
    while gy < CHAR_H {
        let bits = glyph[gy as usize];
        let mut gx = 0u32;
        while gx < CHAR_W {
            if (bits >> (7 - gx)) & 1 != 0 {
                fill_rect(
                    px + (gx * scale) as i32,
                    py + (gy * scale) as i32,
                    scale,
                    scale,
                    color,
                );
            }
            gx += 1;
        }
        gy += 1;
    }
}

/// Draw an ASCII string centered horizontally, with its top at pixel row `py`,
/// magnified by `scale`, in `color`.
fn draw_text_centered(s: &[u8], py: i32, scale: u32, color: u32) {
    let width = FB_WIDTH.load(Ordering::Relaxed) as i32;
    let cw = (CHAR_W * scale) as i32;
    let total = s.len() as i32 * cw;
    let mut x = (width - total) / 2;
    for &ch in s {
        draw_glyph_px(ch, x, py, scale, color);
        x += cw;
    }
}

/// Pixel width of `len` glyphs at `scale` (8 px per glyph cell).
pub fn text_width(len: usize, scale: u32) -> i32 {
    (len as u32 * CHAR_W * scale) as i32
}

/// Draw an ASCII string left-aligned at `(x, py)`, magnified by `scale`.
pub fn draw_text(s: &[u8], x: i32, py: i32, scale: u32, color: u32) {
    let cw = (CHAR_W * scale) as i32;
    let mut cx = x;
    for &ch in s {
        draw_glyph_px(ch, cx, py, scale, color);
        cx += cw;
    }
}

/// Pixel width of `len` glyphs rendered dot-matrix style with grid `cell`.
pub fn dotted_text_width(len: usize, cell: u32) -> i32 {
    (len as u32 * CHAR_W * cell) as i32
}

/// Render ASCII as a dot matrix (the "Doto" look): each lit pixel of the 8×8
/// glyph becomes a `dot`-sized square centered in a `cell`-sized grid cell.
/// `cell > dot` leaves the gaps between dots. Left-aligned at `(x, py)`.
pub fn draw_dotted_text(s: &[u8], x: i32, py: i32, cell: u32, dot: u32, color: u32) {
    let off = (cell.saturating_sub(dot) / 2) as i32;
    let cell_i = cell as i32;
    let char_adv = (CHAR_W * cell) as i32;
    let mut cx = x;
    for &ch in s {
        if (0x20..0x80).contains(&ch) {
            let glyph = &FONT[(ch - 0x20) as usize];
            let mut gy = 0u32;
            while gy < CHAR_H {
                let bits = glyph[gy as usize];
                let mut gx = 0u32;
                while gx < CHAR_W {
                    if (bits >> (7 - gx)) & 1 != 0 {
                        fill_rect(
                            cx + (gx as i32) * cell_i + off,
                            py + (gy as i32) * cell_i + off,
                            dot,
                            dot,
                            color,
                        );
                    }
                    gx += 1;
                }
                gy += 1;
            }
        }
        cx += char_adv;
    }
}

/// 256×256 8-bit alpha mask of the white OSCortex/Dotcorr logo mark, rasterised
/// from landing/public/dotcorr-logo-mark-white.svg at build-prep time.
static LOGO_MASK: &[u8; 256 * 256] = include_bytes!("../../assets/logo_mask.bin");
const LOGO_SRC: u32 = 256;

/// Boot splash, drawn by the compositor while no app surface has presented yet
/// (the Flutter engine's JIT warm-up). The white OSCortex logo, centered, scaled
/// to ~1/4 of the screen's shorter dimension — a consistent, Apple-boot-mark size
/// on any display. The caller paints the (black) background first; this blits the
/// mark over it. Static (white on black: black is power-friendly, the mark is
/// already white). A Flutter splash can't appear until the engine is ready, so
/// this has to be drawn by the kernel.
pub fn draw_boot_splash(_frame: u64) {
    let w = FB_WIDTH.load(Ordering::Relaxed);
    let h = FB_HEIGHT.load(Ordering::Relaxed);
    if w == 0 || h == 0 {
        return;
    }
    // Target box = 1/5 of the shorter screen side (clamped), centered — the same
    // proportion as the Apple boot mark. Nearest-neighbour scale of the 256² alpha
    // mask (which is cropped tight to the logo), so it's crisp and fully
    // resolution-independent.
    let target = (w.min(h) / 5).max(64);
    let ox = w.saturating_sub(target) / 2;
    let oy = h.saturating_sub(target) / 2;
    let mut ty = 0u32;
    while ty < target {
        let sy = (ty * LOGO_SRC) / target;
        let mut tx = 0u32;
        while tx < target {
            let sx = (tx * LOGO_SRC) / target;
            // The mark is white-on-transparent: alpha doubles as coverage.
            if LOGO_MASK[(sy * LOGO_SRC + sx) as usize] >= 110 {
                set_pixel(ox + tx, oy + ty, 0x00FF_FFFF); // white (XRGB8888)
            }
            tx += 1;
        }
        ty += 1;
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
                dst_base.add(dst_row + col).write_volatile(fb_pack(xrgb));
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
            // A full-fb back buffer is large on high-res panels — a 2880x1800
            // Retina framebuffer (e.g. a 2016 MacBook Pro) is ~20 MiB. Allocate
            // it FALLIBLY (and reject an implausible geometry) so a big fb or a
            // tight heap degrades to single-buffer / direct-to-fb rendering
            // instead of `vec!` aborting and hanging the boot at
            // cortex::compositor. Single-buffer is fully supported (every write
            // path checks DOUBLE_BUFFER_ACTIVE); it just tears.
            const MAX_DB_PIXELS: usize = 24 * 1024 * 1024; // ~96 MiB sanity ceiling
            let mut buf: Vec<u32> = Vec::new();
            if size == 0 || size > MAX_DB_PIXELS || buf.try_reserve_exact(size).is_err() {
                DOUBLE_BUFFER_ACTIVE.store(false, Ordering::SeqCst);
                log::error!(
                    "[fb] double-buffer unavailable (size={} px) — single-buffer mode",
                    size
                );
            } else {
                buf.resize(size, 0u32);
                *db = Some(buf);
            }
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
            if FB_XRGB_FAST.load(Ordering::Relaxed) {
                core::ptr::copy_nonoverlapping(buf.as_ptr(), addr, total);
            } else {
                // Non-XRGB firmware: repack each pixel into the panel's channel order.
                for i in 0..total {
                    addr.add(i).write_volatile(fb_pack(buf[i]));
                }
            }
        }
    }
}

// ── Internal helpers ──────────────────────────────────────────────────────────

fn write_byte(b: u8) {
    // The text console is a best-effort serial mirror. If the FB geometry is
    // unpublished or degenerate (e.g. a corrupted FB_ROWS/FB_COLS static), do
    // nothing rather than risk an arithmetic-overflow panic in the kernel.
    if FB_ROWS.load(Ordering::Relaxed) == 0 || FB_COLS.load(Ordering::Relaxed) == 0 {
        return;
    }
    match b {
        b'\r' => {
            CURSOR.lock().0 = 0;
        }
        b'\n' => {
            let mut cur = CURSOR.lock();
            cur.0 = 0;
            cur.1 += 1;
            let rows = FB_ROWS.load(Ordering::Relaxed);
            if rows > 0 && cur.1 >= rows {
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
    if glyph_idx >= FONT.len() { return; }
    let glyph = &FONT[glyph_idx];

    // Bound the cell to the framebuffer. A corrupted/overlarge col or row (e.g.
    // before the FB geometry is published, or if the CURSOR static is clobbered)
    // must NOT panic the kernel via an arithmetic overflow in `col * CHAR_W` —
    // the text console is only a serial mirror and is never worth a panic.
    let cols = if width  != 0 { width  / CHAR_W } else { 0 };
    let rows = if height != 0 { height / CHAR_H } else { 0 };
    if col >= cols || row >= rows { return; }

    let px0 = col.saturating_mul(CHAR_W);
    let py0 = row.saturating_mul(CHAR_H);

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
                    ptr.add(offset).write_volatile(fb_pack(color));
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

    // Guard against an unpublished/degenerate FB geometry: the serial-mirror
    // console must never panic the kernel (subtract-with-overflow on height or
    // rows). Bail out cleanly if there is nothing safe to scroll.
    if addr == 0 || height < CHAR_H || rows == 0 || pitch_px == 0 {
        return;
    }

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
            clear_start.add(i).write_volatile(fb_pack(BG));
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
            base.add(i).write_volatile(fb_pack(BG));
        }
    }
}

