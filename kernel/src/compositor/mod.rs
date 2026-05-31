//! M13 compositor scaffold.
//!
//! This is the first plumbing layer for window/surface management before
//! Flutter embedder integration. It intentionally keeps a fixed-size table and
//! avoids heap allocation in early boot.

use alloc::vec;
use alloc::vec::Vec;
use spin::Mutex;
use core::sync::atomic::{AtomicBool, Ordering};

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
            // Phase 33-C: write to back buffer, flip at next vsync.
            if c.back_buffers[idx].is_none()
                || c.back_buffers[idx].as_ref().map(|b| b.len()).unwrap_or(0) != pixels
            {
                c.back_buffers[idx] = Some(alloc::vec![0u32; pixels]);
            }
            let buf = c.back_buffers[idx].as_mut().ok_or("no back buffer")?;
            // Flutter's software surface is kN32 (= BGRA8888 on little-endian x86):
            // payload bytes are [B, G, R, A]. The compositor buffer / blit_rgba32
            // expect R in the low byte, so swap B and R while packing.
            for i in 0..pixels {
                let b = i * 4;
                let bl = payload[b    ] as u32;
                let g  = payload[b + 1] as u32;
                let r  = payload[b + 2] as u32;
                let a  = payload[b + 3] as u32;
                buf[i] = r | (g << 8) | (bl << 16) | (a << 24);
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
fn draw_software_cursor() {
    use core::sync::atomic::Ordering;
    if !crate::drivers::ps2::PS2_READY.load(Ordering::Relaxed) {
        return;
    }
    let (mut x, mut y) = crate::drivers::ps2::cursor_pos();
    if let Some((w, h)) = crate::drivers::fb::size_px() {
        x = x.clamp(0, w as i32 - 14);
        y = y.clamp(0, h as i32 - 18);
    }
    const FG: u32 = 0xFFFFFFFF;
    const BG: u32 = 0xFF000000;
    // Classic arrow: vertical shaft + diagonal fill.
    for i in 0..14 {
        crate::drivers::fb::fill_rect(x, y + i, 1, 1, BG);
        crate::drivers::fb::fill_rect(x + 1, y + i, 1, 1, FG);
    }
    for i in 0..9 {
        crate::drivers::fb::fill_rect(x + i, y + i, 2, 2, BG);
        if i < 8 {
            crate::drivers::fb::fill_rect(x + i + 1, y + i + 1, 1, 1, FG);
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
    // Determine the surface dimensions we need for stride validation.
    let (width, height) = {
        let c = COMP.lock();
        let mut dims = None;
        for s in c.surfaces.iter() {
            if s.in_use && s.id == id {
                dims = Some((s.width as usize, s.height as usize));
                break;
            }
        }
        dims.ok_or("no such surface")?
    };

    let tight_stride = width * 4;

    // Fast path: no stride conversion needed.
    if row_bytes == 0 || row_bytes == tight_stride {
        let expected_bytes = tight_stride * height;
        if payload.len() < expected_bytes {
            return Err("bad payload size");
        }
        return gpu_submit_for(caller_pid, id, &payload[..expected_bytes]);
    }

    // Validate that the buffer is large enough for the strided layout.
    let expected_bytes = row_bytes * height;
    if payload.len() < expected_bytes {
        return Err("bad payload size");
    }

    // Re-pack rows into a tight RGBA32 buffer.
    let mut tight: Vec<u8> = vec![0u8; tight_stride * height];
    for row in 0..height {
        let src_start = row * row_bytes;
        let dst_start = row * tight_stride;
        tight[dst_start..dst_start + tight_stride]
            .copy_from_slice(&payload[src_start..src_start + tight_stride]);
    }

    gpu_submit_for(caller_pid, id, &tight)
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
        if let Some((w, h)) = crate::drivers::fb::size_px() {
            crate::drivers::fb::fill_rect(0, 0, w, h, 0x000c1c26);
        }
        draw_software_cursor();
        crate::drivers::fb::swap_buffers();
        crate::wm::push_vsync(frame);
        return;
    }

    let bypass = is_fb_bypass();
    drop(c);

    if !bypass {
        if let Some((w, h)) = crate::drivers::fb::size_px() {
            crate::drivers::fb::fill_rect(0, 0, w, h, 0x00101820);
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
    crate::drivers::fb::swap_buffers();
    let mut c = COMP.lock();
    c.frame_counter = c.frame_counter.wrapping_add(1);
    crate::wm::push_vsync(c.frame_counter);
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

    if dirty {
        drop(c);
        render_frame();
    } else {
        // Even if we don't render, keep pushing vsync to notify event consumers
        c.frame_counter = c.frame_counter.wrapping_add(1);
        crate::wm::push_vsync(c.frame_counter);
    }
}

