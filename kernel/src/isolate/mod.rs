//! Dart Isolate Lifecycle Manager — Phase 35-A.
//!
//! Tracks all live Dart isolates (one per entry point inside an AOT snapshot)
//! and provides the kernel-internal API used by syscall handlers.
//!
//! ## Layout
//!
//! An isolate is a Dart concurrent-execution unit backed by a kernel thread
//! (a `spawn_thread` slot).  It has its own heap managed by the Dart VM;
//! from the kernel's perspective it is simply a thread that runs inside a
//! mapped AOT snapshot region.
//!
//! ## State machine
//!
//! ```text
//!  ┌──────────┐  spawn()   ┌─────────┐  pause()  ┌──────────┐
//!  │ (none)   │ ────────►  │ Running │ ─────────► │  Paused  │
//!  └──────────┘            └────┬────┘ ◄───────── └──────────┘
//!                               │ kill()    resume()
//!                               ▼
//!                           ┌──────┐
//!                           │ Dead │
//!                           └──────┘
//! ```
//!
//! A Dead slot may be reclaimed by a subsequent `spawn()`.

use spin::Mutex;

/// Maximum simultaneous live isolates.
pub const MAX_ISOLATES: usize = 64;

/// Isolate lifecycle state (matches the `op=0` return values in the syscall ABI).
#[derive(Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum IsolateState {
    Running = 1,
    Paused  = 2,
    Dead    = 3,
}

/// Kernel-side record for a single Dart isolate.
#[derive(Clone, Copy)]
pub struct IsolateHandle {
    /// Monotonically increasing handle value, 1-based.
    pub id:          u32,
    /// State.
    pub state:       IsolateState,
    /// PID of the process that owns this isolate.
    pub owner_pid:   u32,
    /// Thread ID returned by `process::spawn_thread`.
    pub thread_id:   u32,
    /// VA of the mapped AOT snapshot (set at spawn; used for debugging).
    pub aot_va:      u64,
    /// Size (bytes) of the AOT snapshot region.
    pub aot_size:    u64,
}

struct IsolateTable {
    slots:    [Option<IsolateHandle>; MAX_ISOLATES],
    next_id:  u32,
}

impl IsolateTable {
    const fn new() -> Self {
        Self {
            slots:   [const { None }; MAX_ISOLATES],
            next_id: 1,
        }
    }

    fn alloc_slot(&mut self) -> Option<usize> {
        // Prefer a truly empty slot; fall back to a Dead one.
        let mut dead = None;
        for (i, s) in self.slots.iter().enumerate() {
            match s {
                None => return Some(i),
                Some(h) if h.state == IsolateState::Dead => {
                    if dead.is_none() { dead = Some(i); }
                }
                _ => {}
            }
        }
        dead
    }

    fn find_mut(&mut self, id: u32) -> Option<&mut IsolateHandle> {
        self.slots.iter_mut()
            .filter_map(|s| s.as_mut())
            .find(|h| h.id == id && h.state != IsolateState::Dead)
    }

    fn find(&self, id: u32) -> Option<&IsolateHandle> {
        self.slots.iter()
            .filter_map(|s| s.as_ref())
            .find(|h| h.id == id)
    }
}

static TABLE: Mutex<IsolateTable> = Mutex::new(IsolateTable::new());

// ── Public kernel API ─────────────────────────────────────────────────────────

/// Spawn a new Dart isolate in `owner_pid`'s address space.
///
/// `aot_va` and `aot_size` identify the previously mapped snapshot region
/// (returned by `SYS_AOT_SNAPSHOT_LOAD`).  `entry_offset` is the byte offset
/// inside the snapshot where execution begins (0 for the root isolate).
/// `stack_size` of 0 defaults to 128 KiB.
///
/// Returns the new isolate ID on success, or `Err` on failure.
pub fn spawn(
    owner_pid:    u32,
    aot_va:       u64,
    aot_size:     u64,
    entry_offset: u64,
    stack_size:   usize,
) -> Result<u32, &'static str> {
    if aot_va == 0 { return Err("invalid aot_va"); }
    if entry_offset >= aot_size.max(1) { return Err("entry_offset out of range"); }

    let stack = if stack_size == 0 { 131_072 } else { stack_size };
    let entry_va = aot_va + entry_offset;

    // Spawn the backing kernel thread.
    let thread_id = crate::process::spawn_thread(owner_pid, entry_va, 0, stack)
        .map_err(|_| "spawn_thread failed")?;

    let mut table = TABLE.lock();
    let slot = table.alloc_slot().ok_or("isolate table full")?;
    let id = table.next_id;
    table.next_id = table.next_id.wrapping_add(1).max(1);

    table.slots[slot] = Some(IsolateHandle {
        id,
        state: IsolateState::Running,
        owner_pid,
        thread_id,
        aot_va,
        aot_size,
    });

    log::info!(
        "[isolate] spawned id={} owner={} entry={:#x} tid={}",
        id, owner_pid, entry_va, thread_id
    );

    // Create the isolate's message inbox.
    crate::isolate_msg::create_inbox(id);

    // Notify the WM engine-host about the new isolate.
    crate::wm::push_event(
        crate::embedder::abi::EV_ISOLATE,
        IsolateState::Running as u32,
        id as u64,
        owner_pid as u64,
    );

    Ok(id)
}

/// Kill a live isolate.  The backing thread is terminated; the slot is
/// marked Dead so its ID is never reused.
pub fn kill(id: u32) -> Result<(), &'static str> {
    let thread_id = {
        let mut table = TABLE.lock();
        let h = table.find_mut(id).ok_or("no such isolate")?;
        let tid = h.thread_id;
        h.state = IsolateState::Dead;
        tid
    };

    // Best-effort thread kill — ignore error if the thread already exited.
    let _ = crate::process::kill(thread_id);

    // Tear down the message inbox.
    crate::isolate_msg::destroy_inbox(id);

    crate::wm::push_event(
        crate::embedder::abi::EV_ISOLATE,
        IsolateState::Dead as u32,
        id as u64,
        0,
    );
    log::info!("[isolate] killed id={} tid={}", id, thread_id);
    Ok(())
}

/// Query or change an isolate's state.
///
/// `op`:
///   * `0` — get state: returns `IsolateState as u32` (1/2/3)
///   * `1` — pause:  marks state as Paused, sends SIGSTOP-equivalent (stub)
///   * `2` — resume: marks state as Running
pub fn ctrl(id: u32, op: u32) -> Result<u32, &'static str> {
    let mut table = TABLE.lock();
    match op {
        0 => {
            // Get state — allow querying Dead slots too.
            let h = table.find(id).ok_or("no such isolate")?;
            Ok(h.state as u32)
        }
        1 => {
            let h = table.find_mut(id).ok_or("no such isolate")?;
            if h.state == IsolateState::Running {
                h.state = IsolateState::Paused;
                let iid = h.id;
                let pid = h.owner_pid;
                drop(table);
                crate::wm::push_event(
                    crate::embedder::abi::EV_ISOLATE,
                    IsolateState::Paused as u32,
                    iid as u64,
                    pid as u64,
                );
            }
            Ok(IsolateState::Paused as u32)
        }
        2 => {
            let h = table.find_mut(id).ok_or("no such isolate")?;
            if h.state == IsolateState::Paused {
                h.state = IsolateState::Running;
                let iid = h.id;
                let pid = h.owner_pid;
                drop(table);
                crate::wm::push_event(
                    crate::embedder::abi::EV_ISOLATE,
                    IsolateState::Running as u32,
                    iid as u64,
                    pid as u64,
                );
            }
            Ok(IsolateState::Running as u32)
        }
        _ => Err("unknown op"),
    }
}
