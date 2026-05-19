//! Phase 45 — Minimal kernel networking layer.
//!
//! Sits on top of `drivers::virtio_net` and provides:
//!   * ARP cache (IP → MAC, 16 entries)
//!   * Single UDP socket (recv ring-buffer of 8 frames)
//!   * Phase 50: smoltcp TCP/IP via `tcp` submodule

pub mod tcp;

use spin::Mutex;

// ── ARP cache ─────────────────────────────────────────────────────────────────

const ARP_SLOTS: usize = 16;

struct ArpEntry {
    ip:  u32,
    mac: [u8; 6],
    valid: bool,
}

impl ArpEntry {
    const fn empty() -> Self { Self { ip: 0, mac: [0; 6], valid: false } }
}

struct NetState {
    arp:       [ArpEntry; ARP_SLOTS],
    local_ip:  u32,
    // Simple recv ring.
    rx_ring:   [[u8; 1528]; 8],
    rx_len:    [usize; 8],
    rx_head:   usize,
    rx_tail:   usize,
}

impl NetState {
    const fn new() -> Self {
        Self {
            arp:      [const { ArpEntry::empty() }; ARP_SLOTS],
            local_ip: 0,
            rx_ring:  [[0u8; 1528]; 8],
            rx_len:   [0usize; 8],
            rx_head:  0,
            rx_tail:  0,
        }
    }
}

static NET_STATE: Mutex<NetState> = Mutex::new(NetState::new());

// ── Public API ────────────────────────────────────────────────────────────────

/// Initialise the networking layer.  Called after `virtio_net::init()`.
pub fn init() {
    log::info!("[net] UDP stack online (virtio-net backend)");
    // Phase 50: initialise smoltcp TCP/IP stack.
    tcp::init_smoltcp();
}

/// Format a text line with MAC and IP info.
///
/// Delegates to `virtio_net::info_text()`.
pub fn info_text(buf_ptr: *mut u8, cap: usize) -> usize {
    if buf_ptr.is_null() || cap == 0 { return 0; }
    let mut tmp = [0u8; 256];
    let cap2 = cap.min(tmp.len());
    let n = crate::drivers::virtio_net::info_text(&mut tmp[..cap2]);
    unsafe { core::ptr::copy_nonoverlapping(tmp.as_ptr(), buf_ptr, n); }
    n
}

/// Send a UDP datagram (wrapped in a minimal Ethernet + IPv4 + UDP frame).
///
/// This is a best-effort fire-and-forget — it does NOT wait for ARP.
pub fn send_udp(dst_ip: u32, dst_port: u16, data: &[u8]) -> Result<(), &'static str> {
    if !crate::drivers::virtio_net::is_ready() { return Err("no NIC"); }
    if data.len() > 1472 { return Err("payload too large"); }

    let src_ip = NET_STATE.lock().local_ip;
    let src_port: u16 = 49152; // ephemeral

    // Resolve dst MAC from ARP cache; fall back to broadcast.
    let dst_mac = arp_lookup(dst_ip).unwrap_or([0xFF; 6]);
    let src_mac = [0x52, 0x54, 0x00, 0x12, 0x34, 0x56]; // QEMU virtio default

    let udp_len = 8 + data.len();
    let ip_len  = 20 + udp_len;
    let frame_len = 14 + ip_len;

    let mut frame = [0u8; 14 + 20 + 8 + 1472];

    // ── Ethernet header ───────────────────────────────────────────────────────
    frame[0..6].copy_from_slice(&dst_mac);
    frame[6..12].copy_from_slice(&src_mac);
    frame[12] = 0x08; frame[13] = 0x00; // IPv4

    // ── IPv4 header ───────────────────────────────────────────────────────────
    let ip = &mut frame[14..14 + 20];
    ip[0]  = 0x45; // version=4, ihl=5
    ip[1]  = 0;
    ip[2]  = (ip_len >> 8) as u8;
    ip[3]  = (ip_len & 0xFF) as u8;
    ip[4]  = 0; ip[5] = 1; // id
    ip[6]  = 0x40; ip[7] = 0; // DF, frag offset
    ip[8]  = 64; // TTL
    ip[9]  = 17; // protocol UDP
    // checksum at [10..12] = 0 (compute below)
    ip[12] = (src_ip >> 24) as u8;
    ip[13] = (src_ip >> 16) as u8;
    ip[14] = (src_ip >>  8) as u8;
    ip[15] =  src_ip        as u8;
    ip[16] = (dst_ip >> 24) as u8;
    ip[17] = (dst_ip >> 16) as u8;
    ip[18] = (dst_ip >>  8) as u8;
    ip[19] =  dst_ip        as u8;
    let ip_chk = ip4_checksum(&frame[14..14 + 20]);
    frame[24] = (ip_chk >> 8) as u8;
    frame[25] = (ip_chk & 0xFF) as u8;

    // ── UDP header ────────────────────────────────────────────────────────────
    let udp = &mut frame[34..34 + 8];
    udp[0] = (src_port >> 8) as u8;
    udp[1] = (src_port & 0xFF) as u8;
    udp[2] = (dst_port >> 8) as u8;
    udp[3] = (dst_port & 0xFF) as u8;
    udp[4] = (udp_len >> 8) as u8;
    udp[5] = (udp_len & 0xFF) as u8;
    udp[6] = 0; udp[7] = 0; // checksum optional for UDP/IPv4

    // ── Payload ───────────────────────────────────────────────────────────────
    frame[42..42 + data.len()].copy_from_slice(data);

    crate::drivers::virtio_net::send_raw(&frame[..frame_len])
}

/// Poll for a received UDP datagram.
///
/// Returns `Some((len, src_ip, src_port))` if available.
pub fn recv_udp(buf: &mut [u8]) -> Option<(usize, u32, u16)> {
    // First drain any raw frames from virtio-net into our ring.
    let mut raw = [0u8; 1528];
    while let Some(n) = crate::drivers::virtio_net::recv_raw(&mut raw) {
        // Parse enough of the frame to find UDP.
        if n >= 42 && raw[12] == 0x08 && raw[13] == 0x00 // IPv4
            && raw[23] == 17 // IP protocol UDP
        {
            enqueue_raw_frame(&raw[..n]);
        }
    }

    // Pop from ring.
    let mut ns = NET_STATE.lock();
    if ns.rx_head == ns.rx_tail { return None; }
    let idx = ns.rx_head % 8;
    let len = ns.rx_len[idx];
    if len == 0 { ns.rx_head = ns.rx_head.wrapping_add(1); return None; }

    // Parse src IP + port from frame.
    let frame = &ns.rx_ring[idx];
    let src_ip = u32::from_be_bytes([frame[26], frame[27], frame[28], frame[29]]);
    let src_port = u16::from_be_bytes([frame[34], frame[35]]);

    // UDP payload starts at byte 42.
    let payload_len = if len > 42 { len - 42 } else { 0 };
    let copy = payload_len.min(buf.len());
    buf[..copy].copy_from_slice(&frame[42..42 + copy]);

    ns.rx_len[idx] = 0;
    ns.rx_head = ns.rx_head.wrapping_add(1);

    Some((copy, src_ip, src_port))
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn enqueue_raw_frame(frame: &[u8]) {
    let mut ns = NET_STATE.lock();
    if ns.rx_tail.wrapping_sub(ns.rx_head) >= 8 { return; } // ring full
    let idx = ns.rx_tail % 8;
    let copy = frame.len().min(1528);
    ns.rx_ring[idx][..copy].copy_from_slice(&frame[..copy]);
    ns.rx_len[idx] = copy;
    ns.rx_tail = ns.rx_tail.wrapping_add(1);
}

fn arp_lookup(ip: u32) -> Option<[u8; 6]> {
    let ns = NET_STATE.lock();
    for e in ns.arp.iter() {
        if e.valid && e.ip == ip { return Some(e.mac); }
    }
    None
}

fn ip4_checksum(header: &[u8]) -> u16 {
    let mut sum: u32 = 0;
    for chunk in header.chunks(2) {
        let word = if chunk.len() == 2 {
            ((chunk[0] as u32) << 8) | (chunk[1] as u32)
        } else {
            (chunk[0] as u32) << 8
        };
        sum += word;
        if sum > 0xFFFF { sum -= 0xFFFF; }
    }
    !(sum as u16)
}
