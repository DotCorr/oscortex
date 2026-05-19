//! Isolate message-passing bridge — Phase 36-A.
//!
//! Implements the kernel-side mailbox infrastructure for Dart isolate
//! `SendPort` / `ReceivePort` semantics.
//!
//! ## Design
//!
//! Each live isolate has an *inbox* — a bounded FIFO of variable-length byte
//! blobs.  A sender identifies the destination by isolate ID; the kernel
//! routes the message into that isolate's inbox and pushes an `EV_ISOLATE_MSG`
//! event so the embedder knows to call `isolate_msg_recv`.
//!
//! ## Wire format
//!
//! Messages are opaque byte blobs from the kernel's perspective.  The
//! embedder is responsible for Dart-level serialisation (standard object-
//! transfer encoding).  Maximum per-message payload is `MAX_MSG_SIZE`.
//!
//! ## Thread safety
//!
//! A single `spin::Mutex` guards the entire inbox table.  Lock contention is
//! expected to be low: most operations are a handful of Vec operations.

use alloc::vec::Vec;
use spin::Mutex;

use crate::isolate::MAX_ISOLATES;

/// Maximum size of a single isolate message payload (64 KiB).
pub const MAX_MSG_SIZE: usize = 64 * 1024;

/// Maximum messages queued per isolate inbox before the oldest is dropped.
pub const INBOX_DEPTH: usize = 128;

// ── Per-isolate inbox ─────────────────────────────────────────────────────────

struct Inbox {
    /// Which isolate owns this inbox.
    isolate_id: u32,
    /// Pending messages, oldest first.
    msgs:       Vec<Vec<u8>>,
    /// Total messages dropped due to overflow (saturating).
    dropped:    u64,
}

impl Inbox {
    fn new(id: u32) -> Self {
        Self { isolate_id: id, msgs: Vec::new(), dropped: 0 }
    }

    fn push(&mut self, data: &[u8]) {
        if self.msgs.len() >= INBOX_DEPTH {
            // Drop the oldest message to make room (back-pressure signal).
            self.msgs.remove(0);
            self.dropped = self.dropped.saturating_add(1);
        }
        self.msgs.push(data.to_vec());
    }

    fn pop_into(&mut self, buf: &mut [u8]) -> usize {
        if self.msgs.is_empty() {
            return 0;
        }
        let msg = self.msgs.remove(0);
        let n = msg.len().min(buf.len());
        buf[..n].copy_from_slice(&msg[..n]);
        n
    }

    fn peek_len(&self) -> Option<usize> {
        self.msgs.first().map(|m| m.len())
    }
}

// ── Global inbox table ────────────────────────────────────────────────────────

struct InboxTable {
    slots: [Option<Inbox>; MAX_ISOLATES],
}

impl InboxTable {
    const fn new() -> Self {
        Self { slots: [const { None }; MAX_ISOLATES] }
    }

    fn slot_for(&self, id: u32) -> Option<usize> {
        self.slots.iter().position(|s| {
            s.as_ref().map_or(false, |i| i.isolate_id == id)
        })
    }

    fn slot_for_mut(&mut self, id: u32) -> Option<&mut Inbox> {
        self.slots.iter_mut()
            .filter_map(|s| s.as_mut())
            .find(|i| i.isolate_id == id)
    }
}

static TABLE: Mutex<InboxTable> = Mutex::new(InboxTable::new());

// ── Public kernel API ─────────────────────────────────────────────────────────

/// Create an inbox for `isolate_id`.  Called by `isolate::spawn` after the
/// isolate handle is inserted into the isolate table.
pub fn create_inbox(isolate_id: u32) {
    let mut t = TABLE.lock();
    // Find a free slot.
    for slot in t.slots.iter_mut() {
        if slot.is_none() {
            *slot = Some(Inbox::new(isolate_id));
            return;
        }
    }
    log::warn!("[isolate_msg] inbox table full — could not create inbox for isolate {}", isolate_id);
}

/// Destroy the inbox for `isolate_id`.  Called by `isolate::kill`.
pub fn destroy_inbox(isolate_id: u32) {
    let mut t = TABLE.lock();
    for slot in t.slots.iter_mut() {
        if slot.as_ref().map_or(false, |i| i.isolate_id == isolate_id) {
            *slot = None;
            return;
        }
    }
}

/// Send `data` to the inbox of isolate `dst_id`.
///
/// Pushes an `EV_ISOLATE_MSG` event so the embedder's event loop wakes up.
/// Returns `Ok(())` on success or `Err` if the destination has no inbox
/// or the payload exceeds `MAX_MSG_SIZE`.
pub fn send(dst_id: u32, data: &[u8]) -> Result<(), &'static str> {
    if data.len() > MAX_MSG_SIZE {
        return Err("message too large");
    }
    {
        let mut t = TABLE.lock();
        let inbox = t.slot_for_mut(dst_id).ok_or("no inbox for isolate")?;
        inbox.push(data);
    }
    // Notify the engine-host's event loop.
    crate::wm::push_event(
        crate::embedder::abi::EV_ISOLATE_MSG,
        0,
        dst_id as u64,
        data.len() as u64,
    );
    Ok(())
}

/// Copy the next pending message for `isolate_id` into `buf`.
///
/// Returns the number of bytes written, or 0 if the inbox is empty.
/// If `buf` is smaller than the first message the message is truncated and
/// consumed (the caller should use `msg_peek_len` to size the buffer first).
pub fn recv(isolate_id: u32, buf: &mut [u8]) -> usize {
    let mut t = TABLE.lock();
    match t.slot_for_mut(isolate_id) {
        Some(inbox) => inbox.pop_into(buf),
        None => 0,
    }
}

/// Return the byte-length of the next pending message for `isolate_id`,
/// or `None` if the inbox is empty or the isolate has no inbox.
pub fn peek_len(isolate_id: u32) -> Option<usize> {
    let t = TABLE.lock();
    t.slots.iter()
        .filter_map(|s| s.as_ref())
        .find(|i| i.isolate_id == isolate_id)
        .and_then(|i| i.peek_len())
}

/// Return the number of pending messages in `isolate_id`'s inbox.
pub fn pending(isolate_id: u32) -> usize {
    let t = TABLE.lock();
    t.slots.iter()
        .filter_map(|s| s.as_ref())
        .find(|i| i.isolate_id == isolate_id)
        .map_or(0, |i| i.msgs.len())
}

/// Return the total dropped message count for `isolate_id`.
pub fn dropped(isolate_id: u32) -> u64 {
    let t = TABLE.lock();
    t.slots.iter()
        .filter_map(|s| s.as_ref())
        .find(|i| i.isolate_id == isolate_id)
        .map_or(0, |i| i.dropped)
}
