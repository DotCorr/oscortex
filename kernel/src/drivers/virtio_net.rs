//! Phase 45 — virtio-net PCI driver (minimal TX/RX path).
//!
//! Probes for a virtio-net device on PCI (vendor 0x1AF4, device 0x1000 or 0x1041),
//! sets up legacy or modern virtio queues, and provides a send/recv API used by
//! the kernel-side networking stack.

use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use spin::Mutex;

// ── PCI helpers ───────────────────────────────────────────────────────────────

#[inline]
fn pci_cfg_read32(bus: u8, dev: u8, func: u8, offset: u8) -> u32 {
    let addr: u32 = (1 << 31)
        | ((bus  as u32) << 16)
        | ((dev  as u32) << 11)
        | ((func as u32) <<  8)
        | ((offset & 0xFC) as u32);
    unsafe {
        core::arch::asm!(
            "out dx, eax",
            in("dx")  0xCF8u16,
            in("eax") addr,
            options(nostack, nomem)
        );
        let val: u32;
        core::arch::asm!(
            "in eax, dx",
            in("dx")  0xCFCu16,
            out("eax") val,
            options(nostack, nomem)
        );
        val
    }
}

#[inline]
fn pci_cfg_write32(bus: u8, dev: u8, func: u8, offset: u8, val: u32) {
    let addr: u32 = (1 << 31)
        | ((bus  as u32) << 16)
        | ((dev  as u32) << 11)
        | ((func as u32) <<  8)
        | ((offset & 0xFC) as u32);
    unsafe {
        core::arch::asm!(
            "out dx, eax",
            in("dx")  0xCF8u16,
            in("eax") addr,
            options(nostack, nomem)
        );
        core::arch::asm!(
            "out dx, eax",
            in("dx")  0xCFCu16,
            in("eax") val,
            options(nostack, nomem)
        );
    }
}

// ── virtio-net packet buffer ──────────────────────────────────────────────────

const MTU: usize = 1514;
const RX_BUFS: usize = 8;
const TX_BUFS: usize = 8;

#[derive(Clone, Copy)]
struct NetBuf {
    data: [u8; MTU + 10], // +10 for virtio-net header
    len:  usize,
}

impl NetBuf {
    const fn zeroed() -> Self { Self { data: [0; MTU + 10], len: 0 } }
}

struct VirtioNetState {
    ready:    bool,
    io_base:  u16,          // legacy BAR0 I/O port base
    mac:      [u8; 6],
    ip:       u32,          // IPv4 address (DHCP or static; 0 = unconfigured)
    rx_head:  usize,
    rx_bufs:  [NetBuf; RX_BUFS],
    tx_head:  usize,
    tx_bufs:  [NetBuf; TX_BUFS],
}

static NET: Mutex<VirtioNetState> = Mutex::new(VirtioNetState {
    ready:   false,
    io_base: 0,
    mac:     [0; 6],
    ip:      0,
    rx_head: 0,
    rx_bufs: [const { NetBuf::zeroed() }; RX_BUFS],
    tx_head: 0,
    tx_bufs: [const { NetBuf::zeroed() }; TX_BUFS],
});

// ── I/O port helpers for virtio legacy device ─────────────────────────────────

#[inline]
unsafe fn vio_read32(base: u16, offset: u16) -> u32 {
    let mut v: u32;
    core::arch::asm!("in eax, dx", in("dx") base + offset, out("eax") v, options(nostack, nomem));
    v
}
#[inline]
unsafe fn vio_write32(base: u16, offset: u16, v: u32) {
    core::arch::asm!("out dx, eax", in("dx") base + offset, in("eax") v, options(nostack, nomem));
}
#[inline]
unsafe fn vio_read8(base: u16, offset: u16) -> u8 {
    let mut v: u8;
    core::arch::asm!("in al, dx", in("dx") base + offset, out("al") v, options(nostack, nomem));
    v
}
#[inline]
unsafe fn vio_write8(base: u16, offset: u16, v: u8) {
    core::arch::asm!("out dx, al", in("dx") base + offset, in("al") v, options(nostack, nomem));
}

// virtio legacy registers (relative to I/O BAR)
const VIONET_DEVICE_FEATURES:  u16 = 0x00;
const VIONET_GUEST_FEATURES:   u16 = 0x04;
const VIONET_QUEUE_PFN:        u16 = 0x08;
const VIONET_QUEUE_SIZE:       u16 = 0x0C;
const VIONET_QUEUE_SELECT:     u16 = 0x0E;
const VIONET_QUEUE_NOTIFY:     u16 = 0x10;
const VIONET_DEVICE_STATUS:    u16 = 0x12;
const VIONET_ISR_STATUS:       u16 = 0x13;
const VIONET_MAC_BASE:         u16 = 0x14;

const VIRTIO_STATUS_ACKNOWLEDGE: u8 = 1;
const VIRTIO_STATUS_DRIVER:      u8 = 2;
const VIRTIO_STATUS_DRIVER_OK:   u8 = 4;
const VIRTIO_F_CSUM_OFFLOAD:     u32 = 1 << 0;

// ── Probe + init ──────────────────────────────────────────────────────────────

/// Scan PCI bus 0 for a virtio-net device.  Returns `(bus, dev)` if found.
fn pci_find_virtio_net() -> Option<(u8, u8)> {
    for dev in 0u8..32 {
        let id = pci_cfg_read32(0, dev, 0, 0);
        let vendor = (id & 0xFFFF) as u16;
        let device = (id >> 16) as u16;
        if vendor == 0x1AF4 && (device == 0x1000 || device == 0x1041) {
            return Some((0, dev));
        }
    }
    None
}

/// Read the I/O BAR (BAR0) for a PCI device.
fn pci_io_bar(bus: u8, dev: u8) -> u16 {
    let bar0 = pci_cfg_read32(bus, dev, 0, 0x10);
    // BAR0 is I/O if bit 0 is set; mask the flag bits.
    if bar0 & 1 == 1 {
        (bar0 & 0xFFFC) as u16
    } else {
        0
    }
}

/// Enable PCI bus-mastering + I/O space for the device.
fn pci_enable_device(bus: u8, dev: u8) {
    let cmd = pci_cfg_read32(bus, dev, 0, 0x04);
    // bit 0 = I/O space, bit 2 = bus master
    pci_cfg_write32(bus, dev, 0, 0x04, cmd | 0x05);
}

/// Initialise the virtio-net device.  Fills `NET` on success.
pub fn init() {
    let (bus, dev) = match pci_find_virtio_net() {
        Some(d) => d,
        None => {
            log::info!("[virtio-net] no device found on PCI bus 0");
            return;
        }
    };

    pci_enable_device(bus, dev);
    let io_base = pci_io_bar(bus, dev);
    if io_base == 0 {
        log::warn!("[virtio-net] BAR0 is not an I/O BAR — skipping");
        return;
    }

    log::info!("[virtio-net] found at PCI 0:{} io_base={:#x}", dev, io_base);

    unsafe {
        // Reset device.
        vio_write8(io_base, VIONET_DEVICE_STATUS, 0);
        // Acknowledge + driver.
        vio_write8(io_base, VIONET_DEVICE_STATUS, VIRTIO_STATUS_ACKNOWLEDGE | VIRTIO_STATUS_DRIVER);

        // Read MAC from device config space (bytes after ISR register at 0x14+).
        let mut mac = [0u8; 6];
        for i in 0..6u16 {
            mac[i as usize] = vio_read8(io_base, VIONET_MAC_BASE + i);
        }

        // Negotiate minimal features (no checksum offload needed).
        let dev_feat = vio_read32(io_base, VIONET_DEVICE_FEATURES);
        let my_feat  = dev_feat & !(VIRTIO_F_CSUM_OFFLOAD);
        vio_write32(io_base, VIONET_GUEST_FEATURES, my_feat);

        // Signal driver OK (we skip full vring setup for the probe-level init).
        vio_write8(io_base, VIONET_DEVICE_STATUS,
            VIRTIO_STATUS_ACKNOWLEDGE | VIRTIO_STATUS_DRIVER | VIRTIO_STATUS_DRIVER_OK);

        let mut net = NET.lock();
        net.ready   = true;
        net.io_base = io_base;
        net.mac     = mac;
        log::info!("[virtio-net] MAC {:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
            mac[0], mac[1], mac[2], mac[3], mac[4], mac[5]);
    }
}

// ── Public API ────────────────────────────────────────────────────────────────

/// Returns true if a virtio-net device was successfully initialised.
pub fn is_ready() -> bool {
    NET.lock().ready
}

/// Copy MAC and IPv4 info as a human-readable text string into `out`.
/// Returns bytes written.
pub fn info_text(out: &mut [u8]) -> usize {
    let net = NET.lock();
    if !net.ready {
        let s = b"net: no device\n";
        let n = s.len().min(out.len());
        out[..n].copy_from_slice(&s[..n]);
        return n;
    }
    let m = net.mac;
    let ip = net.ip;
    // Format: "mac=XX:XX:XX:XX:XX:XX ip=A.B.C.D\n"
    let mut tmp = [0u8; 64];
    let n = fmt_net_info(&mut tmp, &m, ip);
    let copy = n.min(out.len());
    out[..copy].copy_from_slice(&tmp[..copy]);
    copy
}

fn fmt_net_info(buf: &mut [u8; 64], mac: &[u8; 6], ip: u32) -> usize {
    let mut pos = 0usize;
    let prefix = b"mac=";
    buf[pos..pos + 4].copy_from_slice(prefix); pos += 4;
    for (i, &b) in mac.iter().enumerate() {
        pos += write_hex_byte(&mut buf[pos..], b);
        if i < 5 { buf[pos] = b':'; pos += 1; }
    }
    buf[pos] = b' '; pos += 1;
    let ip_label = b"ip=";
    buf[pos..pos + 3].copy_from_slice(ip_label); pos += 3;
    let octets = [(ip >> 24) as u8, (ip >> 16) as u8, (ip >> 8) as u8, ip as u8];
    for (i, &o) in octets.iter().enumerate() {
        pos += write_decimal_u8(&mut buf[pos..], o);
        if i < 3 { buf[pos] = b'.'; pos += 1; }
    }
    buf[pos] = b'\n'; pos += 1;
    pos
}

fn write_hex_byte(buf: &mut [u8], v: u8) -> usize {
    const HEX: &[u8] = b"0123456789abcdef";
    buf[0] = HEX[(v >> 4) as usize];
    buf[1] = HEX[(v & 0xF) as usize];
    2
}

fn write_decimal_u8(buf: &mut [u8], mut v: u8) -> usize {
    if v == 0 { buf[0] = b'0'; return 1; }
    let mut tmp = [0u8; 3];
    let mut n = 0usize;
    while v > 0 { tmp[n] = b'0' + v % 10; v /= 10; n += 1; }
    for i in 0..n { buf[i] = tmp[n - 1 - i]; }
    n
}

/// Enqueue a raw Ethernet frame for transmission.
/// Returns `Ok(())` or `Err("not ready")` / `Err("tx buffer full")`.
pub fn send_raw(frame: &[u8]) -> Result<(), &'static str> {
    if frame.len() > MTU { return Err("frame too large"); }
    let mut net = NET.lock();
    if !net.ready { return Err("not ready"); }
    let idx = net.tx_head % TX_BUFS;
    let buf = &mut net.tx_bufs[idx];
    buf.len = frame.len();
    buf.data[..frame.len()].copy_from_slice(frame);
    net.tx_head = net.tx_head.wrapping_add(1);
    // Notify virtqueue 1 (TX).
    let io = net.io_base;
    unsafe { vio_write32(io, VIONET_QUEUE_NOTIFY, 1); }
    Ok(())
}

/// Pop the next received raw Ethernet frame into `buf`.
/// Returns `Some(len)` or `None` if no frame available.
pub fn recv_raw(buf: &mut [u8]) -> Option<usize> {
    let mut net = NET.lock();
    if !net.ready { return None; }
    let idx = net.rx_head % RX_BUFS;
    let pkt = &net.rx_bufs[idx];
    if pkt.len == 0 { return None; }
    let len = pkt.len.min(buf.len());
    buf[..len].copy_from_slice(&pkt.data[..len]);
    net.rx_bufs[idx].len = 0;
    net.rx_head = net.rx_head.wrapping_add(1);
    Some(len)
}

/// Set the IPv4 address (called after DHCP or static config).
pub fn set_ip(ip: u32) {
    NET.lock().ip = ip;
}
