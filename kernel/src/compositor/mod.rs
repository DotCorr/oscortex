//! M13 compositor scaffold.
//!
//! This is the first plumbing layer for window/surface management before
//! Flutter embedder integration. It intentionally keeps a fixed-size table and
//! avoids heap allocation in early boot.

use alloc::vec;
use alloc::vec::Vec;
use spin::Mutex;
use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};

/// On-screen input debug HUD (opt-in via `--features input-hud`; no-op otherwise).
mod input_hud;

/// The mouse-cursor shape the compositor currently draws. Set from userspace
/// via `SYS_CURSOR_SHAPE_SET` when the Flutter framework activates a system
/// cursor (e.g. hovering a button → click/hand, hovering a text field → I-beam).
/// Values are the `CURSOR_SHAPE_*` constants in `embedder::abi`.
static ACTIVE_CURSOR_SHAPE: AtomicU32 = AtomicU32::new(0);

/// Set the active mouse-cursor shape drawn by the compositor overlay.
/// Returns `false` if `shape` is out of range.
pub fn set_cursor_shape(shape: u32) -> bool {
    if shape > crate::embedder::abi::CURSOR_SHAPE_MAX {
        return false;
    }
    ACTIVE_CURSOR_SHAPE.store(shape, Ordering::Release);
    // Repaint so the new pointer shape shows immediately even if Flutter's
    // surface content has not changed.
    invalidate();
    true
}

/// Current active cursor shape.
pub fn cursor_shape() -> u32 {
    ACTIVE_CURSOR_SHAPE.load(Ordering::Acquire)
}

/// When `true`, a userspace process owns the framebuffer directly.
/// `render_frame()` skips the background fill and placeholder blits.
static FB_BYPASS: AtomicBool = AtomicBool::new(false);
/// Set when input moves the cursor so we redraw the software pointer overlay.
static FRAME_DIRTY: AtomicBool = AtomicBool::new(false);

/// Request a compositor redraw (e.g. cursor moved but Flutter surface unchanged).
pub fn invalidate() {
    FRAME_DIRTY.store(true, Ordering::Release);
}

/// Set or clear the FB bypass flag.
pub fn set_fb_bypass(bypass: bool) {
    FB_BYPASS.store(bypass, Ordering::SeqCst);
}

/// Returns `true` if a userspace process currently owns the framebuffer.
pub fn is_fb_bypass() -> bool {
    FB_BYPASS.load(Ordering::SeqCst)
}

pub const MAX_SURFACES: usize = 32;
pub const MAX_SURFACES_PER_PID: usize = 8;
const MAX_SURFACE_PIXELS: usize = 1920 * 1080 * 2;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Surface {
    pub id: u32,
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    pub z: i32,
    pub visible: bool,
    pub clip_x: i32,
    pub clip_y: i32,
    pub clip_w: u32,
    pub clip_h: u32,
    pub damage_x: i32,
    pub damage_y: i32,
    pub damage_w: u32,
    pub damage_h: u32,
    pub has_damage: bool,
    pub in_use: bool,
}

impl Surface {
    const fn empty() -> Self {
        Self {
            id: 0,
            x: 0,
            y: 0,
            width: 0,
            height: 0,
            z: 0,
            visible: true,
            clip_x: 0,
            clip_y: 0,
            clip_w: 0,
            clip_h: 0,
            damage_x: 0,
            damage_y: 0,
            damage_w: 0,
            damage_h: 0,
            has_damage: false,
            in_use: false,
        }
    }
}

struct CompositorState {
    surfaces: [Surface; MAX_SURFACES],
    owners: [u32; MAX_SURFACES],
    buffers: [Option<Vec<u32>>; MAX_SURFACES],
    back_buffers: [Option<Vec<u32>>; MAX_SURFACES],
    presented: [bool; MAX_SURFACES],
    /// Phase 33-C: true when a new gpu_submit has written to back_buffers[i]
    /// but the compositor has not yet flipped it to the front at vsync.
    back_pending: [bool; MAX_SURFACES],
    next_id: u32,
    frame_counter: u64,
}

impl CompositorState {
    const fn new() -> Self {
        Self {
            surfaces: [const { Surface::empty() }; MAX_SURFACES],
            owners: [0; MAX_SURFACES],
            buffers: [const { None }; MAX_SURFACES],
            back_buffers: [const { None }; MAX_SURFACES],
            presented: [false; MAX_SURFACES],
            back_pending: [false; MAX_SURFACES],
            next_id: 1,
            frame_counter: 0,
        }
    }
}

static COMP: Mutex<CompositorState> = Mutex::new(CompositorState::new());

pub fn init() {
    log::info!("[Compositor] online (max_surfaces={})", MAX_SURFACES);

    if crate::drivers::fb::is_ready() {
        // Boot text stops here on the QEMU window — serial continues. Leave a
        // visible hint so this is not mistaken for a hang.
        crate::drivers::fb::write_str("\n  OSCortex: loading Flutter shell...\n  (boot logs continue on serial)\n\n");
        crate::drivers::fb::set_double_buffer(true);
        crate::drivers::fb::disable_fb_logging();
    }
}

pub fn create_surface(width: u32, height: u32) -> Result<u32, &'static str> {
    create_surface_for(0, width, height)
}

pub fn create_surface_for(owner_pid: u32, width: u32, height: u32) -> Result<u32, &'static str> {
    create_surface_internal(owner_pid, width, height)
}

fn create_surface_internal(owner_pid: u32, width: u32, height: u32) -> Result<u32, &'static str> {
    if width == 0 || height == 0 {
        return Err("invalid size");
    }
    let pixels = width as usize * height as usize;
    if pixels > MAX_SURFACE_PIXELS {
        return Err("surface too large");
    }

    let mut c = COMP.lock();
    // Enforce per-pid surface limit.
    let owner_leader = if owner_pid != 0 {
        crate::process::get_group_leader(owner_pid)
    } else {
        0
    };
    if owner_leader != 0 {
        let owned = c.surfaces.iter().enumerate()
            .filter(|(idx, s)| s.in_use && c.owners[*idx] == owner_leader)
            .count();
        if owned >= MAX_SURFACES_PER_PID {
            return Err("per-pid surface limit reached");
        }
    }
    let id = c.next_id;
    let mut allocated_idx = None;
    for (idx, s) in c.surfaces.iter_mut().enumerate() {
        if !s.in_use {
            *s = Surface {
                id,
                x: 0,
                y: 0,
                width,
                height,
                z: 0,
                visible: true,
                clip_x: 0,
                clip_y: 0,
                clip_w: width,
                clip_h: height,
                damage_x: 0,
                damage_y: 0,
                damage_w: 0,
                damage_h: 0,
                has_damage: false,
                in_use: true,
            };
            c.owners[idx] = owner_leader;
            c.buffers[idx] = Some(vec![0u32; pixels]);
            c.presented[idx] = false;
            c.next_id = c.next_id.wrapping_add(1).max(1);
            allocated_idx = Some(idx);
            break;
        }
    }

    if let Some(_idx) = allocated_idx {
        Ok(id)
    } else {
        Err("surface table full")
    }
}

pub fn move_surface(id: u32, x: i32, y: i32, z: i32) -> Result<(), &'static str> {
    move_surface_for(0, id, x, y, z)
}

pub fn move_surface_for(caller_pid: u32, id: u32, x: i32, y: i32, z: i32) -> Result<(), &'static str> {
    let mut c = COMP.lock();
    let idx = match c.surfaces.iter().position(|s| s.in_use && s.id == id) {
        Some(v) => v,
        None => return Err("no such surface"),
    };
    if caller_pid != 0 {
        let caller_leader = crate::process::get_group_leader(caller_pid);
        if c.owners[idx] != caller_leader {
            return Err("permission denied");
        }
    }
    c.surfaces[idx].x = x;
    c.surfaces[idx].y = y;
    c.surfaces[idx].z = z;
    Ok(())
}

pub fn destroy_surface(id: u32) -> Result<(), &'static str> {
    destroy_surface_for(0, id)
}

pub fn destroy_surface_for(caller_pid: u32, id: u32) -> Result<(), &'static str> {
    let mut c = COMP.lock();
    let idx = match c.surfaces.iter().position(|s| s.in_use && s.id == id) {
        Some(v) => v,
        None => return Err("no such surface"),
    };
    if caller_pid != 0 {
        let caller_leader = crate::process::get_group_leader(caller_pid);
        if c.owners[idx] != caller_leader {
            return Err("permission denied");
        }
    }
    c.surfaces[idx] = Surface::empty();
    c.owners[idx] = 0;
    c.buffers[idx] = None;
    c.back_buffers[idx] = None;
    c.back_pending[idx] = false;
    c.presented[idx] = false;
    Ok(())
}

pub fn active_surfaces() -> usize {
    let c = COMP.lock();
    c.surfaces.iter().filter(|s| s.in_use).count()
}

pub fn surface_owner(id: u32) -> Option<u32> {
    let c = COMP.lock();
    let idx = c.surfaces.iter().position(|s| s.in_use && s.id == id)?;
    Some(c.owners[idx])
}

pub fn surface_z_get(id: u32) -> Option<i32> {
    let c = COMP.lock();
    let surf = c.surfaces.iter().find(|s| s.in_use && s.id == id)?;
    Some(surf.z)
}

pub fn surface_z_set_for(caller_pid: u32, id: u32, z: i32) -> Result<(), &'static str> {
    let mut c = COMP.lock();
    let idx = match c.surfaces.iter().position(|s| s.in_use && s.id == id) {
        Some(v) => v,
        None => return Err("no such surface"),
    };
    if caller_pid != 0 {
        let caller_leader = crate::process::get_group_leader(caller_pid);
        if c.owners[idx] != caller_leader {
            return Err("permission denied");
        }
    }
    c.surfaces[idx].z = z;
    Ok(())
}

/// Find the topmost (highest z-order) surface at point (px, py).
/// Returns (surface_id, owner_pid) for hit-testing; None if no surface hit.
pub fn surface_at_point(px: i32, py: i32) -> Option<(u32, u32)> {
    let c = COMP.lock();
    let mut best_z = i32::MIN;
    let mut result = None;
    
    for (idx, surf) in c.surfaces.iter().enumerate() {
        if !surf.in_use {
            continue;
        }
        // Hit test: is point within surface bounds?
        if px >= surf.x && px < (surf.x + surf.width as i32) &&
           py >= surf.y && py < (surf.y + surf.height as i32) {
            // Prefer higher z-order.
            if surf.z > best_z {
                best_z = surf.z;
                result = Some((surf.id, c.owners[idx]));
            }
        }
    }
    result
}

/// Get surface geometry: returns packed (x, y, width, height).
/// Format: ((x as u32) << 48) | ((y as u32) << 32) | (width << 16) | height.
/// Since all are positive, we can represent: x/y as i32 but stored as u32 reinterpreted.
pub fn surface_geometry_get(id: u32) -> Option<(i32, i32, u32, u32)> {
    let c = COMP.lock();
    let surf = c.surfaces.iter().find(|s| s.in_use && s.id == id)?;
    Some((surf.x, surf.y, surf.width, surf.height))
}

/// Set surface geometry with owner enforcement.
pub fn surface_geometry_set_for(caller_pid: u32, id: u32, x: i32, y: i32, width: u32, height: u32) -> Result<(), &'static str> {
    if width == 0 || height == 0 {
        return Err("invalid size");
    }
    let pixels = width as usize * height as usize;
    if pixels > MAX_SURFACE_PIXELS {
        return Err("surface too large");
    }
    
    let mut c = COMP.lock();
    let idx = match c.surfaces.iter().position(|s| s.in_use && s.id == id) {
        Some(v) => v,
        None => return Err("no such surface"),
    };
    if caller_pid != 0 {
        let caller_leader = crate::process::get_group_leader(caller_pid);
        if c.owners[idx] != caller_leader {
            return Err("permission denied");
        }
    }
    c.surfaces[idx].x = x;
    c.surfaces[idx].y = y;
    c.surfaces[idx].width = width;
    c.surfaces[idx].height = height;
    Ok(())
}

pub fn surface_visibility_get(id: u32) -> Option<bool> {
    let c = COMP.lock();
    let surf = c.surfaces.iter().find(|s| s.in_use && s.id == id)?;
    Some(surf.visible)
}

pub fn surface_visibility_set_for(caller_pid: u32, id: u32, visible: bool) -> Result<(), &'static str> {
    let mut c = COMP.lock();
    let idx = match c.surfaces.iter().position(|s| s.in_use && s.id == id) {
        Some(v) => v,
        None => return Err("no such surface"),
    };
    if caller_pid != 0 {
        let caller_leader = crate::process::get_group_leader(caller_pid);
        if c.owners[idx] != caller_leader {
            return Err("permission denied");
        }
    }
    c.surfaces[idx].visible = visible;
    Ok(())
}

pub fn surface_clip_set_for(caller_pid: u32, id: u32, x: i32, y: i32, w: u32, h: u32) -> Result<(), &'static str> {
    let mut c = COMP.lock();
    let idx = match c.surfaces.iter().position(|s| s.in_use && s.id == id) {
        Some(v) => v,
        None => return Err("no such surface"),
    };
    if caller_pid != 0 {
        let caller_leader = crate::process::get_group_leader(caller_pid);
        if c.owners[idx] != caller_leader {
            return Err("permission denied");
        }
    }
    // Clamp clip region to surface bounds.
    let s = &c.surfaces[idx];
    let max_w = s.width as i32;
    let max_h = s.height as i32;
    let clip_x = x.max(0).min(max_w) as i32;
    let clip_y = y.max(0).min(max_h) as i32;
    let clip_w = w.min((max_w - clip_x) as u32);
    let clip_h = h.min((max_h - clip_y) as u32);
    
    c.surfaces[idx].clip_x = clip_x;
    c.surfaces[idx].clip_y = clip_y;
    c.surfaces[idx].clip_w = clip_w;
    c.surfaces[idx].clip_h = clip_h;
    Ok(())
}

/// Return the number of surfaces currently owned by the given PID's thread group.
pub fn surface_count_for(pid: u32) -> usize {
    let leader = if pid != 0 {
        crate::process::get_group_leader(pid)
    } else {
        0
    };
    let c = COMP.lock();
    c.surfaces.iter().enumerate()
        .filter(|(idx, s)| s.in_use && c.owners[*idx] == leader)
        .count()
}

/// Set the dirty/damage rectangle for a surface (owner-enforced).
/// Coordinates are clamped to surface bounds.
pub fn surface_damage_set_for(caller_pid: u32, id: u32, x: i32, y: i32, w: u32, h: u32) -> Result<(), &'static str> {
    let mut c = COMP.lock();
    let idx = match c.surfaces.iter().position(|s| s.in_use && s.id == id) {
        Some(v) => v,
        None => return Err("no such surface"),
    };
    if caller_pid != 0 {
        let caller_leader = crate::process::get_group_leader(caller_pid);
        if c.owners[idx] != caller_leader {
            return Err("permission denied");
        }
    }
    let s = &c.surfaces[idx];
    let max_w = s.width as i32;
    let max_h = s.height as i32;
    let dx = x.max(0).min(max_w);
    let dy = y.max(0).min(max_h);
    let dw = w.min((max_w - dx) as u32);
    let dh = h.min((max_h - dy) as u32);
    c.surfaces[idx].damage_x = dx;
    c.surfaces[idx].damage_y = dy;
    c.surfaces[idx].damage_w = dw;
    c.surfaces[idx].damage_h = dh;
    c.surfaces[idx].has_damage = dw > 0 && dh > 0;
    Ok(())
}

/// Query damage rect; returns (x, y, w, h, has_damage).
pub fn surface_damage_get(id: u32) -> Option<(i32, i32, u32, u32, bool)> {
    let c = COMP.lock();
    let surf = c.surfaces.iter().find(|s| s.in_use && s.id == id)?;
    Some((surf.damage_x, surf.damage_y, surf.damage_w, surf.damage_h, surf.has_damage))
}

/// Atomically swap the back buffer to front (double-buffer flip).
/// Lazily allocates the back buffer on the first call.
pub fn surface_flip_for(caller_pid: u32, id: u32) -> Result<(), &'static str> {
    {
        let mut c = COMP.lock();
        let idx = match c.surfaces.iter().position(|s| s.in_use && s.id == id) {
            Some(v) => v,
            None => return Err("no such surface"),
        };
        if caller_pid != 0 {
            let caller_leader = crate::process::get_group_leader(caller_pid);
            if c.owners[idx] != caller_leader {
                return Err("permission denied");
            }
        }
        let pixels = c.surfaces[idx].width as usize * c.surfaces[idx].height as usize;
        // Lazy back-buffer allocation: copy front on first flip.
        if c.back_buffers[idx].is_none() {
            let copy = c.buffers[idx]
                .as_ref()
                .map(|b| b.clone())
                .unwrap_or_else(|| vec![0u32; pixels]);
            c.back_buffers[idx] = Some(copy);
        }
        // Atomic front/back swap using take() to satisfy borrow checker.
        let front = c.buffers[idx].take();
        let back = c.back_buffers[idx].take();
        c.buffers[idx] = back;
        c.back_buffers[idx] = front;
        c.presented[idx] = true;
    }
    render_frame();
    Ok(())
}

/// Upload full RGBA32 payload into a surface backbuffer.
pub fn upload_surface_rgba32(id: u32, payload: &[u8]) -> Result<(), &'static str> {
    upload_surface_rgba32_for(0, id, payload)
}

pub fn upload_surface_rgba32_for(caller_pid: u32, id: u32, payload: &[u8]) -> Result<(), &'static str> {
    let mut c = COMP.lock();
    for (idx, s) in c.surfaces.iter().enumerate() {
        if s.in_use && s.id == id {
            if caller_pid != 0 {
                let caller_leader = crate::process::get_group_leader(caller_pid);
                if c.owners[idx] != caller_leader {
                    return Err("permission denied");
                }
            }
            let pixels = s.width as usize * s.height as usize;
            let expect = pixels * 4;
            if payload.len() != expect {
                return Err("bad payload size");
            }
            // Write to back buffer when double-buffering is active, else to front.
            let target = if c.back_buffers[idx].is_some() {
                if c.back_buffers[idx].as_ref().map(|b| b.len()).unwrap_or(0) != pixels {
                    c.back_buffers[idx] = Some(vec![0u32; pixels]);
                }
                c.back_buffers[idx].as_mut().ok_or("no back buffer")?
            } else {
                if c.buffers[idx].is_none() {
                    c.buffers[idx] = Some(vec![0u32; pixels]);
                }
                c.buffers[idx].as_mut().ok_or("no buffer")?
            };
            for i in 0..pixels {
                let b = i * 4;
                let r = payload[b] as u32;
                let g = payload[b + 1] as u32;
                let bl = payload[b + 2] as u32;
                let a = payload[b + 3] as u32;
                target[i] = r | (g << 8) | (bl << 16) | (a << 24);
            }
            if c.back_buffers[idx].is_none() {
                // Single-buffer mode: mark presented immediately.
                c.presented[idx] = true;
            }
            return Ok(());
        }
    }
    Err("no such surface")
}

/// Mark a surface ready for composition and render immediately.
pub fn present_surface(id: u32) -> Result<(), &'static str> {
    present_surface_for(0, id)
}

pub fn present_surface_for(caller_pid: u32, id: u32) -> Result<(), &'static str> {
    {
        let mut c = COMP.lock();
        let mut found = false;
        for (idx, s) in c.surfaces.iter().enumerate() {
            if s.in_use && s.id == id {
                if caller_pid != 0 {
                    let caller_leader = crate::process::get_group_leader(caller_pid);
                    if c.owners[idx] != caller_leader {
                        return Err("permission denied");
                    }
                }
                c.presented[idx] = true;
                found = true;
                break;
            }
        }
        if !found {
            return Err("no such surface");
        }
    }
    render_frame();
    Ok(())
}

/// GPU fast-path: upload RGBA32 pixel data into the back buffer.
///
/// Phase 33-C: writes to back_buffers[idx] and sets back_pending so the
/// compositor's next vsync call to render_frame() will flip back→front and
/// blit — eliminating the tearing that occurred when we blitted mid-frame.
///
/// `payload` must be exactly `surface.width * surface.height * 4` bytes.
pub fn gpu_submit_for(caller_pid: u32, id: u32, payload: &[u8]) -> Result<(), &'static str> {
    submit_bgra_impl(caller_pid, id, payload, 0)
}

/// Unified surface submit: Flutter kN32 = BGRA8888 (little-endian x86) → the
/// compositor's RGBA back buffer, in a SINGLE u32-wise, stride-aware pass.
///
/// `src_row_bytes == 0` means a tight `width*4` stride. This replaces the old
/// two-pass path — a strided re-pack into a fresh `vec![0u8; w*h*4]` allocated
/// EVERY frame, followed by a byte-indexed B↔R swap — which cost two full
/// 1M-pixel passes plus a 4 MB allocation per frame. Reading the source as u32
/// and swapping bytes 0↔2 with bit ops is ~one memory op per pixel instead of
/// four, and skips the intermediate buffer entirely.
fn submit_bgra_impl(
    caller_pid: u32,
    id: u32,
    payload: &[u8],
    src_row_bytes: usize,
) -> Result<(), &'static str> {
    let mut c = COMP.lock();
    for (idx, s) in c.surfaces.iter().enumerate() {
        if s.in_use && s.id == id {
            if caller_pid != 0 {
                let caller_leader = crate::process::get_group_leader(caller_pid);
                if c.owners[idx] != caller_leader {
                    return Err("permission denied");
                }
            }
            let w = s.width as usize;
            let h = s.height as usize;
            let pixels = w * h;
            let row_px = if src_row_bytes == 0 { w } else { src_row_bytes / 4 };
            if payload.len() < row_px * 4 * h {
                return Err("bad payload size");
            }
            if c.back_buffers[idx].is_none()
                || c.back_buffers[idx].as_ref().map(|b| b.len()).unwrap_or(0) != pixels
            {
                c.back_buffers[idx] = Some(alloc::vec![0u32; pixels]);
            }
            let buf = c.back_buffers[idx].as_mut().ok_or("no back buffer")?;
            // payload bytes [B,G,R,A] => little-endian u32 = B | G<<8 | R<<16 | A<<24.
            // The compositor wants R in the low byte: swap bytes 0 and 2.
            let src = payload.as_ptr() as *const u32;
            unsafe {
                for y in 0..h {
                    let s_row = src.add(y * row_px);
                    let d_row = y * w;
                    for x in 0..w {
                        let p = core::ptr::read_unaligned(s_row.add(x));
                        buf[d_row + x] =
                            (p & 0xFF00_FF00) | ((p & 0x0000_00FF) << 16) | ((p & 0x00FF_0000) >> 16);
                    }
                }
            }
            c.back_pending[idx] = true;
            drop(c);
            render_frame();
            return Ok(());
        }
    }
    Err("no such surface")
}

fn has_visible_content(c: &CompositorState) -> bool {
    for idx in 0..MAX_SURFACES {
        if c.surfaces[idx].in_use && c.presented[idx] && c.buffers[idx].is_some() {
            return true;
        }
    }
    false
}

/// Simple PS/2 pointer overlay (OS cursor — Flutter does not draw one in software mode).
fn blend_pixel(bg: u32, fg: u32, alpha: u8) -> u32 {
    if alpha == 255 {
        return fg;
    }
    if alpha == 0 {
        return bg;
    }
    let r_bg = (bg >> 16) & 0xFF;
    let g_bg = (bg >> 8) & 0xFF;
    let b_bg = bg & 0xFF;

    let r_fg = (fg >> 16) & 0xFF;
    let g_fg = (fg >> 8) & 0xFF;
    let b_fg = fg & 0xFF;

    let a = alpha as u32;
    let r_out = (r_fg * a + r_bg * (255 - a)) / 255;
    let g_out = (g_fg * a + g_bg * (255 - a)) / 255;
    let b_out = (b_fg * a + b_bg * (255 - a)) / 255;

    (r_out << 16) | (g_out << 8) | b_out
}

/// Plot one solid cursor pixel with bounds checking.
#[inline]
fn cursor_px(x: i32, y: i32, dx: i32, dy: i32, color: u32) {
    let px = x + dx;
    let py = y + dy;
    if px >= 0 && py >= 0 {
        crate::drivers::fb::set_pixel(px as u32, py as u32, color);
    }
}

/// Draw a 1px-thick filled line between two points (Bresenham). Used to build
/// the vector cursor sprites without per-shape bitmap tables.
fn cursor_line(x: i32, y: i32, mut x0: i32, mut y0: i32, x1: i32, y1: i32, color: u32) {
    let dx = (x1 - x0).abs();
    let dy = -(y1 - y0).abs();
    let sx = if x0 < x1 { 1 } else { -1 };
    let sy = if y0 < y1 { 1 } else { -1 };
    let mut err = dx + dy;
    loop {
        cursor_px(x, y, x0, y0, color);
        if x0 == x1 && y0 == y1 {
            break;
        }
        let e2 = 2 * err;
        if e2 >= dy {
            err += dy;
            x0 += sx;
        }
        if e2 <= dx {
            err += dx;
            y0 += sy;
        }
    }
}

/// Mouse-cursor overlay rendering. The OS draws the pointer in software (Flutter
/// does not in software-renderer mode). The shape is selected by the Flutter
/// framework via `SYS_CURSOR_SHAPE_SET` (e.g. button hover → click/hand, text
/// field hover → I-beam), so hovering interactive widgets actually changes the
/// cursor — a real platform capability, not a stub.
fn draw_software_cursor() {
    // Cursor state is unified in `wm` and fed by every pointer source (x86 PS/2
    // AND aarch64 virtio-input), so this works on both arches. Previously it was
    // gated on the x86-only PS2_READY flag and never drew on ARM.
    if !crate::wm::cursor_seen() {
        return;
    }

    // Auto-disappear cursor after 3 seconds of inactivity (arch-neutral ns clock).
    let last_act = crate::wm::cursor_last_act_ns();
    let now = crate::arch::rdtsc_ns();
    if now.wrapping_sub(last_act) > 3_000_000_000 {
        return;
    }

    let shape = cursor_shape();
    use crate::embedder::abi::*;
    if shape == CURSOR_SHAPE_NONE {
        return;
    }

    let (mut x, mut y) = crate::wm::cursor_pos();
    let buttons = crate::wm::cursor_buttons();

    // Keep the sprite fully on-screen (max sprite extent is ~10px from hotspot).
    if let Some((w, h)) = crate::drivers::fb::size_px() {
        x = x.clamp(10, w as i32 - 11);
        y = y.clamp(10, h as i32 - 11);
    }

    // White fill, dark outline — readable on any background.
    let fill = 0x00FFFFFFu32;
    let edge = 0x00101820u32;
    let accent = if buttons != 0 { 0x0038BDF8u32 } else { fill };

    match shape {
        CURSOR_SHAPE_BASIC => draw_arrow_cursor(x, y, buttons, accent, edge),
        CURSOR_SHAPE_TEXT => draw_ibeam_cursor(x, y, fill, edge),
        CURSOR_SHAPE_CLICK => draw_hand_cursor(x, y, fill, edge),
        CURSOR_SHAPE_FORBIDDEN => draw_forbidden_cursor(x, y),
        CURSOR_SHAPE_GRAB | CURSOR_SHAPE_GRABBING => {
            draw_hand_cursor(x, y, fill, edge)
        }
        CURSOR_SHAPE_RESIZE_LR => draw_resize_cursor(x, y, true, fill, edge),
        CURSOR_SHAPE_RESIZE_UD => draw_resize_cursor(x, y, false, fill, edge),
        // Unknown → fall back to the basic pointer.
        _ => draw_arrow_cursor(x, y, buttons, accent, edge),
    }
}

/// The classic circular pointer (the original OSCortex cursor), kept as the
/// default `basic` shape with its click scaling + radial glow.
fn draw_arrow_cursor(x: i32, y: i32, buttons: u32, _accent: u32, _edge: u32) {
    let (r, r_glow, color) = if buttons != 0 {
        (4i32, 8i32, 0x0038BDF8) // Clicked: cyan accent
    } else {
        (6i32, 10i32, 0x00FFFFFF) // Idle: white
    };
    for dy in -r_glow..=r_glow {
        for dx in -r_glow..=r_glow {
            let dist2 = dx * dx + dy * dy;
            let px = x + dx;
            let py = y + dy;
            if px >= 0 && py >= 0 {
                let px = px as u32;
                let py = py as u32;
                if dist2 <= r * r {
                    crate::drivers::fb::set_pixel(px, py, color);
                } else if dist2 <= r_glow * r_glow {
                    let bg = crate::drivers::fb::get_pixel(px, py);
                    let t = (r_glow * r_glow) - dist2;
                    let max_t = (r_glow * r_glow) - (r * r);
                    let alpha = if max_t > 0 { (t * 80) / max_t } else { 0 } as u8;
                    let glow_color = if buttons != 0 { 0x0038BDF8 } else { 0x00FFFFFF };
                    let blended = blend_pixel(bg, glow_color, alpha);
                    crate::drivers::fb::set_pixel(px, py, blended);
                }
            }
        }
    }
}

/// I-beam (text cursor): a vertical bar with top/bottom serifs.
fn draw_ibeam_cursor(x: i32, y: i32, fill: u32, edge: u32) {
    // Outline first (1px around), then fill, for legibility on any background.
    for dy in -8..=8 {
        cursor_px(x, y, -1, dy, edge);
        cursor_px(x, y, 1, dy, edge);
        cursor_px(x, y, 0, dy, fill);
    }
    // Serifs at the top and bottom.
    for dx in -3..=3 {
        cursor_px(x, y, dx, -9, edge);
        cursor_px(x, y, dx, 9, edge);
        cursor_px(x, y, dx, -8, fill);
        cursor_px(x, y, dx, 8, fill);
    }
}

/// Hand / link cursor (also used for grab): a pointing-finger silhouette.
fn draw_hand_cursor(x: i32, y: i32, fill: u32, edge: u32) {
    // Pointing finger: a vertical column for the finger and a rounded palm.
    // Finger (extends up from the hotspot).
    for dy in -9..=2 {
        for dx in -1..=1 {
            cursor_px(x, y, dx, dy, fill);
        }
    }
    // Palm (a filled rounded block below the finger).
    for dy in 2..=9 {
        for dx in -4..=4 {
            let edge_block = dy == 9 || dx == -4 || dx == 4;
            cursor_px(x, y, dx, dy, if edge_block { edge } else { fill });
        }
    }
    // Knuckle lines hinting folded fingers.
    cursor_px(x, y, -2, 3, edge);
    cursor_px(x, y, 0, 3, edge);
    cursor_px(x, y, 2, 3, edge);
    // Outline the finger.
    for dy in -9..=2 {
        cursor_px(x, y, -2, dy, edge);
        cursor_px(x, y, 2, dy, edge);
    }
    cursor_px(x, y, 0, -10, edge);
}

/// Forbidden / no-drop cursor: a red circle with a diagonal slash.
fn draw_forbidden_cursor(x: i32, y: i32) {
    let red = 0x00E02424u32;
    let r = 8i32;
    // Ring (distance-based outline, 2px thick).
    for dy in -r..=r {
        for dx in -r..=r {
            let dist2 = dx * dx + dy * dy;
            if dist2 <= r * r && dist2 >= (r - 2) * (r - 2) {
                cursor_px(x, y, dx, dy, red);
            }
        }
    }
    // Diagonal slash (top-left to bottom-right).
    cursor_line(x, y, -5, -5, 5, 5, red);
    cursor_line(x, y, -5, -4, 5, 6, red);
}

/// Resize cursor: a double-headed arrow, horizontal or vertical.
fn draw_resize_cursor(x: i32, y: i32, horizontal: bool, fill: u32, edge: u32) {
    if horizontal {
        // Shaft.
        for dx in -8..=8 {
            cursor_px(x, y, dx, 0, fill);
            cursor_px(x, y, dx, -1, edge);
            cursor_px(x, y, dx, 1, edge);
        }
        // Arrow heads.
        for i in 0..4 {
            cursor_px(x, y, -8 + i, -i, fill);
            cursor_px(x, y, -8 + i, i, fill);
            cursor_px(x, y, 8 - i, -i, fill);
            cursor_px(x, y, 8 - i, i, fill);
        }
    } else {
        for dy in -8..=8 {
            cursor_px(x, y, 0, dy, fill);
            cursor_px(x, y, -1, dy, edge);
            cursor_px(x, y, 1, dy, edge);
        }
        for i in 0..4 {
            cursor_px(x, y, -i, -8 + i, fill);
            cursor_px(x, y, i, -8 + i, fill);
            cursor_px(x, y, -i, 8 - i, fill);
            cursor_px(x, y, i, 8 - i, fill);
        }
    }
}

// ── Phase 32-D: stride-aware GPU blit ─────────────────────────────────────────

/// Stride-aware variant of [`gpu_submit_for`].
///
/// `row_bytes` is the number of bytes per scanline in `payload`.  If
/// `row_bytes == 0` or `row_bytes == width * 4`, this delegates to the tight-
/// packed path (zero copy overhead).  Otherwise it extracts each row from the
/// strided buffer and packs it into a contiguous slice before uploading.
pub fn gpu_submit_strided_for(
    caller_pid: u32,
    id: u32,
    payload: &[u8],
    row_bytes: usize,
) -> Result<(), &'static str> {
    // submit_bgra_impl handles the source stride directly in its single packing
    // pass — no intermediate tight buffer, no per-frame allocation.
    submit_bgra_impl(caller_pid, id, payload, row_bytes)
}

/// Packed framebuffer size: `(width << 32) | height`.
pub fn framebuffer_size_packed() -> u64 {
    match crate::drivers::fb::size_px() {
        Some((w, h)) => ((w as u64) << 32) | (h as u64),
        None => 0,
    }
}

pub fn frame_counter() -> u64 {
    COMP.lock().frame_counter
}

pub fn wait_vsync(last_seen: u64) -> Option<u64> {
    let now = frame_counter();
    if now > last_seen { Some(now) } else { None }
}

pub fn render_frame() {
    if !crate::drivers::fb::is_ready() {
        return;
    }

    let mut c = COMP.lock();

    for idx in 0..MAX_SURFACES {
        if c.back_pending[idx] {
            let back = c.back_buffers[idx].take();
            let front = c.buffers[idx].take();
            c.buffers[idx] = back;
            c.back_buffers[idx] = front;
            c.back_pending[idx] = false;
            c.presented[idx] = true;
        }
    }

    if !has_visible_content(&c) {
        let frame = c.frame_counter.wrapping_add(1);
        c.frame_counter = frame;
        drop(c);
        // No app has presented a frame yet → we're in the engine warm-up. Draw the
        // kernel boot screen (dot-matrix OSCORTEX + progress + status, or the F2
        // verbose log overlay) instead of raw logs. It fills its own black bg.
        crate::drivers::bootscreen::render();
        draw_software_cursor();
        input_hud::draw();
        crate::drivers::fb::swap_buffers();
        crate::wm::push_vsync(frame);
        return;
    }
    // First real surface presented → boot is done; snap the progress to 100%.
    crate::drivers::bootscreen::mark_done();

    let bypass = is_fb_bypass();
    drop(c);

    if !bypass {
        if let Some((w, h)) = crate::drivers::fb::size_px() {
            // Skip the full-screen clear when a presented surface already covers
            // the whole screen — it would just be overwritten by the blit below.
            // Saves a full 1M-pixel pass per frame (the common case: one
            // full-screen Flutter surface).
            let covered = {
                let c = COMP.lock();
                c.surfaces.iter().enumerate().any(|(idx, s)| {
                    s.in_use
                        && c.presented[idx]
                        && s.x <= 0
                        && s.y <= 0
                        && (s.x + s.width as i32) >= w as i32
                        && (s.y + s.height as i32) >= h as i32
                })
            };
            if !covered {
                crate::drivers::fb::fill_rect(0, 0, w, h, 0x00101820);
            }
        }
    }

    let c = COMP.lock();

    let mut order = [0usize; MAX_SURFACES];
    let mut count = 0usize;
    for (idx, s) in c.surfaces.iter().enumerate() {
        if s.in_use {
            order[count] = idx;
            count += 1;
        }
    }
    for i in 1..count {
        let key = order[i];
        let key_z = c.surfaces[key].z;
        let mut j = i;
        while j > 0 && c.surfaces[order[j - 1]].z > key_z {
            order[j] = order[j - 1];
            j -= 1;
        }
        order[j] = key;
    }

    for idx in order.iter().take(count).copied() {
        if !c.presented[idx] {
            continue;
        }
        let s = c.surfaces[idx];
        if let Some(buf) = c.buffers[idx].as_ref() {
            crate::drivers::fb::blit_rgba32(s.x, s.y, s.width, s.height, buf);
        }
    }

    drop(c);
    draw_software_cursor();
    input_hud::draw();
    crate::drivers::fb::swap_buffers();
    let mut c = COMP.lock();
    c.frame_counter = c.frame_counter.wrapping_add(1);
    crate::wm::push_vsync(c.frame_counter);
}

/// Lightweight vsync handler for the timer ISR: advance the frame counter and
/// deliver the vsync baton (push_vsync) WITHOUT rendering. The Flutter present
/// path (`present_surface_for` → `render_frame`) does the heavy compositing in a
/// normal syscall context, so `render_frame` must NEVER run in the timer ISR —
/// under slow emulation (TCG) a full-screen fill there starves the engine's main
/// thread and the boot livelocks in `fb::fill_rect` before the engine even loads.
/// (The boot-splash animation is sacrificed; the shell renders via present.)
pub fn vsync_baton_tick() {
    if let Some(mut c) = COMP.try_lock() {
        c.frame_counter = c.frame_counter.wrapping_add(1);
        let f = c.frame_counter;
        drop(c);
        crate::wm::push_vsync(f);
    }
}

pub fn tick() {
    if !crate::drivers::fb::is_ready() {
        return;
    }
    if is_fb_bypass() {
        if let Some(mut c) = COMP.try_lock() {
            c.frame_counter = c.frame_counter.wrapping_add(1);
            crate::wm::push_vsync(c.frame_counter);
        }
        return;
    }

    let mut c = match COMP.try_lock() {
        Some(guard) => guard,
        None => return,
    };

    let mut dirty = FRAME_DIRTY.swap(false, Ordering::AcqRel);

    // 1. Check if any surfaces changed (creation, destruction, moving, resizing, visibility, clip, etc.)
    {
        static mut LAST_SURFACES: [Surface; MAX_SURFACES] = [const { Surface::empty() }; MAX_SURFACES];
        if c.surfaces != unsafe { LAST_SURFACES } {
            unsafe { LAST_SURFACES = c.surfaces; }
            dirty = true;
        }
    }

    // 3. Re-render when surfaces or pending back buffers change.
    for idx in 0..MAX_SURFACES {
        if c.back_pending[idx] {
            dirty = true;
            break;
        }
    }

    // 4. Boot splash animation: while nothing visible has been presented yet,
    // force a periodic redraw so the loading indicator animates and the user
    // can see the machine is alive during the engine warm-up. Throttled so it
    // doesn't steal CPU the JIT warm-up needs.
    if !dirty && !has_visible_content(&c) && c.frame_counter % 5 == 0 {
        dirty = true;
    }

    if dirty {
        drop(c);
        render_frame();
    } else {
        // Even if we don't render, keep pushing vsync to notify event consumers
        c.frame_counter = c.frame_counter.wrapping_add(1);
        crate::wm::push_vsync(c.frame_counter);
    }
}

