//! Window-manager event bridge (M13+).
//!
//! Provides a fixed-size lock-protected event ring that userspace can poll
//! through syscalls. This is the first event channel for Flutter embedder
//! plumbing (vsync + input-like events).

use crate::embedder::abi as eabi;
use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use spin::Mutex;

pub const EVENT_CAP: usize = 256;

pub use eabi::{EV_APP, EV_FOCUS, EV_KEY, EV_POINTER, EV_SCROLL, EV_VSYNC};
pub type WmEvent = eabi::WmEvent;

#[inline(always)]
fn canonical_pid(pid: u32) -> u32 {
    if pid <= 1 {
        pid
    } else {
        crate::process::get_group_leader(pid)
    }
}

/// Visibility test between an event's owner and a consumer. BOTH pids MUST be
/// already canonical (group leaders). This is a RAW comparison that deliberately
/// does NOT call `canonical_pid` — that would take PTABLE_LOCK, and this runs
/// inside `with_queue` (WM lock held). Acquiring PTABLE_LOCK under the WM lock
/// inverts the scheduler's PTABLE→WM order (next_runnable_pid_locked calls
/// input_pending_for while holding PTABLE_LOCK) → ABBA deadlock under SMP.
/// Callers canonicalize at the WM-module boundary, before taking the WM lock.
#[inline(always)]
fn owner_visible_to_consumer(owner_pid: u32, consumer_pid: u32) -> bool {
    owner_pid == 0 || owner_pid == consumer_pid
}

struct EventQueue {
    buf: [WmEvent; EVENT_CAP],
    owner_pid: [u32; EVENT_CAP], // 0 = broadcast, otherwise targeted group leader
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
        // owner_pid is ALREADY canonical (canonicalized by the caller before the
        // WM lock was taken). Do NOT call canonical_pid here — it takes PTABLE_LOCK
        // under the WM lock and deadlocks against the scheduler (see
        // owner_visible_to_consumer).
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

        // Coalesce consecutive pointer move/hover events into the most recent
        // queued pointer event when the BUTTON STATE is unchanged.  PS/2 emits
        // a flood of position updates; without this the queue fills with stale
        // hover events and real clicks (button transitions) get buried behind a
        // 100+ event backlog, arriving in Flutter far too late.  Only the tail
        // event is examined, so every button transition (down/up) is preserved
        // as its own distinct event and ordering is never violated.
        if ev.kind == EV_POINTER && self.len > 0 {
            let tail_idx = (self.tail + EVENT_CAP - 1) % EVENT_CAP;
            if self.buf[tail_idx].kind == EV_POINTER
                && self.owner_pid[tail_idx] == owner_pid
                && self.buf[tail_idx].flags == ev.flags
            {
                // Same buttons: replace coordinates in place, keep latest pos.
                self.buf[tail_idx].a = ev.a;
                self.buf[tail_idx].b |= ev.b;
                return;
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
        // owner_pid is ALREADY canonical (see push() — never take PTABLE_LOCK here).
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
            if owner_visible_to_consumer(owner, pid)
                && self.buf[idx].kind == EV_VSYNC
                && self.buf[idx].b != 0
            {
                found_off = Some(off);
                break;
            }
        }

        // Input priority: deliver pointer/key events ahead of the constant
        // EV_VSYNC(baton=0) tick stream.  Otherwise a single coalesced pointer
        // event sits behind the perpetually-refreshed baton=0 vsync at the head
        // and is never popped (clicks never reach Flutter).  Render batons
        // (b!=0, handled above) still take precedence so frames keep flowing.
        if found_off.is_none() {
            for off in 0..self.len {
                let idx = (self.head + off) % EVENT_CAP;
                let owner = self.owner_pid[idx];
                if owner_visible_to_consumer(owner, pid)
                    && (self.buf[idx].kind == EV_POINTER || self.buf[idx].kind == EV_KEY)
                {
                    found_off = Some(off);
                    break;
                }
            }
        }

        // Find first event visible to this consumer.
        if found_off.is_none() {
            for off in 0..self.len {
                let idx = (self.head + off) % EVENT_CAP;
                let owner = self.owner_pid[idx];
                if owner_visible_to_consumer(owner, pid) {
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
            if owner_visible_to_consumer(owner, pid) {
                n += 1;
            }
        }
        n
    }

    fn input_pending_for(&self, pid: u32) -> usize {
        let mut n = 0usize;
        for off in 0..self.len {
            let idx = (self.head + off) % EVENT_CAP;
            let owner = self.owner_pid[idx];
            if owner_visible_to_consumer(owner, pid)
                && (self.buf[idx].kind == EV_POINTER || self.buf[idx].kind == EV_KEY)
            {
                n += 1;
            }
        }
        n
    }

    fn has_baton_vsync_for(&self, pid: u32) -> bool {
        for off in 0..self.len {
            let idx = (self.head + off) % EVENT_CAP;
            let owner = self.owner_pid[idx];
            if owner_visible_to_consumer(owner, pid)
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

#[cfg(target_arch = "x86_64")]
#[inline(always)]
fn irq_save() -> bool {
    let flags: u64;
    unsafe {
        core::arch::asm!("pushfq; pop {}", out(reg) flags, options(nomem, preserves_flags));
        core::arch::asm!("cli", options(nomem, nostack, preserves_flags));
    }
    (flags & 0x200) != 0
}

// aarch64 (and others): mask IRQ/FIQ around the event-queue critical section.
// This was a no-op stub, so `with_queue` ran with IRQs ENABLED — the timer ISR
// could fire while a thread held Q.lock(), then the ISR's own push_vsync ->
// with_queue -> Q.lock() spun on the held lock forever (single core, IRQs masked
// in the ISR) → deadlock (boot hung spinning in wm::push_event_for). Returns the
// prior DAIF so irq_restore can re-enable only if they were enabled.
#[cfg(not(target_arch = "x86_64"))]
#[inline(always)]
fn irq_save() -> bool {
    #[cfg(target_arch = "aarch64")]
    {
        let daif: u64;
        unsafe {
            core::arch::asm!("mrs {}, daif", out(reg) daif, options(nomem, nostack));
            core::arch::asm!("msr daifset, #0b0011", options(nomem, nostack)); // mask IRQ+FIQ
        }
        // DAIF.I is bit 7; "was enabled" == I was clear.
        daif & (1 << 7) == 0
    }
    #[cfg(not(target_arch = "aarch64"))]
    { false }
}

#[cfg(target_arch = "x86_64")]
#[inline(always)]
fn irq_restore(was_enabled: bool) {
    if was_enabled {
        unsafe {
            core::arch::asm!("sti", options(nomem, nostack, preserves_flags));
        }
    }
}

#[cfg(not(target_arch = "x86_64"))]
#[inline(always)]
fn irq_restore(was_enabled: bool) {
    #[cfg(target_arch = "aarch64")]
    if was_enabled {
        unsafe { core::arch::asm!("msr daifclr, #0b0010", options(nomem, nostack)) }; // unmask IRQ
    }
    #[cfg(not(target_arch = "aarch64"))]
    { let _ = was_enabled; }
}

#[inline(always)]
fn with_queue<R>(f: impl FnOnce(&mut EventQueue) -> R) -> R {
    let was_enabled = irq_save();
    let mut q = Q.lock();
    let out = f(&mut q);
    drop(q);
    irq_restore(was_enabled);
    out
}

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
static FLUTTER_INIT_READY: AtomicBool = AtomicBool::new(false);

pub fn flutter_init_ready() -> bool {
    FLUTTER_INIT_READY.load(Ordering::Acquire)
}

pub fn set_flutter_init_ready() {
    FLUTTER_INIT_READY.store(true, Ordering::Release);
}

pub fn flutter_bootstrap_spin_active() -> bool {
    !flutter_init_ready()
}

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
    let pid = canonical_pid(pid);
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
    {
        let (cx, cy) = crate::drivers::ps2::cursor_pos();
        s.x = cx;
        s.y = cy;
    }
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
    false
}

#[cfg(not(target_arch = "x86_64"))]
fn poll_ps2_input(_s: &mut SynthInput, _max_w: i32, _max_h: i32) -> bool {
    false
}

pub fn event_size() -> usize {
    eabi::WM_EVENT_SIZE
}

pub fn pending_count() -> usize {
    with_queue(|q| q.len)
}

pub fn pending_count_for(pid: u32) -> usize {
    let pid = canonical_pid(pid);
    with_queue(|q| q.pending_for(pid))
}

/// Number of queued pointer/key (input) events visible to `pid`.
/// Used by the epoll fast-path to yield the CPU to a starved WM consumer.
pub fn input_pending_for(pid: u32) -> usize {
    let pid = canonical_pid(pid);
    with_queue(|q| q.input_pending_for(pid))
}

/// True when there is a high-priority input event (key event, or mouse click/drag)
/// queued and visible to `pid`.
pub fn high_priority_input_pending_for(pid: u32) -> bool {
    let pid = canonical_pid(pid);
    with_queue(|q| {
        for off in 0..q.len {
            let idx = (q.head + off) % EVENT_CAP;
            let owner = q.owner_pid[idx];
            if owner_visible_to_consumer(owner, pid) {
                if q.buf[idx].kind == EV_KEY {
                    return true;
                }
                if q.buf[idx].kind == EV_POINTER {
                    if q.buf[idx].flags != 0 || q.buf[idx].b != 0 {
                        return true;
                    }
                }
            }
        }
        false
    })
}

pub fn dropped_count() -> u64 {
    with_queue(|q| q.dropped)
}

/// True when a baton≠0 EV_VSYNC is already in the queue for `pid`.
pub fn baton_vsync_queued_for(pid: u32) -> bool {
    let pid = canonical_pid(pid);
    with_queue(|q| q.has_baton_vsync_for(pid))
}

/// True when engine `pid` must run FlutterEngineOnVsync (a baton-carrying
/// EV_VSYNC is queued for it). Per-engine: each Flutter engine (shell pid 1,
/// each launched app) gets its OWN vsync, so a second engine no longer starves
/// when the shell's baton slot is busy (the cause of the open-an-app freeze).
pub fn embedder_baton_due(pid: u32) -> bool {
    baton_vsync_queued_for(pid)
}

pub fn pop_event_for(pid: u32) -> Option<WmEvent> {
    let pid = canonical_pid(pid);
    let ev = with_queue(|q| q.pop_for(pid))?;
    if ev.kind == EV_VSYNC && ev.b != 0 {
        BATON_VSYNC_QUEUED.store(false, Ordering::Release);
    }
    Some(ev)
}

pub fn push_event(kind: u32, flags: u32, a: u64, b: u64) {
    push_event_for(0, kind, flags, a, b);
}

pub fn push_event_for(owner_pid: u32, kind: u32, flags: u32, a: u64, b: u64) {
    let owner_pid = canonical_pid(owner_pid);
    with_queue(|q| {
        q.push(
            WmEvent {
                seq: 0,
                kind,
                flags,
                a,
                b,
            },
            owner_pid,
        );
    });

    // Wake the waiter if they are eligible for this event. We are now OUTSIDE the
    // WM lock (the with_queue closure closed above), so canonicalizing the raw
    // waiter pid here is safe — owner_visible_to_consumer itself is raw and must
    // be fed canonical pids (owner_pid was canonicalized at fn entry).
    let waiter = WM_WAITER.load(Ordering::Acquire);
    if waiter != 0 && owner_visible_to_consumer(owner_pid, canonical_pid(waiter)) {
        WM_WAITER.store(0, Ordering::Release);
        crate::process::wake_process(waiter);
    }

    // Input events (pointer/key) MUST wake their target consumer even when no
    // WM_WAITER is registered. The embedder host (pid 1) oscillates in and out
    // of wm_event_wait — on a timeout==0 fast-return (platform task due) it
    // exits without setting WM_WAITER, so at the instant a click arrives the
    // waiter handshake is often 0 and the event is dropped on the floor while
    // pid 1 stays frozen mid cooperative-yield and the engine threads hog the
    // single core. Mirror push_vsync (which unconditionally wakes pid 1) so
    // clicks and keystrokes are never lost.
    if kind == EV_POINTER || kind == EV_KEY {
        let target = if owner_pid != 0 {
            owner_pid
        } else {
            focus_pid()
        };
        if target != 0 {
            crate::process::wake_process(target);
        }
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
    if baton != 0 || n < 8 {
        log::info!("[push-vsync] #{} frame={} baton={:#x}", n, frame, baton);
    }
    if baton != 0 {
        let mut ev = WmEvent::empty();
        ev.kind = EV_VSYNC;
        ev.a = frame;
        ev.b = baton;
        with_queue(|q| q.push_front(ev, 0));
        BATON_VSYNC_QUEUED.store(true, Ordering::Release);
        crate::process::wake_process(1);
        let waiter = WM_WAITER.load(Ordering::Acquire);
        if waiter != 0 {
            WM_WAITER.store(0, Ordering::Release);
            crate::process::wake_process(waiter);
        }
    } else {
        push_event(EV_VSYNC, 0, frame, baton);
    }

}

/// Deliver a vsync baton to the engine that posted it, immediately.
///
/// Called via `sys_engine_vsync_baton_post` when an engine invokes its
/// `vsync_callback`. The baton is a "call `FlutterEngineOnVsync` now" signal: it
/// must NOT be parked in `VSYNC_BATON` waiting for the next `compositor::tick()`
/// -> `push_vsync` (that path is gated on `COMP.try_lock()` and stalls if any
/// thread holds the compositor lock). We push the EV_VSYNC event to the front of
/// the POSTING engine's queue and wake it, independent of compositor state.
///
/// `pid` is the engine that posted (the syscall caller). Previously this was
/// hardcoded to pid 1, so a launched app's baton was delivered to the shell and
/// the app's engine never advanced — the second-engine freeze. Now each engine
/// gets its own vsync.
pub fn set_vsync_baton(pid: u32, baton: u64) {
    if baton == 0 {
        VSYNC_BATON.store(0, Ordering::Release);
        return;
    }
    // Don't strand the baton; deliver it now to the posting engine.
    VSYNC_BATON.store(0, Ordering::Release);
    let owner = canonical_pid(pid);
    let mut ev = WmEvent::empty();
    ev.kind = EV_VSYNC;
    ev.a = 0;
    ev.b = baton;
    with_queue(|q| q.push_front(ev, owner));
    BATON_VSYNC_QUEUED.store(true, Ordering::Release);
    crate::process::wake_process(owner);
    let waiter = WM_WAITER.load(Ordering::Acquire);
    if waiter != 0 {
        WM_WAITER.store(0, Ordering::Release);
        crate::process::wake_process(waiter);
    }
    static DELIVER_SEQ: AtomicU32 = AtomicU32::new(0);
    let n = DELIVER_SEQ.fetch_add(1, Ordering::Relaxed);
    if n < 30 || n % 300 == 0 {
        log::info!("[deliver-baton] #{} baton={:#x}", n, baton);
    }
}

static LAST_BUTTONS: AtomicU32 = AtomicU32::new(0);

// Unified software-cursor state. Fed by EVERY pointer source — the x86 PS/2
// driver and the aarch64 virtio-input driver both funnel through push_pointer —
// so the compositor can draw the cursor on any arch (previously it read x86-only
// ps2:: state and never drew on ARM).
static CURSOR_X: core::sync::atomic::AtomicI32 = core::sync::atomic::AtomicI32::new(32);
static CURSOR_Y: core::sync::atomic::AtomicI32 = core::sync::atomic::AtomicI32::new(32);
static CURSOR_BUTTONS: AtomicU32 = AtomicU32::new(0);
static CURSOR_SEEN: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);
static CURSOR_LAST_ACT_NS: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

// ── Input introspection ──────────────────────────────────────────────────────
//
// Lightweight, always-on telemetry: which pointing/key device bound, and how many
// events each kind has delivered. Cheap (a few relaxed atomics per event) and
// genuinely useful for answering "is any input device live, and is it sending
// events?" — the exact question when a board/VM shows no cursor. The optional
// `input-hud` build feature draws these on-screen (see compositor::input_hud); the
// counters themselves are unconditional so the data exists even without the HUD.

/// Which input device last bound a pointer/key source.
pub const INPUT_SRC_NONE: u8 = 0;
pub const INPUT_SRC_VIRTIO: u8 = 1;
pub const INPUT_SRC_XHCI: u8 = 2;
pub const INPUT_SRC_PS2: u8 = 3;

static INPUT_SOURCE: core::sync::atomic::AtomicU8 = core::sync::atomic::AtomicU8::new(INPUT_SRC_NONE);
static PTR_COUNT: AtomicU32 = AtomicU32::new(0);
static KEY_COUNT: AtomicU32 = AtomicU32::new(0);
static SCROLL_COUNT: AtomicU32 = AtomicU32::new(0);
/// Last key event, packed `(scancode << 1) | pressed`.
static LAST_KEY: AtomicU32 = AtomicU32::new(0);

/// Record that an input device has bound (called once per device from its driver).
pub fn set_input_source(src: u8) {
    INPUT_SOURCE.store(src, Ordering::Relaxed);
}
/// The bound input source (`INPUT_SRC_*`), or `INPUT_SRC_NONE` if nothing bound.
pub fn input_source() -> u8 {
    INPUT_SOURCE.load(Ordering::Relaxed)
}
/// Short name for the bound input source.
pub fn input_source_name() -> &'static str {
    match input_source() {
        INPUT_SRC_VIRTIO => "virtio",
        INPUT_SRC_XHCI => "xhci",
        INPUT_SRC_PS2 => "ps2",
        _ => "none",
    }
}
/// Event counts so far: `(pointer, key, scroll)`.
pub fn input_counts() -> (u32, u32, u32) {
    (
        PTR_COUNT.load(Ordering::Relaxed),
        KEY_COUNT.load(Ordering::Relaxed),
        SCROLL_COUNT.load(Ordering::Relaxed),
    )
}
/// Last key event as `(scancode, pressed)`.
pub fn last_key() -> (u32, bool) {
    let v = LAST_KEY.load(Ordering::Relaxed);
    (v >> 1, (v & 1) != 0)
}

/// Current pointer position (absolute, framebuffer pixels).
pub fn cursor_pos() -> (i32, i32) {
    (CURSOR_X.load(Ordering::Relaxed), CURSOR_Y.load(Ordering::Relaxed))
}
/// Current pressed-button bitmask.
pub fn cursor_buttons() -> u32 {
    CURSOR_BUTTONS.load(Ordering::Relaxed)
}
/// True once any pointer event has arrived (a pointing device is live).
pub fn cursor_seen() -> bool {
    CURSOR_SEEN.load(Ordering::Relaxed)
}
/// Monotonic-ns timestamp of the last pointer activity (for idle auto-hide).
pub fn cursor_last_act_ns() -> u64 {
    CURSOR_LAST_ACT_NS.load(Ordering::Relaxed)
}

pub fn push_pointer(x: i32, y: i32, buttons: u32) {
    // Record cursor state for the compositor's software cursor (arch-neutral).
    CURSOR_X.store(x, Ordering::Relaxed);
    CURSOR_Y.store(y, Ordering::Relaxed);
    CURSOR_BUTTONS.store(buttons, Ordering::Relaxed);
    CURSOR_SEEN.store(true, Ordering::Relaxed);
    CURSOR_LAST_ACT_NS.store(crate::arch::rdtsc_ns(), Ordering::Relaxed);
    PTR_COUNT.fetch_add(1, Ordering::Relaxed);

    let packed = ((x as u32 as u64) << 32) | (y as u32 as u64);

    // Detect button transition (press or release) to flag it as high-priority.
    let last = LAST_BUTTONS.swap(buttons, Ordering::AcqRel);
    let is_transition = buttons != last;
    let b_val = if is_transition { 1 } else { 0 };

    // Route to focused process first so the active UI always gets click input.
    // Falling back to hit-test owner is useful when no explicit focus is set.
    let focus = focus_pid();

    if focus != 0 {
        push_event_for(focus, EV_POINTER, buttons, packed, b_val);
        return;
    }

    // In bypass mode a userspace process owns the framebuffer directly.
    // Skip compositor hit-testing and broadcast if there is no explicit focus.
    if !crate::compositor::is_fb_bypass() {
        if let Some((_surf_id, owner_pid)) = crate::compositor::surface_at_point(x, y) {
            push_event_for(owner_pid, EV_POINTER, buttons, packed, b_val);
            return;
        }
    }

    if focus == 0 {
        // Avoid broadcast consumption races: route fallback pointer events
        // to the engine host (pid 1) so Flutter reliably receives clicks.
        push_event_for(1, EV_POINTER, buttons, packed, b_val);
    }
}

/// Deliver a mouse scroll-wheel tick. `dz` is the signed wheel delta
/// (negative = scroll toward the user / content down, positive = away / up).
pub fn push_scroll(x: i32, y: i32, dz: i32) {
    SCROLL_COUNT.fetch_add(1, Ordering::Relaxed);
    let packed = ((x as u32 as u64) << 32) | (y as u32 as u64);
    let dz_bits = dz as u32 as u64; // preserve sign through i32→u32→u64

    let focus = focus_pid();
    if focus != 0 {
        push_event_for(focus, EV_SCROLL, 0, packed, dz_bits);
        return;
    }

    if !crate::compositor::is_fb_bypass() {
        if let Some((_surf_id, owner_pid)) = crate::compositor::surface_at_point(x, y) {
            push_event_for(owner_pid, EV_SCROLL, 0, packed, dz_bits);
            return;
        }
    }

    push_event_for(1, EV_SCROLL, 0, packed, dz_bits);
}

pub fn push_key(scancode: u32, pressed: bool) {
    KEY_COUNT.fetch_add(1, Ordering::Relaxed);
    LAST_KEY.store((scancode << 1) | (pressed as u32), Ordering::Relaxed);
    // F2 (PS/2 set-1 make 0x3C) toggles the kernel boot screen's verbose log
    // overlay. Handled here so it works during the engine warm-up before any app
    // is focused; the keypress is still forwarded for normal handling.
    if pressed && scancode == 0x3C {
        crate::drivers::bootscreen::toggle_verbose();
    }
    let focus = focus_pid();
    let flags = if pressed { 1 } else { 0 };
    if focus == 0 {
        push_event(EV_KEY, flags, scancode as u64, 0);
    } else {
        push_event_for(focus, EV_KEY, flags, scancode as u64, 0);
    }
}
