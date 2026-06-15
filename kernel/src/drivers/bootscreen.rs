//! Boot screen — a clean, product-grade boot UI drawn by the kernel while the
//! Flutter engine warms up, instead of scrolling raw kernel logs.
//!
//! Layout (mirrors the DotCorr "Doto" design language):
//!   * a dot-matrix "OSCORTEX" wordmark, centered;
//!   * a small "<elapsed>s cold boot ●" caption to its right;
//!   * a thin progress bar (dark track, blue fill) that ramps as boot proceeds;
//!   * a green status line "cortex::<stage> · <elapsed>s".
//!
//! Kernel logs are silenced on the framebuffer by default (they still go to the
//! serial console for developers). Press **F2** to toggle a verbose overlay that
//! renders the recent kernel log ring on demand.

use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};

use crate::drivers::fb;
use crate::logger;

// ── palette (0x00RRGGBB) ─────────────────────────────────────────────────────
const BG: u32 = 0x0000_0000; // black — power-friendly
const DOT: u32 = 0x00C2_C6CE; // light grey wordmark dots
const CAPTION: u32 = 0x006A_6E78; // dim grey caption
const ACCENT: u32 = 0x003B_9EFF; // blue dot + progress fill
const TRACK: u32 = 0x001E_2129; // progress track
const STATUS: u32 = 0x0052_C88A; // green status line
const LOG_FG: u32 = 0x008A_9098; // verbose log text
const LOG_ERR: u32 = 0x00E0_5A4A; // verbose log error/warn text

static START_NS: AtomicU64 = AtomicU64::new(0);
static VERBOSE: AtomicBool = AtomicBool::new(false);
static DONE: AtomicBool = AtomicBool::new(false);
/// Explicit boot milestone (0 = none → fall back to the time-derived stage).
/// Set by `set_phase` at each major init step so the on-screen status names the
/// LAST step reached — when boot wedges (e.g. on real hardware QEMU can't
/// reproduce), the frozen splash shows exactly where it stopped.
static PHASE: AtomicU32 = AtomicU32::new(0);

/// Record the current boot milestone (see PHASE). Phases are listed in `stage()`.
pub fn set_phase(p: u32) {
    PHASE.store(p, Ordering::Release);
}

fn now_ns() -> u64 {
    crate::syscall::poll::monotonic_ns()
}

/// Record the boot start instant and silence on-screen log spam. Idempotent.
pub fn init() {
    START_NS
        .compare_exchange(0, now_ns().max(1), Ordering::AcqRel, Ordering::Relaxed)
        .ok();
    fb::disable_fb_logging();
}

/// Mark boot complete — the progress bar snaps to 100% and the status reads
/// `boot_completed`.
pub fn mark_done() {
    DONE.store(true, Ordering::Release);
}

pub fn is_verbose() -> bool {
    VERBOSE.load(Ordering::Acquire)
}

/// F2 handler: flip between the boot screen and the verbose log overlay. Verbose
/// mode raises the live log level to Info so the ring keeps filling; the default
/// stays Error so a normal boot is fast.
pub fn toggle_verbose() {
    let v = !VERBOSE.load(Ordering::Acquire);
    VERBOSE.store(v, Ordering::Release);
    log::set_max_level(if v {
        log::LevelFilter::Info
    } else {
        log::LevelFilter::Error
    });
}

fn elapsed_ns() -> u64 {
    let s = START_NS.load(Ordering::Relaxed);
    if s == 0 {
        return 0;
    }
    now_ns().saturating_sub(s)
}

/// Asymptotic ramp toward 92% (so it always looks like it's making progress
/// without knowing the total warm-up time), snapping to 100% once done.
fn percent() -> u32 {
    if DONE.load(Ordering::Acquire) {
        return 100;
    }
    let ms = elapsed_ns() / 1_000_000;
    ((92 * ms) / (ms + 2500)) as u32
}

fn stage() -> &'static [u8] {
    if DONE.load(Ordering::Acquire) {
        return b"boot_completed";
    }
    // Explicit milestone wins (set by set_phase at each init step). The frozen
    // splash then names the last step reached if boot wedges on real hardware.
    match PHASE.load(Ordering::Acquire) {
        0 => match percent() {
            0..=24 => b"starting",
            25..=54 => b"init_runtime",
            _ => b"warming_engine",
        },
        1 => b"drivers",
        2 => b"compositor",
        3 => b"scheduler",
        4 => b"ipc_wm",
        5 => b"filesystem",
        6 => b"block_net",
        7 => b"packages",
        8 => b"cortex",
        9 => b"smp",
        10 => b"spawn_shell",
        11 => b"enter_user",
        _ => b"warming_engine",
    }
}

/// Format `ns` as `"<sec>.<tenths>s"` into `buf`; returns the written slice.
fn fmt_secs(ns: u64, buf: &mut [u8]) -> usize {
    let tenths = ns / 100_000_000;
    let whole = tenths / 10;
    let frac = tenths % 10;
    let mut n = write_u64(whole, buf);
    if n < buf.len() {
        buf[n] = b'.';
        n += 1;
    }
    if n < buf.len() {
        buf[n] = b'0' + frac as u8;
        n += 1;
    }
    if n < buf.len() {
        buf[n] = b's';
        n += 1;
    }
    n
}

fn write_u64(mut v: u64, buf: &mut [u8]) -> usize {
    let mut tmp = [0u8; 20];
    let mut i = 0;
    if v == 0 {
        tmp[0] = b'0';
        i = 1;
    } else {
        while v > 0 && i < tmp.len() {
            tmp[i] = b'0' + (v % 10) as u8;
            v /= 10;
            i += 1;
        }
    }
    let mut n = 0;
    while n < i && n < buf.len() {
        buf[n] = tmp[i - 1 - n];
        n += 1;
    }
    n
}

fn append<'a>(buf: &'a mut [u8], at: usize, src: &[u8]) -> usize {
    let mut n = at;
    for &b in src {
        if n >= buf.len() {
            break;
        }
        buf[n] = b;
        n += 1;
    }
    n
}

/// Render one frame of the boot UI (or the verbose log overlay). Called each
/// vsync by the compositor while no app surface has presented yet, and once
/// early in boot so the screen is never blank.
pub fn render() {
    let (w, h) = match fb::size_px() {
        Some(d) => d,
        None => return,
    };
    fb::fill_rect(0, 0, w, h, BG);

    if VERBOSE.load(Ordering::Acquire) {
        render_logs(w, h);
    } else {
        render_boot(w, h);
    }
}

fn render_boot(w: u32, h: u32) {
    let wi = w as i32;
    let hi = h as i32;

    // Dot-matrix wordmark, ~45% of screen width.
    const MARK: &[u8] = b"OSCORTEX";
    let cell = ((w * 45 / 100) / (MARK.len() as u32 * 8)).clamp(6, 16);
    let dot = (cell * 3 / 5).max(3);
    let mark_w = fb::dotted_text_width(MARK.len(), cell);
    let mark_h = (8 * cell) as i32;
    let mark_x = (wi - mark_w) / 2;
    let mark_y = hi * 30 / 100;
    fb::draw_dotted_text(MARK, mark_x, mark_y, cell, dot, DOT);

    // "<elapsed>s cold boot ●" caption, to the right of the wordmark.
    let mut cap = [0u8; 32];
    let mut n = fmt_secs(elapsed_ns(), &mut cap);
    n = append(&mut cap, n, b" cold boot");
    let cap_scale = (cell / 5).max(1);
    let cap_x = mark_x + mark_w + (cell * 2) as i32;
    let cap_y = mark_y + (cell as i32); // near the top of the wordmark
    if cap_x + fb::text_width(n, cap_scale) + (8 * cap_scale) as i32 <= wi {
        fb::draw_text(&cap[..n], cap_x, cap_y, cap_scale, CAPTION);
        // accent dot ●
        let dot_px = (3 * cap_scale).max(3);
        let dx = cap_x + fb::text_width(n, cap_scale) + (cap_scale * 4) as i32;
        fb::fill_rect(dx, cap_y + (cap_scale) as i32, dot_px, dot_px, ACCENT);
    }

    // Progress bar, same width as the wordmark.
    let bar_h = (cell).max(4);
    let bar_x = mark_x;
    let bar_y = mark_y + mark_h + (cell * 4) as i32;
    fb::fill_rect(bar_x, bar_y, mark_w as u32, bar_h, TRACK);
    let fill = (mark_w as u32) * percent() / 100;
    if fill > 0 {
        fb::fill_rect(bar_x, bar_y, fill, bar_h, ACCENT);
    }

    // "cortex::<stage> · <elapsed>s" status line, centered below the bar. The
    // mid-dot separator isn't in the ASCII font, so it's drawn as a small square.
    let mut left = [0u8; 48];
    let mut ln = append(&mut left, 0, b"cortex::");
    ln = append(&mut left, ln, stage());
    let mut right = [0u8; 16];
    let rn = fmt_secs(elapsed_ns(), &mut right);

    let st_scale = (cell / 4).max(1);
    let gap = (6 * st_scale) as i32;
    let dot_sz = (2 * st_scale).max(2);
    let lw = fb::text_width(ln, st_scale);
    let rw = fb::text_width(rn, st_scale);
    let total = lw + gap + dot_sz as i32 + gap + rw;
    let mut x = (wi - total) / 2;
    let y = bar_y + bar_h as i32 + (cell * 3) as i32;
    fb::draw_text(&left[..ln], x, y, st_scale, STATUS);
    x += lw + gap;
    fb::fill_rect(x, y + (3 * st_scale) as i32, dot_sz, dot_sz, STATUS);
    x += dot_sz as i32 + gap;
    fb::draw_text(&right[..rn], x, y, st_scale, STATUS);
}

fn render_logs(w: u32, h: u32) {
    let mut lines = [[0u8; logger::RING_COLS]; logger::RING_LINES];
    let mut lens = [0u8; logger::RING_LINES];
    let count = logger::snapshot_logs(&mut lines, &mut lens);

    let scale = if w >= 1100 { 2 } else { 1 };
    let line_h = (8 * scale + 4) as i32;
    let pad = 16i32;

    fb::draw_text(b"VERBOSE  (F2 to hide)  -  full log: serial COM1", pad, pad, scale, STATUS);
    if count == 0 {
        fb::draw_text(
            b"(no kernel log lines captured yet)",
            pad,
            pad + line_h + 6,
            scale,
            LOG_FG,
        );
    }

    // Draw the most recent lines that fit below the header.
    let avail = ((h as i32 - pad * 2 - line_h) / line_h).max(0) as usize;
    let first = count.saturating_sub(avail);
    let mut y = pad + line_h + 6;
    for i in first..count {
        let line = &lines[i][..lens[i] as usize];
        let is_err = contains(line, b"[ERROR]") || contains(line, b"[WARN]");
        let color = if is_err { LOG_ERR } else { LOG_FG };
        fb::draw_text(line, pad, y, scale, color);
        y += line_h;
    }
}

fn contains(hay: &[u8], needle: &[u8]) -> bool {
    if needle.is_empty() || hay.len() < needle.len() {
        return false;
    }
    hay.windows(needle.len()).any(|w| w == needle)
}
