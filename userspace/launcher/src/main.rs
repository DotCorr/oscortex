//! OSCortex Desktop Launcher — Phase 61
//!
//! Graphical app launcher / desktop shell that replaces the plain text init
//! shell.  Runs as PID 1, draws directly to the framebuffer, and uses the
//! kernel compositor surface API when available.
//!
//! Layout (1280×720 reference):
//!   ┌─────────────────────────────────┐
//!   │  Taskbar  (top 40 px)           │
//!   ├─────────────────────────────────┤
//!   │                                 │
//!   │  App Grid (centred, icon tiles) │
//!   │                                 │
//!   ├─────────────────────────────────┤
//!   │  Dock  (bottom 56 px)           │
//!   └─────────────────────────────────┘
//!
//! Input: EV_KEY + EV_POINTER via SYS_WM_NEXT_EVENT.
//! Launch: SYS_EXEC_WAIT on the selected app path.
#![no_std]
#![no_main]
#![allow(dead_code)]

use core::arch::asm;

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    loop { unsafe { asm!("hlt"); } }
}

// ── Entry ─────────────────────────────────────────────────────────────────────
core::arch::global_asm!(
    ".section .text._start",
    ".global _start",
    "_start:",
    "   xor  rbp, rbp",
    "   and  rsp, -16",
    "   call launcher_main",
    "0: hlt",
    "   jmp  0b",
);

// ── Syscall numbers ───────────────────────────────────────────────────────────
const SYS_EXIT:               u64 = 60;
const SYS_EXEC:               u64 = 59;
const SYS_FB_MAP:             u64 = 0x377;
const SYS_WM_NEXT_EVENT:      u64 = 0x378;
const SYS_SCHED_YIELD:        u64 = 0x390;
const SYS_EXEC_WAIT:          u64 = 0x39E;
const SYS_SERIAL_WRITE:       u64 = 0x384;

// ── WM event kinds ────────────────────────────────────────────────────────────
const EV_KEY:     u32 = 3;
const EV_POINTER: u32 = 4;

// ── Colour palette ────────────────────────────────────────────────────────────
const C_BG:         u32 = 0x0D1117; // near-black
const C_TASKBAR:    u32 = 0x161B22; // dark panel
const C_DOCK:       u32 = 0x161B22;
const C_DOCK_GLOW:  u32 = 0x1F2937; // slightly lighter for hover
const C_ICON_BG:    u32 = 0x21262D;
const C_ICON_HOV:   u32 = 0x2D3748; // hovered icon
const C_ICON_SEL:   u32 = 0x1A3A5C; // selected / pressed
const C_ACCENT:     u32 = 0x3B82F6; // blue accent
const C_GREEN:      u32 = 0x3FB950;
const C_CYAN:       u32 = 0x79C0FF;
const C_ORANGE:     u32 = 0xF78166;
const C_PURPLE:     u32 = 0xC084FC;
const C_WHITE:      u32 = 0xE6EDF3;
const C_GREY:       u32 = 0x8B949E;
const C_TITLE_BG:   u32 = 0x1C2128;
const C_SEP:        u32 = 0x2D333B;

// ── ABI structs ───────────────────────────────────────────────────────────────
#[repr(C)]
struct FbInfo {
    addr:   u64,
    width:  u32,
    height: u32,
    pitch:  u32, // bytes per row
    bpp:    u32,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct WmEvent {
    seq:   u64,
    kind:  u32,
    flags: u32,
    a:     u64,
    b:     u64,
}

// ── Unsafe syscall wrappers ───────────────────────────────────────────────────
#[inline(always)]
unsafe fn syscall1(nr: u64, a0: u64) -> i64 {
    let r: i64;
    asm!("syscall",
         in("rax") nr, in("rdi") a0,
         out("rcx") _, out("r11") _, lateout("rax") r,
         options(nostack));
    r
}
#[inline(always)]
unsafe fn syscall2(nr: u64, a0: u64, a1: u64) -> i64 {
    let r: i64;
    asm!("syscall",
         in("rax") nr, in("rdi") a0, in("rsi") a1,
         out("rcx") _, out("r11") _, lateout("rax") r,
         options(nostack));
    r
}
#[inline(always)]
unsafe fn syscall0(nr: u64) -> i64 {
    let r: i64;
    asm!("syscall",
         in("rax") nr,
         out("rcx") _, out("r11") _, lateout("rax") r,
         options(nostack));
    r
}

#[inline(always)]
fn serial_write(msg: &[u8]) {
    unsafe {
        let _ = syscall2(SYS_SERIAL_WRITE, msg.as_ptr() as u64, msg.len() as u64);
    }
}

// ── Framebuffer helpers ───────────────────────────────────────────────────────
struct Fb {
    addr:     *mut u32,
    width:    u32,
    height:   u32,
    pitch_px: u32, // pitch in u32 words
}

impl Fb {
    #[inline(always)]
    fn set(&self, x: u32, y: u32, color: u32) {
        if x >= self.width || y >= self.height { return; }
        unsafe {
            self.addr.add((y * self.pitch_px + x) as usize).write_volatile(color);
        }
    }

    fn fill(&self, x: i32, y: i32, w: u32, h: u32, color: u32) {
        let x0 = x.max(0) as u32;
        let y0 = y.max(0) as u32;
        let x1 = ((x + w as i32) as u32).min(self.width);
        let y1 = ((y + h as i32) as u32).min(self.height);
        for py in y0..y1 {
            for px in x0..x1 {
                self.set(px, py, color);
            }
        }
    }

    // Draw a 1-px border rect
    fn border(&self, x: i32, y: i32, w: u32, h: u32, color: u32) {
        self.fill(x,                y,              w, 1, color);
        self.fill(x,                y + h as i32-1, w, 1, color);
        self.fill(x,                y,              1, h, color);
        self.fill(x + w as i32 - 1, y,              1, h, color);
    }

    fn draw_char(&self, x: u32, y: u32, ch: u8, fg: u32, bg: u32) {
        let glyph = FONT[(ch as usize).min(127)];
        for row in 0u32..8 {
            for col in 0u32..8 {
                let bit = (glyph[row as usize] >> (7 - col)) & 1;
                self.set(x + col, y + row, if bit != 0 { fg } else { bg });
            }
        }
    }

    fn draw_str(&self, x: u32, y: u32, s: &[u8], fg: u32, bg: u32) -> u32 {
        let mut cx = x;
        for &b in s {
            self.draw_char(cx, y, b, fg, bg);
            cx += 8;
        }
        cx
    }

    fn draw_str_centered(&self, y: u32, s: &[u8], fg: u32, bg: u32) {
        let tw = (s.len() as u32) * 8;
        let x  = if self.width > tw { (self.width - tw) / 2 } else { 0 };
        self.draw_str(x, y, s, fg, bg);
    }

    fn draw_icon_symbol(&self, cx: i32, cy: i32, kind: u8, color: u32) {
        // Simple geometric icons per kind.
        match kind {
            // Terminal: > prompt
            0 => {
                self.draw_char((cx - 8) as u32, (cy - 4) as u32, b'>', color, 0);
                self.draw_char((cx) as u32,     (cy - 4) as u32, b'_', color, 0);
            }
            // Files: folder shape (rect + tab)
            1 => {
                self.fill(cx - 12, cy - 8, 24, 16, color);
                self.fill(cx - 12, cy - 12, 10, 5, color);
            }
            // Settings: gear-ish (cross)
            2 => {
                self.fill(cx - 3, cy - 10, 6, 20, color);
                self.fill(cx - 10, cy - 3, 20, 6, color);
                self.fill(cx - 7, cy - 7, 4, 4, color);
                self.fill(cx + 3, cy - 7, 4, 4, color);
                self.fill(cx - 7, cy + 3, 4, 4, color);
                self.fill(cx + 3, cy + 3, 4, 4, color);
            }
            // Editor: pencil lines
            3 => {
                self.fill(cx - 12, cy - 8, 24, 2, color);
                self.fill(cx - 12, cy - 3, 18, 2, color);
                self.fill(cx - 12, cy + 2, 24, 2, color);
                self.fill(cx - 12, cy + 7, 12, 2, color);
            }
            // Browser: globe circle
            4 => {
                for r in 0..360u32 {
                    let rad = (r as f32) * 3.14159 / 180.0;
                    let px = cx + (approx_cos(rad) * 12.0) as i32;
                    let py = cy + (approx_sin(rad) * 12.0) as i32;
                    self.set(px as u32, py as u32, color);
                }
                self.fill(cx - 12, cy - 1, 24, 2, color);
                self.fill(cx - 1, cy - 12, 2, 24, color);
            }
            // About: "i" letter
            _ => {
                self.fill(cx - 2, cy - 10, 4, 4, color);
                self.fill(cx - 2, cy - 4, 4, 14, color);
            }
        }
    }
}

// Very rough sin/cos approximations (no libm).
fn approx_sin(x: f32) -> f32 {
    // Bhaskara I approximation: accurate enough for icon drawing
    let x = x % (2.0 * 3.14159);
    let x = if x > 3.14159 { x - 2.0 * 3.14159 } else { x };
    let sign = if x < 0.0 { -1.0f32 } else { 1.0f32 };
    let x = if x < 0.0 { -x } else { x };
    sign * (16.0 * x * (3.14159 - x)) / (5.0 * 3.14159 * 3.14159 - 4.0 * x * (3.14159 - x))
}
fn approx_cos(x: f32) -> f32 {
    approx_sin(x + 3.14159 / 2.0)
}

// ── App descriptor ────────────────────────────────────────────────────────────
struct App {
    name:   &'static [u8],
    path:   &'static [u8],
    color:  u32,
    icon:   u8,
}

const APPS: &[App] = &[
    App { name: b"Terminal",  path: b"/bin/init\0",     color: C_GREEN,  icon: 0 },
    App { name: b"Files",     path: b"/bin/files\0",    color: C_CYAN,   icon: 1 },
    App { name: b"Settings",  path: b"/bin/settings\0", color: C_GREY,   icon: 2 },
    App { name: b"Editor",    path: b"/bin/editor\0",   color: C_ORANGE, icon: 3 },
    App { name: b"Hello",     path: b"/bin/hello\0",    color: C_ACCENT, icon: 4 },
    App { name: b"Shell",     path: b"/bin/init\0",     color: C_PURPLE, icon: 5 },
];

// ── Tile geometry ─────────────────────────────────────────────────────────────
const TILE_W:  u32 = 120;
const TILE_H:  u32 = 130;
const TILE_GAP: u32 = 20;
const COLS:    u32 = 3;
const ROWS:    u32 = 2;
const TASKBAR_H: u32 = 40;
const DOCK_H:    u32 = 56;

// ── Launcher state ────────────────────────────────────────────────────────────
struct LauncherState {
    fb:        Fb,
    mouse_x:   i32,
    mouse_y:   i32,
    last_buttons: u32,
    hovered:   i32,  // -1 = none
    clicked:   i32,  // -1 = none (transient flash frame)
    grid_x0:   i32,
    grid_y0:   i32,
    needs_redraw: bool,
}

impl LauncherState {
    fn tile_rect(&self, idx: usize) -> (i32, i32, u32, u32) {
        let col = (idx as u32) % COLS;
        let row = (idx as u32) / COLS;
        let x = self.grid_x0 + (col * (TILE_W + TILE_GAP)) as i32;
        let y = self.grid_y0 + (row * (TILE_H + TILE_GAP)) as i32;
        (x, y, TILE_W, TILE_H)
    }

    fn hit_test(&self, mx: i32, my: i32) -> i32 {
        for i in 0..APPS.len() {
            let (x, y, w, h) = self.tile_rect(i);
            if mx >= x && mx < x + w as i32 && my >= y && my < y + h as i32 {
                return i as i32;
            }
        }
        -1
    }

    fn draw_taskbar(&self) {
        let fb = &self.fb;
        let w = fb.width;
        // Background
        fb.fill(0, 0, w, TASKBAR_H, C_TASKBAR);
        fb.fill(0, TASKBAR_H as i32 - 1, w, 1, C_SEP);

        // Logo dot
        fb.fill(10, 12, 16, 16, C_ACCENT);
        fb.fill(13, 15, 10, 10, C_BG);
        fb.fill(15, 17, 6, 6, C_ACCENT);

        // Title
        fb.draw_str(34, 16, b"OSCortex", C_WHITE, C_TASKBAR);

        // Right: "Desktop" label
        let label = b"Desktop";
        let lx = w.saturating_sub(label.len() as u32 * 8 + 16);
        fb.draw_str(lx, 16, label, C_GREY, C_TASKBAR);

        // Status bar: version string
        fb.draw_str(160, 16, b"Phase 61", C_GREY, C_TASKBAR);
    }

    fn draw_dock(&self) {
        let fb = &self.fb;
        let w  = fb.width;
        let h  = fb.height;
        let y  = h - DOCK_H;
        fb.fill(0, y as i32, w, DOCK_H, C_DOCK);
        fb.fill(0, y as i32, w, 1, C_SEP);

        // Dock items: quick-launch for first 3 apps
        let n    = APPS.len().min(4);
        let item = 48u32;
        let gap  = 16u32;
        let total_w = n as u32 * item + (n as u32 - 1) * gap;
        let start_x = ((w - total_w) / 2) as i32;
        for i in 0..n {
            let app = &APPS[i];
            let ix = start_x + (i as i32) * (item + gap) as i32;
            let iy = y as i32 + 4;
            fb.fill(ix, iy, item, item, C_ICON_BG);
            fb.border(ix, iy, item, item, C_SEP);
            let cx = ix + (item / 2) as i32;
            let cy = iy + (item / 2) as i32;
            fb.draw_icon_symbol(cx, cy, app.icon, app.color);
        }

        // Help text
        let hint = b"Click an app to launch   |   Esc to return";
        fb.draw_str_centered(y + DOCK_H - 14, hint, C_GREY, C_DOCK);
    }

    fn draw_grid(&self) {
        let fb = &self.fb;
        for i in 0..APPS.len() {
            let app = &APPS[i];
            let (tx, ty, tw, th) = self.tile_rect(i);
            let ii = i as i32;

            let bg = if self.clicked == ii {
                C_ICON_SEL
            } else if self.hovered == ii {
                C_ICON_HOV
            } else {
                C_ICON_BG
            };

            // Tile background
            fb.fill(tx, ty, tw, th, bg);

            // Coloured top accent bar
            fb.fill(tx, ty, tw, 3, app.color);

            // Border
            let border_col = if self.hovered == ii { app.color } else { C_SEP };
            fb.border(tx, ty, tw, th, border_col);

            // Icon symbol (centred horizontally, upper half)
            let cx = tx + (tw / 2) as i32;
            let cy = ty + 52i32;
            fb.draw_icon_symbol(cx, cy, app.icon, app.color);

            // App name (centred at bottom of tile)
            let name = app.name;
            let name_w = (name.len() as u32) * 8;
            let nx = tx + ((tw.saturating_sub(name_w)) / 2) as i32;
            fb.draw_str(nx as u32, (ty + th as i32 - 20) as u32, name, C_WHITE, bg);
        }
    }

    fn draw_background(&self) {
        let fb = &self.fb;
        let w  = fb.width;
        let h  = fb.height;
        // Gradient-ish: rows of slightly varying dark colour
        for y in TASKBAR_H..(h - DOCK_H) {
            let shade = C_BG.wrapping_add((y / 40) & 0x03);
            fb.fill(0, y as i32, w, 1, shade);
        }

        // Subtitle
        let sub = b"Welcome to OSCortex";
        let slen = sub.len() as u32 * 8;
        let sx = (w.saturating_sub(slen)) / 2;
        let sy = TASKBAR_H + 16;
        fb.draw_str(sx, sy, sub, C_GREY, C_BG);

        let ver = b"AI-First Operating System  |  Phase 61";
        let vlen = ver.len() as u32 * 8;
        let vx = (w.saturating_sub(vlen)) / 2;
        fb.draw_str(vx, sy + 12, ver, C_SEP, C_BG);
    }

    fn redraw_full(&mut self) {
        self.draw_background();
        self.draw_taskbar();
        self.draw_dock();
        self.draw_grid();
        self.draw_cursor();
        self.needs_redraw = false;
    }

    fn draw_cursor(&self) {
        let fb = &self.fb;
        let mx = self.mouse_x;
        let my = self.mouse_y;

        for dy in 0..12 {
            for dx in 0..dy + 1 {
                if dx < 12 {
                    let px = mx + dx;
                    let py = my + dy;
                    if px >= 0 && py >= 0 && px < fb.width as i32 && py < fb.height as i32 {
                        let is_border = dx == 0 || dx == dy || dy == 11;
                        let color = if is_border { 0x000000 } else { 0xFFFFFF };
                        fb.set(px as u32, py as u32, color);
                    }
                }
            }
        }
    }

    fn launch(&self, idx: usize) {
        if idx >= APPS.len() { return; }
        let app = &APPS[idx];
        serial_write(b"[launcher] launch start\n");
        // SYS_EXEC_WAIT: kernel spawns child and blocks us until it exits.
        let rc = unsafe {
            syscall2(SYS_EXEC_WAIT, app.path.as_ptr() as u64, (app.path.len() - 1) as u64)
        };
        if rc < 0 {
            serial_write(b"[launcher] launch failed\n");
        } else {
            serial_write(b"[launcher] app exited\n");
        }
        // When the child exits we get control back and redraw.
    }
}

// ── Main ──────────────────────────────────────────────────────────────────────
#[no_mangle]
pub extern "C" fn launcher_main() -> ! {
    // Map framebuffer.
    let mut fbinfo = FbInfo { addr: 0, width: 0, height: 0, pitch: 0, bpp: 0 };
    let r = unsafe { syscall1(SYS_FB_MAP, &mut fbinfo as *mut FbInfo as u64) };
    if r < 0 {
        loop { unsafe { asm!("hlt"); } }
    }

    let pitch_px = fbinfo.pitch / 4;
    let fb = Fb {
        addr:     fbinfo.addr as *mut u32,
        width:    fbinfo.width,
        height:   fbinfo.height,
        pitch_px,
    };

    // Grid centering
    let grid_total_w = COLS * TILE_W + (COLS - 1) * TILE_GAP;
    let grid_total_h = ROWS * TILE_H + (ROWS - 1) * TILE_GAP;
    let content_h    = fb.height - TASKBAR_H - DOCK_H;
    let grid_x0 = ((fb.width.saturating_sub(grid_total_w)) / 2) as i32;
    let grid_y0 = (TASKBAR_H as i32) + ((content_h.saturating_sub(grid_total_h)) / 2) as i32;

    let mut state = LauncherState {
        fb,
        mouse_x: 0,
        mouse_y: 0,
        last_buttons: 0,
        hovered: -1,
        clicked: -1,
        grid_x0,
        grid_y0,
        needs_redraw: true,
    };

    let mut ev = WmEvent::default();
    let mut click_flash: u32 = 0;

    loop {
        // Handle click flash timeout (redraw after brief selection highlight).
        if click_flash > 0 {
            click_flash -= 1;
            if click_flash == 0 {
                let launch_idx = state.clicked as usize;
                state.clicked = -1;
                state.needs_redraw = true;
                state.redraw_full();
                state.launch(launch_idx);
                state.needs_redraw = true;
                continue;
            }
        }

        if state.needs_redraw {
            state.redraw_full();
        }

        // Poll next WM event (non-blocking).
        let r = unsafe { syscall1(SYS_WM_NEXT_EVENT, &mut ev as *mut WmEvent as u64) };

        if r < 0 {
            // No event — yield and spin.
            unsafe { syscall0(SYS_SCHED_YIELD); }
            continue;
        }

        match ev.kind {
            EV_POINTER => {
                // flags = buttons, a = packed (x: u32 << 32 | y: u32)
                let px = (ev.a >> 32) as i32;
                let py = (ev.a & 0xFFFF_FFFF) as i32;
                let buttons = ev.flags;

                if px != state.mouse_x || py != state.mouse_y {
                    state.mouse_x = px;
                    state.mouse_y = py;
                    state.needs_redraw = true;
                }

                let prev_hov = state.hovered;
                state.hovered = state.hit_test(px, py);

                if state.hovered != prev_hov {
                    state.needs_redraw = true;
                }

                // Left button down-edge (bit 0).
                let left_down = (buttons & 1) != 0;
                let prev_left_down = (state.last_buttons & 1) != 0;
                if left_down && !prev_left_down && state.hovered >= 0 && state.clicked < 0 {
                    state.clicked = state.hovered;
                    state.needs_redraw = true;
                    click_flash = 3; // 3 event-loop iters of flash
                }
                state.last_buttons = buttons;
            }
            EV_KEY => {
                let pressed = ev.flags != 0;
                let sc = ev.a; // scancode is u64
                if pressed {
                    let mut moved = false;
                    let current = state.hovered;
                    if sc == 0x4D || sc == 0xE04D || sc == 0x20 { // Right / D
                        state.hovered = if current == -1 { 0 } else { (current + 1) % 6 };
                        moved = true;
                    } else if sc == 0x4B || sc == 0xE04B || sc == 0x1E { // Left / A
                        state.hovered = if current == -1 { 0 } else { if current > 0 { current - 1 } else { 5 } };
                        moved = true;
                    } else if sc == 0x50 || sc == 0xE050 || sc == 0x1F { // Down / S
                        state.hovered = if current == -1 { 0 } else { (current + 3) % 6 };
                        moved = true;
                    } else if sc == 0x48 || sc == 0xE048 || sc == 0x11 { // Up / W
                        state.hovered = if current == -1 { 0 } else { if current >= 3 { current - 3 } else { current + 3 } };
                        moved = true;
                    } else if sc == 0x1C || sc == 0xE01C || sc == 0x39 { // Enter / Numpad Enter / Space
                        if state.hovered >= 0 && state.clicked < 0 {
                            state.clicked = state.hovered;
                            state.needs_redraw = true;
                            click_flash = 3;
                        }
                    } else if sc == 0x01 {
                        // Escape — do nothing
                    }
                    if moved {
                        if state.hovered >= 0 {
                            let (tx, ty, tw, th) = state.tile_rect(state.hovered as usize);
                            state.mouse_x = tx + (tw as i32 / 2);
                            state.mouse_y = ty + (th as i32 / 2);
                        }
                        state.needs_redraw = true;
                    }
                }
            }
            _ => {}
        }
    }
}

// ── 8×8 bitmap font (printable ASCII 0x20–0x7E) ──────────────────────────────
// Each entry is 8 bytes, one per row, MSB = leftmost pixel.
static FONT: [[u8; 8]; 128] = include_font();

const fn include_font() -> [[u8; 8]; 128] {
    let mut f = [[0u8; 8]; 128];
    // Space
    // ! "  #  $  %  &  '  (  )
    // Inline a minimal subset for the strings we actually draw.
    // Full 8×8 PC-BIOS font encoded inline.
    macro_rules! g {
        ($c:literal, $b0:literal,$b1:literal,$b2:literal,$b3:literal,
                     $b4:literal,$b5:literal,$b6:literal,$b7:literal) => {
            f[$c] = [$b0,$b1,$b2,$b3,$b4,$b5,$b6,$b7];
        };
    }
    // Numbers
    g!(0x30,0x3C,0x66,0x6E,0x76,0x66,0x66,0x3C,0x00); // 0
    g!(0x31,0x18,0x38,0x18,0x18,0x18,0x18,0x7E,0x00); // 1
    g!(0x32,0x3C,0x66,0x06,0x1C,0x30,0x66,0x7E,0x00); // 2
    g!(0x33,0x3C,0x66,0x06,0x1C,0x06,0x66,0x3C,0x00); // 3
    g!(0x34,0x0E,0x1E,0x36,0x66,0x7F,0x06,0x06,0x00); // 4
    g!(0x35,0x7E,0x60,0x7C,0x06,0x06,0x66,0x3C,0x00); // 5
    g!(0x36,0x1C,0x30,0x60,0x7C,0x66,0x66,0x3C,0x00); // 6
    g!(0x37,0x7E,0x66,0x0C,0x18,0x18,0x18,0x18,0x00); // 7
    g!(0x38,0x3C,0x66,0x66,0x3C,0x66,0x66,0x3C,0x00); // 8
    g!(0x39,0x3C,0x66,0x66,0x3E,0x06,0x0C,0x38,0x00); // 9
    // Uppercase
    g!(0x41,0x18,0x3C,0x66,0x7E,0x66,0x66,0x66,0x00); // A
    g!(0x42,0x7C,0x66,0x66,0x7C,0x66,0x66,0x7C,0x00); // B
    g!(0x43,0x3C,0x66,0x60,0x60,0x60,0x66,0x3C,0x00); // C
    g!(0x44,0x78,0x6C,0x66,0x66,0x66,0x6C,0x78,0x00); // D
    g!(0x45,0x7E,0x60,0x60,0x78,0x60,0x60,0x7E,0x00); // E
    g!(0x46,0x7E,0x60,0x60,0x78,0x60,0x60,0x60,0x00); // F
    g!(0x47,0x3C,0x66,0x60,0x6E,0x66,0x66,0x3C,0x00); // G
    g!(0x48,0x66,0x66,0x66,0x7E,0x66,0x66,0x66,0x00); // H
    g!(0x49,0x3C,0x18,0x18,0x18,0x18,0x18,0x3C,0x00); // I
    g!(0x4A,0x1E,0x0C,0x0C,0x0C,0x0C,0x6C,0x38,0x00); // J
    g!(0x4B,0x66,0x6C,0x78,0x70,0x78,0x6C,0x66,0x00); // K
    g!(0x4C,0x60,0x60,0x60,0x60,0x60,0x60,0x7E,0x00); // L
    g!(0x4D,0x63,0x77,0x7F,0x6B,0x63,0x63,0x63,0x00); // M
    g!(0x4E,0x66,0x76,0x7E,0x7E,0x6E,0x66,0x66,0x00); // N
    g!(0x4F,0x3C,0x66,0x66,0x66,0x66,0x66,0x3C,0x00); // O
    g!(0x50,0x7C,0x66,0x66,0x7C,0x60,0x60,0x60,0x00); // P
    g!(0x51,0x3C,0x66,0x66,0x66,0x6E,0x3C,0x06,0x00); // Q
    g!(0x52,0x7C,0x66,0x66,0x7C,0x6C,0x66,0x66,0x00); // R
    g!(0x53,0x3C,0x66,0x60,0x3C,0x06,0x66,0x3C,0x00); // S
    g!(0x54,0x7E,0x18,0x18,0x18,0x18,0x18,0x18,0x00); // T
    g!(0x55,0x66,0x66,0x66,0x66,0x66,0x66,0x3C,0x00); // U
    g!(0x56,0x66,0x66,0x66,0x66,0x66,0x3C,0x18,0x00); // V
    g!(0x57,0x63,0x63,0x63,0x6B,0x7F,0x77,0x63,0x00); // W
    g!(0x58,0x66,0x66,0x3C,0x18,0x3C,0x66,0x66,0x00); // X
    g!(0x59,0x66,0x66,0x66,0x3C,0x18,0x18,0x18,0x00); // Y
    g!(0x5A,0x7E,0x06,0x0C,0x18,0x30,0x60,0x7E,0x00); // Z
    // Lowercase
    g!(0x61,0x00,0x00,0x3C,0x06,0x3E,0x66,0x3E,0x00); // a
    g!(0x62,0x60,0x60,0x7C,0x66,0x66,0x66,0x7C,0x00); // b
    g!(0x63,0x00,0x00,0x3C,0x66,0x60,0x66,0x3C,0x00); // c
    g!(0x64,0x06,0x06,0x3E,0x66,0x66,0x66,0x3E,0x00); // d
    g!(0x65,0x00,0x00,0x3C,0x66,0x7E,0x60,0x3C,0x00); // e
    g!(0x66,0x1C,0x30,0x30,0x7C,0x30,0x30,0x30,0x00); // f
    g!(0x67,0x00,0x00,0x3E,0x66,0x66,0x3E,0x06,0x7C); // g
    g!(0x68,0x60,0x60,0x7C,0x66,0x66,0x66,0x66,0x00); // h
    g!(0x69,0x18,0x00,0x38,0x18,0x18,0x18,0x3C,0x00); // i
    g!(0x6A,0x0C,0x00,0x1C,0x0C,0x0C,0x0C,0x6C,0x38); // j
    g!(0x6B,0x60,0x60,0x66,0x6C,0x78,0x6C,0x66,0x00); // k
    g!(0x6C,0x38,0x18,0x18,0x18,0x18,0x18,0x3C,0x00); // l
    g!(0x6D,0x00,0x00,0x63,0x77,0x7F,0x6B,0x63,0x00); // m
    g!(0x6E,0x00,0x00,0x7C,0x66,0x66,0x66,0x66,0x00); // n
    g!(0x6F,0x00,0x00,0x3C,0x66,0x66,0x66,0x3C,0x00); // o
    g!(0x70,0x00,0x00,0x7C,0x66,0x66,0x7C,0x60,0x60); // p
    g!(0x71,0x00,0x00,0x3E,0x66,0x66,0x3E,0x06,0x06); // q
    g!(0x72,0x00,0x00,0x6C,0x76,0x60,0x60,0x60,0x00); // r
    g!(0x73,0x00,0x00,0x3E,0x60,0x3C,0x06,0x7C,0x00); // s
    g!(0x74,0x30,0x30,0x7C,0x30,0x30,0x30,0x1C,0x00); // t
    g!(0x75,0x00,0x00,0x66,0x66,0x66,0x66,0x3E,0x00); // u
    g!(0x76,0x00,0x00,0x66,0x66,0x66,0x3C,0x18,0x00); // v
    g!(0x77,0x00,0x00,0x63,0x6B,0x7F,0x77,0x63,0x00); // w
    g!(0x78,0x00,0x00,0x66,0x3C,0x18,0x3C,0x66,0x00); // x
    g!(0x79,0x00,0x00,0x66,0x66,0x66,0x3E,0x06,0x7C); // y
    g!(0x7A,0x00,0x00,0x7E,0x0C,0x18,0x30,0x7E,0x00); // z
    // Punctuation / symbols
    g!(0x20,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00); // space
    g!(0x21,0x18,0x18,0x18,0x18,0x00,0x00,0x18,0x00); // !
    g!(0x22,0x66,0x66,0x24,0x00,0x00,0x00,0x00,0x00); // "
    g!(0x23,0x6C,0x6C,0xFE,0x6C,0xFE,0x6C,0x6C,0x00); // #
    g!(0x24,0x18,0x7E,0x60,0x3C,0x06,0x7E,0x18,0x00); // $
    g!(0x25,0x00,0x66,0x0C,0x18,0x30,0x66,0x00,0x00); // %
    g!(0x26,0x38,0x6C,0x38,0x56,0x6C,0x6C,0x3A,0x00); // &
    g!(0x27,0x18,0x18,0x30,0x00,0x00,0x00,0x00,0x00); // '
    g!(0x28,0x0C,0x18,0x30,0x30,0x30,0x18,0x0C,0x00); // (
    g!(0x29,0x30,0x18,0x0C,0x0C,0x0C,0x18,0x30,0x00); // )
    g!(0x2A,0x00,0x66,0x3C,0xFF,0x3C,0x66,0x00,0x00); // *
    g!(0x2B,0x00,0x18,0x18,0x7E,0x18,0x18,0x00,0x00); // +
    g!(0x2C,0x00,0x00,0x00,0x00,0x00,0x18,0x18,0x30); // ,
    g!(0x2D,0x00,0x00,0x00,0x7E,0x00,0x00,0x00,0x00); // -
    g!(0x2E,0x00,0x00,0x00,0x00,0x00,0x18,0x18,0x00); // .
    g!(0x2F,0x03,0x06,0x0C,0x18,0x30,0x60,0x40,0x00); // /
    g!(0x3A,0x00,0x00,0x18,0x18,0x00,0x18,0x18,0x00); // :
    g!(0x3B,0x00,0x00,0x18,0x18,0x00,0x18,0x18,0x30); // ;
    g!(0x3C,0x06,0x0C,0x18,0x30,0x18,0x0C,0x06,0x00); // <
    g!(0x3D,0x00,0x00,0x7E,0x00,0x00,0x7E,0x00,0x00); // =
    g!(0x3E,0x60,0x30,0x18,0x0C,0x18,0x30,0x60,0x00); // >
    g!(0x3F,0x3C,0x66,0x06,0x0C,0x18,0x00,0x18,0x00); // ?
    g!(0x40,0x3C,0x66,0x6E,0x6A,0x6E,0x60,0x3C,0x00); // @
    g!(0x5B,0x3C,0x30,0x30,0x30,0x30,0x30,0x3C,0x00); // [
    g!(0x5C,0x60,0x30,0x18,0x0C,0x06,0x03,0x01,0x00); // backslash
    g!(0x5D,0x3C,0x0C,0x0C,0x0C,0x0C,0x0C,0x3C,0x00); // ]
    g!(0x5E,0x18,0x3C,0x66,0x00,0x00,0x00,0x00,0x00); // ^
    g!(0x5F,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0xFF); // _
    g!(0x60,0x30,0x18,0x0C,0x00,0x00,0x00,0x00,0x00); // `
    g!(0x7B,0x0E,0x18,0x18,0x70,0x18,0x18,0x0E,0x00); // {
    g!(0x7C,0x18,0x18,0x18,0x00,0x18,0x18,0x18,0x00); // |
    g!(0x7D,0x70,0x18,0x18,0x0E,0x18,0x18,0x70,0x00); // }
    g!(0x7E,0x00,0x00,0x00,0x76,0xDC,0x00,0x00,0x00); // ~
    f
}

