//! IPC — inter-process communication.
//!
//! Each process has a fixed-size ring-buffer inbox (capacity = 16 × 64-byte
//! messages).  `send()` places a message in the destination inbox; `recv()`
//! pops the oldest message from the caller's inbox.
//!
//! This is a single-copy, non-blocking design.  A future revision will add
//! a blocking recv() variant that puts the caller to sleep until a message
//! arrives.

use spin::Mutex;
use crate::process::MAX_PROCS;

// ── Message ───────────────────────────────────────────────────────────────────

/// Maximum message payload length.
pub const MSG_LEN: usize = 64;
/// Ring-buffer capacity per process (number of messages).
const INBOX_CAP: usize = 16;

#[derive(Copy, Clone)]
pub struct Message {
    pub len:  usize,
    pub data: [u8; MSG_LEN],
}

impl Message {
    const fn empty() -> Self { Self { len: 0, data: [0u8; MSG_LEN] } }
}

// ── Inbox ─────────────────────────────────────────────────────────────────────

struct Inbox {
    buf:  [Message; INBOX_CAP],
    head: usize,
    tail: usize,
    len:  usize,
}

impl Inbox {
    const fn new() -> Self {
        Self {
            buf:  [const { Message::empty() }; INBOX_CAP],
            head: 0,
            tail: 0,
            len:  0,
        }
    }

    fn push(&mut self, msg: Message) -> bool {
        if self.len == INBOX_CAP { return false; } // drop: inbox full
        self.buf[self.tail] = msg;
        self.tail = (self.tail + 1) % INBOX_CAP;
        self.len += 1;
        true
    }

    fn pop(&mut self) -> Option<Message> {
        if self.len == 0 { return None; }
        let msg = self.buf[self.head];
        self.head = (self.head + 1) % INBOX_CAP;
        self.len -= 1;
        Some(msg)
    }
}

// ── Global inbox table ────────────────────────────────────────────────────────

static INBOXES: Mutex<[Inbox; MAX_PROCS]> =
    Mutex::new([const { Inbox::new() }; MAX_PROCS]);

// ── Public API ────────────────────────────────────────────────────────────────

/// Send a message to process `dst_pid`.
/// Silently drops the message if the inbox is full or `dst_pid` is out of range.
pub fn send(dst_pid: u32, data: &[u8]) {
    if dst_pid as usize >= MAX_PROCS { return; }
    let copy_len = data.len().min(MSG_LEN);
    let mut msg = Message::empty();
    msg.len = copy_len;
    msg.data[..copy_len].copy_from_slice(&data[..copy_len]);
    INBOXES.lock()[dst_pid as usize].push(msg);
}

/// Receive the oldest message from `src_pid`'s inbox.
/// Returns `None` if the inbox is empty.
pub fn recv(src_pid: u32) -> Option<&'static [u8]> {
    if src_pid as usize >= MAX_PROCS { return None; }
    // We pop into a static buffer — safe because IPC is serialised by the lock.
    static mut RECV_BUF: Message = Message::empty();
    let msg = INBOXES.lock()[src_pid as usize].pop()?;
    // SAFETY: single-threaded dispatch path; no re-entrancy.
    unsafe {
        RECV_BUF = msg;
        Some(&RECV_BUF.data[..RECV_BUF.len])
    }
}

pub fn init() {
    log::info!("[IPC] Message-passing subsystem online (cap={} msgs/proc)", INBOX_CAP);
}

