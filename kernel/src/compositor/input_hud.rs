//! On-screen input debug HUD — opt-in via `--features input-hud`.
//!
//! Draws a compact panel each frame showing which input device bound and the
//! live pointer/key/scroll event counts plus the last pointer position. This is
//! the "eyes on it" tool for diagnosing input on a real VM or board where there
//! is no serial console:
//!   * `src=none`       → no input device bound at all.
//!   * `p=` never rises → events aren't reaching the kernel while you move/click.
//!   * `xy=` pinned off-screen → the cursor is being clamped or scaled wrong.
//!   * border red       → no device; cyan → a device is bound.
//!
//! Release builds (without the feature) compile `draw()` to a no-op, so the
//! shipped ISOs stay clean and pay zero cost.

/// Draw the input HUD over the current back buffer (call just before swap).
#[cfg(feature = "input-hud")]
pub fn draw() {
    use crate::drivers::fb;
    let (_sw, sh) = match fb::size_px() {
        Some(s) => s,
        None => return,
    };

    let (pc, kc, scnt) = crate::wm::input_counts();
    let (cx, cy) = crate::wm::cursor_pos();
    let btn = crate::wm::cursor_buttons();
    let (key, pressed) = crate::wm::last_key();
    let src = crate::wm::input_source_name();
    let bound = crate::wm::input_source() != crate::wm::INPUT_SRC_NONE;

    // line 1: device + per-kind event counters
    let mut l1 = Line::new();
    l1.s(b"INPUT src=");
    l1.s(src.as_bytes());
    l1.s(b" p=");
    l1.u(pc as u64);
    l1.s(b" k=");
    l1.u(kc as u64);
    l1.s(b" s=");
    l1.u(scnt as u64);

    // line 2: last pointer position / buttons / key
    let mut l2 = Line::new();
    l2.s(b"xy=");
    l2.i(cx);
    l2.b(b',');
    l2.i(cy);
    l2.s(b" btn=");
    l2.u(btn as u64);
    l2.s(b" key=0x");
    l2.hex(key as u8);
    l2.b(if pressed { b'+' } else { b'-' });

    let scale = 2u32;
    let line_h = (8 * scale + 3) as i32;
    let pad = 6i32;
    let tw = fb::text_width(l1.len, scale).max(fb::text_width(l2.len, scale));
    let panel_w = (tw + pad * 2) as u32;
    let panel_h = (line_h * 2 + pad * 2) as u32;
    let x0 = 8i32;
    let y0 = sh as i32 - panel_h as i32 - 8;

    fb::fill_rect(x0, y0, panel_w, panel_h, 0x00000000);
    let border = if bound { 0x0038BDF8u32 } else { 0x00FF3030u32 };
    fb::fill_rect(x0, y0, panel_w, 2, border);
    fb::fill_rect(x0, y0 + panel_h as i32 - 2, panel_w, 2, border);

    let tx = x0 + pad;
    fb::draw_text(l1.as_slice(), tx, y0 + pad, scale, 0x0038BDF8);
    fb::draw_text(l2.as_slice(), tx, y0 + pad + line_h, scale, 0x00FFFFFF);
}

/// No-op when the HUD feature is disabled (clean release builds).
#[cfg(not(feature = "input-hud"))]
#[inline(always)]
pub fn draw() {}

/// Fixed-capacity ASCII line builder — no heap, no `core::fmt` formatting cost.
#[cfg(feature = "input-hud")]
struct Line {
    buf: [u8; 96],
    len: usize,
}

#[cfg(feature = "input-hud")]
impl Line {
    fn new() -> Self {
        Self { buf: [0; 96], len: 0 }
    }
    fn as_slice(&self) -> &[u8] {
        &self.buf[..self.len]
    }
    fn b(&mut self, c: u8) {
        if self.len < self.buf.len() {
            self.buf[self.len] = c;
            self.len += 1;
        }
    }
    fn s(&mut self, s: &[u8]) {
        for &c in s {
            self.b(c);
        }
    }
    fn u(&mut self, mut n: u64) {
        if n == 0 {
            self.b(b'0');
            return;
        }
        let mut tmp = [0u8; 20];
        let mut i = 0;
        while n > 0 {
            tmp[i] = b'0' + (n % 10) as u8;
            n /= 10;
            i += 1;
        }
        while i > 0 {
            i -= 1;
            self.b(tmp[i]);
        }
    }
    fn i(&mut self, n: i32) {
        if n < 0 {
            self.b(b'-');
            self.u((-(n as i64)) as u64);
        } else {
            self.u(n as u64);
        }
    }
    fn hex(&mut self, n: u8) {
        let h = b"0123456789ABCDEF";
        self.b(h[(n >> 4) as usize]);
        self.b(h[(n & 0xF) as usize]);
    }
}
