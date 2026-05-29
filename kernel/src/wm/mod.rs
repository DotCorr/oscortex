//! Window-manager event bridge (M13+).
//!
//! Provides a fixed-size lock-protected event ring that userspace can poll
//! through syscalls. This is the first event channel for Flutter embedder
//! plumbing (vsync + input-like events).

use crate::embedder::abi as eabi;
use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use spin::Mutex;

pub const EVENT_CAP: usize = 256;

pub use eabi::{EV_APP, EV_FOCUS, EV_KEY, EV_POINTER, EV_VSYNC};
pub type WmEvent = eabi::WmEvent;

struct EventQueue {
    buf: [WmEvent; EVENT_CAP],
    owner_pid: [u32; EVENT_CAP], // 0 = broadcast, otherwise targeted PID
    head: usize,
    tail: usize,
    len: usize,
    next_seq: u64,
    dropped: u64,
}

impl EventQueue {
    const fn new() -> Self {
        Self {
            buf: [const { WmEvent::empty() }; EVENT_CAP],
            owner_pid: [0; EVENT_CAP],
            head: 0,
            tail: 0,
            len: 0,
            next_seq: 1,
            dropped: 0,
        }
    }

    fn push(&mut self, mut ev: WmEvent, owner_pid: u32) {
        ev.seq = self.next_seq;
        self.next_seq = self.next_seq.wrapping_add(1).max(1);

        // Coalesce baton=0 vsync events: update the existing pending entry
        // in-place rather than adding a new one.  This prevents EV_VSYNC(0)
        // floods from filling the queue and pushing EV_VSYNC(baton≠0) out.
        if ev.kind == EV_VSYNC && ev.b == 0 {
            for off in 0..self.len {
                let idx = (self.head + off) % EVENT_CAP;
                if self.buf[idx].kind == EV_VSYNC
                    && self.buf[idx].b == 0
                    && self.owner_pid[idx] == owner_pid
                {
                    // Update frame counter in-place; baton stays 0.
                    self.buf[idx].a = ev.a;
                    return;
                }
            }
        }

        if self.len == EVENT_CAP {
            self.head = (self.head + 1) % EVENT_CAP;
            self.len -= 1;
            self.dropped = self.dropped.wrapping_add(1);
        }

        self.buf[self.tail] = ev;
        self.owner_pid[self.tail] = owner_pid;
        self.tail = (self.tail + 1) % EVENT_CAP;
        self.len += 1;
    }

    fn push_front(&mut self, mut ev: WmEvent, owner_pid: u32) {
        ev.seq = self.next_seq;
        self.next_seq = self.next_seq.wrapping_add(1).max(1);

        if self.len == EVENT_CAP {
            self.tail = (self.tail + EVENT_CAP - 1) % EVENT_CAP;
            self.len -= 1;
            self.dropped = self.dropped.wrapping_add(1);
        }

        self.head = (self.head + EVENT_CAP - 1) % EVENT_CAP;
        self.buf[self.head] = ev;
        self.owner_pid[self.head] = owner_pid;
        self.len += 1;
    }

    fn pop_for(&mut self, pid: u32) -> Option<WmEvent> {
        if self.len == 0 {
            return None;
        }

        // Baton vsync events must not sit behind baton=0 ticks — the embedder
        // needs FlutterEngineOnVsync promptly or the engine stalls rendering.
        let mut found_off = None;
        for off in 0..self.len {
            let idx = (self.head + off) % EVENT_CAP;
            let owner = self.owner_pid[idx];
            if (owner == 0 || owner == pid)
                && self.buf[idx].kind == EV_VSYNC
                && self.buf[idx].b != 0
            {
                found_off = Some(off);
                break;
            }
        }

        // Find first event visible to this consumer.
        if found_off.is_none() {
            for off in 0..self.len {
                let idx = (self.head + off) % EVENT_CAP;
                let owner = self.owner_pid[idx];
                if owner == 0 || owner == pid {
                    found_off = Some(off);
                    break;
                }
            }
        }
        let off = found_off?;
        let idx = (self.head + off) % EVENT_CAP;
        let ev = self.buf[idx];

        // Remove the matched slot by shifting later entries left in the ring.
        let mut j = off;
        while j + 1 < self.len {
            let from = (self.head + j + 1) % EVENT_CAP;
            let to = (self.head + j) % EVENT_CAP;
            self.buf[to] = self.buf[from];
            self.owner_pid[to] = self.owner_pid[from];
            j += 1;
        }
        self.tail = (self.tail + EVENT_CAP - 1) % EVENT_CAP;
        self.len -= 1;
        Some(ev)
    }

    fn pending_for(&self, pid: u32) -> usize {
        let mut n = 0usize;
        for off in 0..self.len {
            let idx = (self.head + off) % EVENT_CAP;
            let owner = self.owner_pid[idx];
            if owner == 0 || owner == pid {
                n += 1;
            }
        }
        n
    }

    fn has_baton_vsync_for(&self, pid: u32) -> bool {
        for off in 0..self.len {
            let idx = (self.head + off) % EVENT_CAP;
            let owner = self.owner_pid[idx];
            if (owner == 0 || owner == pid)
                && self.buf[idx].kind == EV_VSYNC
                && self.buf[idx].b != 0
            {
                return true;
            }
        }
        false
    }
}

static Q: Mutex<EventQueue> = Mutex::new(EventQueue::new());

static WM_WAITER: AtomicU32 = AtomicU32::new(0);

pub fn set_wm_waiter(pid: u32) {
    WM_WAITER.store(pid, Ordering::Release);
}

pub fn get_wm_waiter() -> u32 {
    WM_WAITER.load(Ordering::Acquire)
}

struct SynthInput {
    tick: u64,
    x: i32,
    y: i32,
    e0_prefix: bool,
    mouse_pkt: [u8; 3],
    mouse_len: usize,
}

impl SynthInput {
    const fn new() -> Self {
        Self {
            tick: 0,
            x: 32,
            y: 32,
            e0_prefix: false,
            mouse_pkt: [0; 3],
            mouse_len: 0,
        }
    }
}

static SYNTH: Mutex<SynthInput> = Mutex::new(SynthInput::new());
static FOCUS_PID: AtomicU32 = AtomicU32::new(0);
static FOCUS_MIRROR: AtomicBool = AtomicBool::new(false);
/// Pending vsync baton posted by the embedder via `sys_engine_vsync_baton_post`.
/// Included in the next EV_VSYNC event's `b` field so the embedder can call
/// `FlutterEngineOnVsync` with the correct baton value.
static VSYNC_BATON: AtomicU64 = AtomicU64::new(0);
static BATON_VSYNC_QUEUED: AtomicBool = AtomicBool::new(false);

pub fn init() {
    log::info!("[WM] Event queue online (cap={})", EVENT_CAP);
}

pub fn focus_pid() -> u32 {
    FOCUS_PID.load(Ordering::Acquire)
}

pub fn focus_mirror_enabled() -> bool {
    FOCUS_MIRROR.load(Ordering::Acquire)
}

pub fn set_focus_mirror_enabled(enabled: bool) {
    FOCUS_MIRROR.store(enabled, Ordering::Release);
}

/// Push an app lifecycle event (EV_APP) to a specific target PID.
/// `subkind` is one of APP_LAUNCH / APP_TERMINATE / APP_PAUSE / APP_RESUME.
/// `surface_id` is the surface associated with the event (0 if none).
pub fn push_app_event(target_pid: u32, subkind: u32, surface_id: u32) {
    push_event_for(target_pid, EV_APP, subkind, surface_id as u64, 0);
}

pub fn set_focus_pid(pid: u32) {
    let old = FOCUS_PID.swap(pid, Ordering::AcqRel);
    if old == pid {
        return;
    }

    if old != 0 {
        // Target old foreground process with explicit focus-loss event.
        push_event_for(old, EV_FOCUS, eabi::FOCUS_LOST, pid as u64, 0);
    }
    if pid != 0 {
        // Target new foreground process with explicit focus-gain event.
        push_event_for(pid, EV_FOCUS, eabi::FOCUS_GAINED, old as u64, 0);
    }

    // Optional global observer mirror event: a=old_focus_pid, b=new_focus_pid.
    if focus_mirror_enabled() {
        push_event(EV_FOCUS, eabi::FOCUS_MIRROR, old as u64, pid as u64);
    }
}

/// Poll real PS/2 input and forward pointer/key events to userspace.
pub fn tick() {
    let Some((w, h)) = crate::drivers::fb::size_px() else {
        return;
    };

    if !crate::drivers::ps2::PS2_READY.load(core::sync::atomic::Ordering::Relaxed) {
        return;
    }

    let mut s = SYNTH.lock();
    s.tick = s.tick.wrapping_add(1);
    poll_ps2_input(&mut s, w as i32, h as i32);
}

#[cfg(target_arch = "x86_64")]
#[inline(always)]
unsafe fn in8(port: u16) -> u8 {
    let val: u8;
    unsafe {
        core::arch::asm!("in al, dx", in("dx") port, out("al") val, options(nomem, nostack));
    }
    val
}

#[cfg(target_arch = "x86_64")]
fn poll_ps2_input(s: &mut SynthInput, max_w: i32, max_h: i32) -> bool {
    let mut produced = false;

    // Drain a bounded number of queued controller bytes per tick.
    for _ in 0..64 {
        let status = unsafe { in8(0x64) };
        if status & 0x01 == 0 {
            break; // output buffer empty
        }

        let byte = unsafe { in8(0x60) };
        let is_mouse = (status & 0x20) != 0;

        if is_mouse {
            s.mouse_pkt[s.mouse_len] = byte;
            s.mouse_len += 1;
            if s.mouse_len == 3 {
                let p0 = s.mouse_pkt[0];
                let p1 = s.mouse_pkt[1];
                let p2 = s.mouse_pkt[2];
                s.mouse_len = 0;

                // Ignore overflow packets.
                if (p0 & 0xC0) != 0 {
                    continue;
                }

                let dx = if (p0 & 0x10) != 0 {
                    (p1 as i16 as i8) as i32
                } else {
                    p1 as i32
                };
                let dy = if (p0 & 0x20) != 0 {
                    (p2 as i16 as i8) as i32
                } else {
                    p2 as i32
                };

                s.x = (s.x + dx).clamp(0, (max_w - 1).max(0));
                // PS/2 Y is positive when moving down? In practice invert for screen space.
                s.y = (s.y - dy).clamp(0, (max_h - 1).max(0));

                let buttons = (p0 & 0x07) as u32;
                push_pointer(s.x, s.y, buttons);
                produced = true;
            }
        } else {
            if byte == 0xE0 {
                s.e0_prefix = true;
                continue;
            }
            let pressed = (byte & 0x80) == 0;
            let sc = (byte & 0x7F) as u32;
            let scancode = if s.e0_prefix {
                s.e0_prefix = false;
                0xE000 | sc
            } else {
                sc
            };
            push_key(scancode, pressed);
            produced = true;
        }
    }

    produced
}

#[cfg(not(target_arch = "x86_64"))]
fn poll_ps2_input(_s: &mut SynthInput, _max_w: i32, _max_h: i32) -> bool {
    false
}

pub fn event_size() -> usize {
    eabi::WM_EVENT_SIZE
}

pub fn pending_count() -> usize {
    Q.lock().len
}

pub fn pending_count_for(pid: u32) -> usize {
    Q.lock().pending_for(pid)
}

pub fn dropped_count() -> u64 {
    Q.lock().dropped
}

/// True when the embedder must run FlutterEngineOnVsync (baton queued or posted).
pub fn embedder_baton_due() -> bool {
    vsync_baton_pending() || Q.lock().has_baton_vsync_for(1)
}

pub fn pop_event_for(pid: u32) -> Option<WmEvent> {
    let ev = Q.lock().pop_for(pid)?;
    if ev.kind == EV_VSYNC && ev.b != 0 {
        BATON_VSYNC_QUEUED.store(false, Ordering::Release);
    }
    Some(ev)
}

pub fn push_event(kind: u32, flags: u32, a: u64, b: u64) {
    push_event_for(0, kind, flags, a, b);
}

pub fn push_event_for(owner_pid: u32, kind: u32, flags: u32, a: u64, b: u64) {
    Q.lock().push(WmEvent {
        seq: 0,
        kind,
        flags,
        a,
        b,
    }, owner_pid);

    // Wake the waiter if they are eligible for this event.
    let waiter = WM_WAITER.load(Ordering::Acquire);
    if waiter != 0 && (owner_pid == 0 || owner_pid == waiter) {
        WM_WAITER.store(0, Ordering::Release);
        crate::process::set_state(waiter, crate::process::ProcState::Running);
    }
}

/// Returns true when the engine has posted a vsync baton not yet delivered.
pub fn vsync_baton_pending() -> bool {
    VSYNC_BATON.load(Ordering::Acquire) != 0 || BATON_VSYNC_QUEUED.load(Ordering::Acquire)
}

pub fn push_vsync(frame: u64) {
    // Consume the pending baton (if any) so the embedder's FlutterEngineOnVsync
    // call can use the correct baton. If no baton is posted, b = 0 (ignored).
    let baton = VSYNC_BATON.swap(0, Ordering::AcqRel);
    static PV_SEQ: AtomicU32 = AtomicU32::new(0);
    let n = PV_SEQ.fetch_add(1, Ordering::Relaxed);
    if baton != 0 || n < 30 || n % 300 == 0 {
        log::info!("[push-vsync] #{} frame={} baton={:#x}", n, frame, baton);
    }
    if baton != 0 {
        let mut ev = WmEvent::empty();
        ev.kind = EV_VSYNC;
        ev.a = frame;
        ev.b = baton;
        Q.lock().push_front(ev, 0);
        BATON_VSYNC_QUEUED.store(true, Ordering::Release);
        crate::process::set_state(1, crate::process::ProcState::Running);
        let waiter = WM_WAITER.load(Ordering::Acquire);
        if waiter != 0 {
            WM_WAITER.store(0, Ordering::Release);
            crate::process::set_state(waiter, crate::process::ProcState::Running);
        }
    } else {
        push_event(EV_VSYNC, 0, frame, baton);
    }
}

/// Set the vsync baton that will be included in the next EV_VSYNC event.
/// Called via `sys_engine_vsync_baton_post` when the engine invokes the
/// embedder's `vsync_callback`.
pub fn set_vsync_baton(baton: u64) {
    VSYNC_BATON.store(baton, Ordering::Release);
}

pub fn push_pointer(x: i32, y: i32, buttons: u32) {
    let packed = ((x as u32 as u64) << 32) | (y as u32 as u64);
    
    // In bypass mode a userspace process owns the framebuffer directly.
    // Skip compositor hit-testing entirely so the focused process receives
    // every pointer event regardless of compositor surface layout.
    if !crate::compositor::is_fb_bypass() {
        if let Some((_surf_id, owner_pid)) = crate::compositor::surface_at_point(x, y) {
            push_event_for(owner_pid, EV_POINTER, buttons, packed, 0);
            return;
        }
    }
    
    // Route to focused process, or broadcast if no focus.
    let focus = focus_pid();
    if focus == 0 {
        push_event(EV_POINTER, buttons, packed, 0);
    } else {
        push_event_for(focus, EV_POINTER, buttons, packed, 0);
    }
}

pub fn push_key(scancode: u32, pressed: bool) {
    let focus = focus_pid();
    let flags = if pressed { 1 } else { 0 };
    if focus == 0 {
        push_event(EV_KEY, flags, scancode as u64, 0);
    } else {
        push_event_for(focus, EV_KEY, flags, scancode as u64, 0);
    }
}

