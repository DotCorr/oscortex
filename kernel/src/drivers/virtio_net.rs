//! Phase 45 — virtio-net PCI driver (legacy I/O BAR, split virtqueues).
//!
//! Queue 0 = RX, queue 1 = TX. Polling driver (no MSI) matching the virtio-blk
//! bring-up style. Used by the kernel UDP/TCP stack via `send_raw` / `recv_raw`.

use core::sync::atomic::{AtomicBool, Ordering};
use spin::Mutex;

use crate::arch::{pci, port_io};
use crate::drivers::common::virtio_net_frame::{
    self, validate_tx_frame, VIRTIO_NET_HDR_SIZE, RX_BUF_LEN, MTU,
};
use crate::drivers::common::vring::{VirtqDesc, VringLayout, VIRTQ_DESC_F_WRITE};

// Posted RX descriptors. 8 slots (~12 KB of frames in flight) overflowed the
// moment a TCP sender bursts more than 8 frames between guest polls — slirp
// then drops and falls into retransmit-timeout pacing (~28 KB/s observed on a
// 5.4 MB package fetch, regardless of the TCP window). 64 slots absorbs the
// bursts; the virtqueue is 256 entries and each slot is one 4 KiB frame.
const RX_SLOTS: usize = 64;

const VIRTIO_DEVICE_FEATURES: u16 = 0x00;
const VIRTIO_GUEST_FEATURES: u16 = 0x04;
const VIRTIO_QUEUE_ADDRESS: u16 = 0x08;
const VIRTIO_QUEUE_SIZE: u16 = 0x0C;
const VIRTIO_QUEUE_SELECT: u16 = 0x0E;
const VIRTIO_QUEUE_NOTIFY: u16 = 0x10;
const VIRTIO_DEVICE_STATUS: u16 = 0x12;
const VIRTIO_ISR_STATUS: u16 = 0x13;
const VIRTIO_NET_MAC_BASE: u16 = 0x14;

const VIRTIO_STATUS_RESET: u8 = 0x00;
const VIRTIO_STATUS_ACKNOWLEDGE: u8 = 0x01;
const VIRTIO_STATUS_DRIVER: u8 = 0x02;
const VIRTIO_STATUS_FEATURES_OK: u8 = 0x08;
const VIRTIO_STATUS_DRIVER_OK: u8 = 0x04;

const RX_QUEUE: u16 = 0;
const TX_QUEUE: u16 = 1;

struct NetQueue {
    vring_phys: u64,
    layout: VringLayout,
    avail_idx: u16,
    used_idx: u16,
}

struct VirtioNetState {
    ready: bool,
    io_base: u16,
    mac: [u8; 6],
    ip: u32,
    rx: Option<NetQueue>,
    tx: Option<NetQueue>,
    rx_buf_phys: [u64; RX_SLOTS],
    tx_buf_phys: u64,
    rx_slots: u16,
}

static NET: Mutex<VirtioNetState> = Mutex::new(VirtioNetState {
    ready: false,
    io_base: 0,
    mac: [0; 6],
    ip: 0,
    rx: None,
    tx: None,
    rx_buf_phys: [0; RX_SLOTS],
    tx_buf_phys: 0,
    rx_slots: 0,
});

static NET_READY: AtomicBool = AtomicBool::new(false);

fn hhdm() -> u64 {
    crate::mm::frame_allocator::hhdm_offset()
}

fn phys_to_virt(phys: u64) -> u64 {
    phys + hhdm()
}

unsafe fn vring_write_desc(vring_virt: u64, idx: u16, desc: VirtqDesc) {
    let off = (idx as u64) * 16;
    core::ptr::write((vring_virt + off) as *mut VirtqDesc, desc);
}

unsafe fn vring_read_u16(vring_virt: u64, off: u32) -> u16 {
    core::ptr::read_volatile((vring_virt + off as u64) as *const u16)
}

unsafe fn vring_write_u16(vring_virt: u64, off: u32, val: u16) {
    core::ptr::write_volatile((vring_virt + off as u64) as *mut u16, val);
}

unsafe fn vring_read_u32(vring_virt: u64, off: u32) -> u32 {
    core::ptr::read_volatile((vring_virt + off as u64) as *const u32)
}

fn alloc_vring(qsize: u16) -> Option<(u64, VringLayout)> {
    let layout = VringLayout::for_qsize(qsize)?;
    let vring_phys = crate::mm::frame_allocator::alloc_contiguous_frames(layout.page_count)?;
    unsafe {
        core::ptr::write_bytes(
            phys_to_virt(vring_phys) as *mut u8,
            0,
            layout.page_count * 4096,
        );
    }
    Some((vring_phys, layout))
}

unsafe fn attach_queue(io_base: u16, queue_idx: u16, vring_phys: u64) -> Option<u16> {
    port_io::outw(io_base + VIRTIO_QUEUE_SELECT, queue_idx);
    let qsize = port_io::inw(io_base + VIRTIO_QUEUE_SIZE);
    if VringLayout::for_qsize(qsize).is_none() {
        return None;
    }
    port_io::outl(io_base + VIRTIO_QUEUE_ADDRESS, (vring_phys >> 12) as u32);
    Some(qsize)
}

unsafe fn post_rx_buffers(
    io_base: u16,
    rx: &mut NetQueue,
    rx_buf_phys: &[u64],
    slots: u16,
) -> Result<(), &'static str> {
    let layout = rx.layout;
    let vring_virt = phys_to_virt(rx.vring_phys);
    let qmask = (layout.qsize - 1) as u32;

    for i in 0..slots as u16 {
        let buf_phys = rx_buf_phys[i as usize];
        vring_write_desc(vring_virt, i, VirtqDesc {
            addr: buf_phys,
            len: RX_BUF_LEN,
            flags: VIRTQ_DESC_F_WRITE,
            next: 0,
        });

        let ai = (rx.avail_idx as u32) & qmask;
        vring_write_u16(vring_virt, layout.avail_ring_off + ai * 2, i);
        crate::arch::memory_fence();
        rx.avail_idx = rx.avail_idx.wrapping_add(1);
        vring_write_u16(vring_virt, layout.avail_idx_off, rx.avail_idx);
        crate::arch::memory_fence();
    }

    let _ = port_io::inb(io_base + VIRTIO_ISR_STATUS);
    Ok(())
}

unsafe fn repost_rx_descriptor(rx: &mut NetQueue, desc_id: u16) {
    let layout = rx.layout;
    let vring_virt = phys_to_virt(rx.vring_phys);
    let qmask = (layout.qsize - 1) as u32;
    let ai = (rx.avail_idx as u32) & qmask;
    vring_write_u16(vring_virt, layout.avail_ring_off + ai * 2, desc_id);
    crate::arch::memory_fence();
    rx.avail_idx = rx.avail_idx.wrapping_add(1);
    vring_write_u16(vring_virt, layout.avail_idx_off, rx.avail_idx);
    crate::arch::memory_fence();
}

unsafe fn poll_queue_used(
    io_base: u16,
    queue: &mut NetQueue,
    _queue_idx: u16,
    timeout: usize,
) -> Option<(u32, u32)> {
    let layout = queue.layout;
    let vring_virt = phys_to_virt(queue.vring_phys);

    for spins in 0..timeout {
        crate::arch::memory_fence();
        let _ = port_io::inb(io_base + VIRTIO_ISR_STATUS);
        let used_idx = vring_read_u16(vring_virt, layout.used_idx_off);
        if used_idx != queue.used_idx {
            let elem_off = layout.used_elem_offset(queue.used_idx);
            let desc_id = vring_read_u32(vring_virt, elem_off);
            let total_len = vring_read_u32(vring_virt, elem_off + 4);
            queue.used_idx = queue.used_idx.wrapping_add(1);
            return Some((desc_id, total_len));
        }
        if spins + 1 < timeout {
            crate::arch::spin_pause();
        }
    }
    None
}

pub fn init() {
    if !pci::LEGACY_IO_AVAILABLE {
        log::info!("[virtio-net] skipped — no legacy PCI on this arch");
        return;
    }

    let Some((bus, dev)) = pci::find_virtio_legacy(0, 0x1AF4, 0x1000)
        .or_else(|| pci::find_virtio_legacy(0, 0x1AF4, 0x1041))
    else {
        log::info!("[virtio-net] no device found on PCI bus 0");
        return;
    };

    pci::enable_io_and_busmaster(bus, dev, 0);
    let io_base = pci::bar0_io_base(bus, dev, 0);
    if io_base == 0 {
        log::warn!("[virtio-net] BAR0 is not an I/O BAR — skipping");
        return;
    }

    log::info!("[virtio-net] found at PCI 0:{} io_base={:#x}", dev, io_base);

    if setup_device(io_base) {
        return;
    }
}

fn setup_device(io_base: u16) -> bool {
    unsafe {
        port_io::outb(io_base + VIRTIO_DEVICE_STATUS, VIRTIO_STATUS_RESET);
        port_io::outb(io_base + VIRTIO_DEVICE_STATUS, VIRTIO_STATUS_ACKNOWLEDGE);
        port_io::outb(
            io_base + VIRTIO_DEVICE_STATUS,
            VIRTIO_STATUS_ACKNOWLEDGE | VIRTIO_STATUS_DRIVER,
        );

        let mut mac = [0u8; 6];
        for i in 0..6u16 {
            mac[i as usize] = port_io::inb(io_base + VIRTIO_NET_MAC_BASE + i);
        }

        let _dev_feat = port_io::inl(io_base + VIRTIO_DEVICE_FEATURES);
        port_io::outl(io_base + VIRTIO_GUEST_FEATURES, 0);
        port_io::outb(
            io_base + VIRTIO_DEVICE_STATUS,
            VIRTIO_STATUS_ACKNOWLEDGE | VIRTIO_STATUS_DRIVER | VIRTIO_STATUS_FEATURES_OK,
        );

        port_io::outw(io_base + VIRTIO_QUEUE_SELECT, RX_QUEUE);
        let rx_qsize = port_io::inw(io_base + VIRTIO_QUEUE_SIZE);
        let (rx_vring_phys, rx_layout) = match alloc_vring(rx_qsize) {
            Some(v) => v,
            None => {
                log::warn!("[virtio-net] RX vring allocation failed (qsize={})", rx_qsize);
                return false;
            }
        };
        if attach_queue(io_base, RX_QUEUE, rx_vring_phys).is_none() {
            log::warn!("[virtio-net] invalid RX queue size {}", rx_qsize);
            return false;
        }

        port_io::outw(io_base + VIRTIO_QUEUE_SELECT, TX_QUEUE);
        let tx_qsize = port_io::inw(io_base + VIRTIO_QUEUE_SIZE);
        let (tx_vring_phys, tx_layout) = match alloc_vring(tx_qsize) {
            Some(v) => v,
            None => {
                log::warn!("[virtio-net] TX vring allocation failed (qsize={})", tx_qsize);
                return false;
            }
        };
        if attach_queue(io_base, TX_QUEUE, tx_vring_phys).is_none() {
            log::warn!("[virtio-net] invalid TX queue size {}", tx_qsize);
            return false;
        }

        let mut rx_buf_phys = [0u64; RX_SLOTS];
        let rx_slots = RX_SLOTS.min(rx_layout.qsize as usize) as u16;
        for i in 0..rx_slots as usize {
            let phys = match crate::mm::frame_allocator::alloc_frame() {
                Some(p) => p,
                None => {
                    log::warn!("[virtio-net] RX buffer allocation failed");
                    return false;
                }
            };
            core::ptr::write_bytes(phys_to_virt(phys) as *mut u8, 0, 4096);
            rx_buf_phys[i] = phys;
        }

        let tx_buf_phys = match crate::mm::frame_allocator::alloc_frame() {
            Some(p) => p,
            None => {
                log::warn!("[virtio-net] TX buffer allocation failed");
                return false;
            }
        };
        core::ptr::write_bytes(phys_to_virt(tx_buf_phys) as *mut u8, 0, 4096);

        let mut rx = NetQueue {
            vring_phys: rx_vring_phys,
            layout: rx_layout,
            avail_idx: 0,
            used_idx: 0,
        };
        let mut tx = NetQueue {
            vring_phys: tx_vring_phys,
            layout: tx_layout,
            avail_idx: 0,
            used_idx: 0,
        };

        if post_rx_buffers(io_base, &mut rx, &rx_buf_phys, rx_slots).is_err() {
            return false;
        }

        port_io::outb(
            io_base + VIRTIO_DEVICE_STATUS,
            VIRTIO_STATUS_ACKNOWLEDGE
                | VIRTIO_STATUS_DRIVER
                | VIRTIO_STATUS_FEATURES_OK
                | VIRTIO_STATUS_DRIVER_OK,
        );

        let mut net = NET.lock();
        net.ready = true;
        net.io_base = io_base;
        net.mac = mac;
        net.rx = Some(rx);
        net.tx = Some(tx);
        net.rx_buf_phys = rx_buf_phys;
        net.tx_buf_phys = tx_buf_phys;
        net.rx_slots = rx_slots;
        NET_READY.store(true, Ordering::Release);

        log::info!(
            "[virtio-net] ready — MAC {:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x} rx_q={} tx_q={} slots={}",
            mac[0],
            mac[1],
            mac[2],
            mac[3],
            mac[4],
            mac[5],
            rx_layout.qsize,
            tx_layout.qsize,
            rx_slots
        );
    }

    match loopback_self_test() {
        Ok(()) => log::info!("[virtio-net] self-test TX completion OK"),
        Err(e) => log::warn!("[virtio-net] self-test skipped/failed: {}", e),
    }
    true
}

/// Send a minimal frame and wait for TX completion (validates virtqueue path).
fn loopback_self_test() -> Result<(), &'static str> {
    if !is_ready() {
        return Err("not ready");
    }
    // Dummy Ethernet frame (broadcast ARP-ish size) — QEMU may drop it; we only
    // need the device to consume the TX descriptor.
    let mut frame = [0u8; 42];
    frame[0..6].copy_from_slice(&[0xFF; 6]);
    frame[6..12].copy_from_slice(&NET.lock().mac);
    frame[12] = 0x08;
    frame[13] = 0x06;
    send_raw(&frame)?;
    Ok(())
}

pub fn is_ready() -> bool {
    NET_READY.load(Ordering::Acquire)
}

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
    let mut tmp = [0u8; 64];
    let n = fmt_net_info(&mut tmp, &m, ip);
    let copy = n.min(out.len());
    out[..copy].copy_from_slice(&tmp[..copy]);
    copy
}

fn fmt_net_info(buf: &mut [u8; 64], mac: &[u8; 6], ip: u32) -> usize {
    let mut pos = 0usize;
    let prefix = b"mac=";
    buf[pos..pos + 4].copy_from_slice(prefix);
    pos += 4;
    for (i, &b) in mac.iter().enumerate() {
        pos += write_hex_byte(&mut buf[pos..], b);
        if i < 5 {
            buf[pos] = b':';
            pos += 1;
        }
    }
    buf[pos] = b' ';
    pos += 1;
    let ip_label = b"ip=";
    buf[pos..pos + 3].copy_from_slice(ip_label);
    pos += 3;
    let octets = [(ip >> 24) as u8, (ip >> 16) as u8, (ip >> 8) as u8, ip as u8];
    for (i, &o) in octets.iter().enumerate() {
        pos += write_decimal_u8(&mut buf[pos..], o);
        if i < 3 {
            buf[pos] = b'.';
            pos += 1;
        }
    }
    buf[pos] = b'\n';
    pos += 1;
    pos
}

fn write_hex_byte(buf: &mut [u8], v: u8) -> usize {
    const HEX: &[u8] = b"0123456789abcdef";
    buf[0] = HEX[(v >> 4) as usize];
    buf[1] = HEX[(v & 0xF) as usize];
    2
}

fn write_decimal_u8(buf: &mut [u8], mut v: u8) -> usize {
    if v == 0 {
        buf[0] = b'0';
        return 1;
    }
    let mut tmp = [0u8; 3];
    let mut n = 0usize;
    while v > 0 {
        tmp[n] = b'0' + v % 10;
        v /= 10;
        n += 1;
    }
    for i in 0..n {
        buf[i] = tmp[n - 1 - i];
    }
    n
}

pub fn send_raw(frame: &[u8]) -> Result<(), &'static str> {
    validate_tx_frame(frame.len())?;

    let mut net = NET.lock();
    if !net.ready {
        return Err("not ready");
    }

    let io_base = net.io_base;
    let tx_buf_phys = net.tx_buf_phys;
    let total_len = VIRTIO_NET_HDR_SIZE + frame.len();

    unsafe {
        let tx_buf = phys_to_virt(tx_buf_phys) as *mut u8;
        core::ptr::write_bytes(tx_buf, 0, VIRTIO_NET_HDR_SIZE);
        core::ptr::copy_nonoverlapping(frame.as_ptr(), tx_buf.add(VIRTIO_NET_HDR_SIZE), frame.len());

        let tx = net.tx.as_mut().ok_or("no tx queue")?;
        let layout = tx.layout;
        let vring_virt = phys_to_virt(tx.vring_phys);
        let qmask = (layout.qsize - 1) as u32;

        vring_write_desc(vring_virt, 0, VirtqDesc {
            addr: tx_buf_phys,
            len: total_len as u32,
            flags: 0,
            next: 0,
        });

        let ai = (tx.avail_idx as u32) & qmask;
        vring_write_u16(vring_virt, layout.avail_ring_off + ai * 2, 0);
        crate::arch::memory_fence();
        tx.avail_idx = tx.avail_idx.wrapping_add(1);
        vring_write_u16(vring_virt, layout.avail_idx_off, tx.avail_idx);
        crate::arch::memory_fence();

        port_io::outw(io_base + VIRTIO_QUEUE_NOTIFY, TX_QUEUE);

        if poll_queue_used(io_base, tx, TX_QUEUE, 5_000_000).is_none() {
            return Err("tx timeout");
        }
    }

    Ok(())
}

pub fn recv_raw(buf: &mut [u8]) -> Option<usize> {
    let mut net = NET.lock();
    if !net.ready {
        return None;
    }

    let io_base = net.io_base;
    let rx_buf_phys = net.rx_buf_phys;
    let rx = net.rx.as_mut()?;

    unsafe {
        let (desc_id, total_len) = poll_queue_used(io_base, rx, RX_QUEUE, 1)?;
        let frame_len = match virtio_net_frame::frame_len_from_rx_total(total_len) {
            Some(n) => n,
            None => {
                repost_rx_descriptor(rx, desc_id as u16);
                return None;
            }
        };

        let slot = desc_id as usize;
        if slot >= RX_SLOTS {
            repost_rx_descriptor(rx, desc_id as u16);
            return None;
        }

        let pkt_ptr = phys_to_virt(rx_buf_phys[slot]) as *const u8;
        if frame_len > buf.len() {
            repost_rx_descriptor(rx, desc_id as u16);
            return None;
        }

        core::ptr::copy_nonoverlapping(
            pkt_ptr.add(VIRTIO_NET_HDR_SIZE),
            buf.as_mut_ptr(),
            frame_len,
        );

        repost_rx_descriptor(rx, desc_id as u16);
        Some(frame_len)
    }
}

pub fn set_ip(ip: u32) {
    NET.lock().ip = ip;
}
