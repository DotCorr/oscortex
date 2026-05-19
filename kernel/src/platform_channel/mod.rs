//! Platform channel message bridge — Phase 32-C.
//!
//! Provides a kernel-side mailbox for bidirectional Flutter platform channel
//! messages between native kernel services and the Dart/Flutter engine running
//! in the engine-host process.
//!
//! ## Message flow
//!
//! ```text
//! Native syscall                  Flutter engine (embedder loop)
//! ─────────────────────────────   ───────────────────────────────
//! sys_platform_msg_post(ch, data) →  EV_PLATFORM_MSG event pushed
//!                                     embedder calls sys_platform_msg_recv
//!                                     embedder processes message in Dart
//!                                 ←  sys_platform_msg_reply(seq, reply)
//! sys_platform_msg_ack(seq)       ←  returns reply bytes
//! ```

use alloc::vec::Vec;
use spin::Mutex;

const MAX_PENDING: usize = 32;
/// Maximum platform channel message payload (32 KiB).
pub const MAX_PAYLOAD: usize = 32 * 1024;

pub struct PlatformMsg {
    pub seq:        u64,
    pub sender_pid: u32,
    pub channel:    Vec<u8>,
    pub payload:    Vec<u8>,
    /// Set once the engine-host calls `sys_platform_msg_reply`.
    pub reply:      Option<Vec<u8>>,
}

struct MsgBox {
    msgs:     Vec<PlatformMsg>,
    next_seq: u64,
}

impl MsgBox {
    const fn new() -> Self {
        Self { msgs: Vec::new(), next_seq: 1 }
    }
}

static MBOX: Mutex<MsgBox> = Mutex::new(MsgBox::new());

// ── Public API ────────────────────────────────────────────────────────────────

/// Post a platform-channel message from native kernel code.
///
/// Returns the sequence number that the caller uses to poll for a reply,
/// or `Err` if the queue is full or the payload is too large.
pub fn post(sender_pid: u32, channel: &[u8], payload: &[u8]) -> Result<u64, &'static str> {
    if payload.len() > MAX_PAYLOAD {
        return Err("payload too large");
    }
    let mut mb = MBOX.lock();
    if mb.msgs.len() >= MAX_PENDING {
        return Err("platform message queue full");
    }
    let seq = mb.next_seq;
    mb.next_seq = mb.next_seq.wrapping_add(1).max(1);
    mb.msgs.push(PlatformMsg {
        seq,
        sender_pid,
        channel: channel.to_vec(),
        payload: payload.to_vec(),
        reply:   None,
    });
    Ok(seq)
}

/// Serialise the oldest pending message into `buf`.
///
/// Wire format: `[seq:u64][channel_len:u16][payload_len:u32][channel bytes][payload bytes]`
///
/// Returns the number of bytes written, or 0 if no pending messages.
pub fn recv_into(buf: &mut [u8]) -> usize {
    let mb = MBOX.lock();
    // Find the first message without a reply.
    let msg = mb.msgs.iter().find(|m| m.reply.is_none());
    let msg = match msg {
        Some(m) => m,
        None    => return 0,
    };

    let header_sz = 8 + 2 + 4; // seq + channel_len + payload_len
    let total = header_sz + msg.channel.len() + msg.payload.len();
    if buf.len() < total {
        return 0; // EFAULT-equivalent: caller must provide large enough buffer
    }

    let mut off = 0usize;
    buf[off..off+8].copy_from_slice(&msg.seq.to_le_bytes());   off += 8;
    buf[off..off+2].copy_from_slice(&(msg.channel.len() as u16).to_le_bytes()); off += 2;
    buf[off..off+4].copy_from_slice(&(msg.payload.len() as u32).to_le_bytes()); off += 4;
    buf[off..off+msg.channel.len()].copy_from_slice(&msg.channel); off += msg.channel.len();
    buf[off..off+msg.payload.len()].copy_from_slice(&msg.payload);
    total
}

/// Record a reply from the engine host for message `seq`.
pub fn reply(seq: u64, reply_data: &[u8]) -> Result<(), &'static str> {
    let mut mb = MBOX.lock();
    for msg in mb.msgs.iter_mut() {
        if msg.seq == seq {
            msg.reply = Some(reply_data.to_vec());
            return Ok(());
        }
    }
    Err("no such sequence")
}

/// Copy a reply into `buf` and remove the message from the queue.
/// Returns byte count written, or 0 if no reply yet.
pub fn ack(seq: u64, buf: &mut [u8]) -> usize {
    let mut mb = MBOX.lock();
    let pos = mb.msgs.iter().position(|m| m.seq == seq && m.reply.is_some());
    let idx = match pos { Some(i) => i, None => return 0 };
    let reply = mb.msgs[idx].reply.take().unwrap();
    mb.msgs.remove(idx);
    let copy_len = reply.len().min(buf.len());
    buf[..copy_len].copy_from_slice(&reply[..copy_len]);
    copy_len
}

/// Push an `EV_PLATFORM_MSG` WM event (broadcast) so the engine host
/// process knows a message is waiting in the queue.
/// `seq` is the message sequence number; `channel_hash` is a 32-bit FNV-1a
/// hash of the channel name for quick filtering.
pub fn notify_engine_host(seq: u64, channel_hash: u32) {
    use crate::embedder::abi as eabi;
    // Broadcast (owner_pid = 0); the engine host filters via pop_for.
    crate::wm::push_event(eabi::EV_PLATFORM_MSG, channel_hash, seq, 0);
}

/// Drain all messages belonging to dead PIDs (called on process exit).
pub fn reap_pid(pid: u32) {
    let mut mb = MBOX.lock();
    mb.msgs.retain(|m| m.sender_pid != pid);
}

