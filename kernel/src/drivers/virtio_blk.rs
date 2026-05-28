//! Phase 49 — virtio-blk PCI driver (minimal sector read/write path).
//!
//! Probes PCI for a virtio block device (vendor 0x1AF4, device 0x1001),
//! negotiates legacy virtio features, and provides synchronous sector-level
//! read/write via the legacy I/O BAR (BAR0).
//!
//! Sector size is always 512 bytes.  The driver maintains a single-request
//! virtqueue (virtq 0) and uses polling (no MSI/IRQ) for simplicity.

use core::sync::atomic::{AtomicBool, Ordering};
use spin::Mutex;

use crate::arch::{pci, port_io};

// ── virtio legacy I/O register offsets ───────────────────────────────────────
const VIRTIO_DEVICE_FEATURES:      u16 = 0x00;
const VIRTIO_GUEST_FEATURES:       u16 = 0x04;
const VIRTIO_QUEUE_ADDRESS:        u16 = 0x08;
const VIRTIO_QUEUE_SIZE:           u16 = 0x0C;
const VIRTIO_QUEUE_SELECT:         u16 = 0x0E;
const VIRTIO_QUEUE_NOTIFY:         u16 = 0x10;
const VIRTIO_DEVICE_STATUS:        u16 = 0x12;
const VIRTIO_ISR_STATUS:           u16 = 0x13;
const VIRTIO_BLK_CONFIG_CAP_LO:   u16 = 0x14;
const VIRTIO_BLK_CONFIG_CAP_HI:   u16 = 0x18;

const VIRTIO_STATUS_RESET:         u8  = 0x00;
const VIRTIO_STATUS_ACKNOWLEDGE:   u8  = 0x01;
const VIRTIO_STATUS_DRIVER:        u8  = 0x02;
const VIRTIO_STATUS_DRIVER_OK:     u8  = 0x04;
const VIRTIO_STATUS_FEATURES_OK:   u8  = 0x08;

const VIRTIO_BLK_F_RO: u32 = 1 << 5;

const VIRTIO_BLK_T_IN:  u32 = 0;
const VIRTIO_BLK_T_OUT: u32 = 1;

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct VirtqDesc {
    addr:  u64,
    len:   u32,
    flags: u16,
    next:  u16,
}

const VIRTQ_DESC_F_NEXT:  u16 = 1;
const VIRTQ_DESC_F_WRITE: u16 = 2;

#[repr(C)]
struct BlkReqHdr {
    typ:    u32,
    _res:   u32,
    sector: u64,
}

#[repr(C, align(512))]
struct BlkReqBuf {
    hdr:    BlkReqHdr,
    status: u8,
}

/// Legacy virtio split-ring layout (used ring page-aligned per spec).
#[derive(Clone, Copy)]
struct VringLayout {
    qsize:          u16,
    avail_idx_off:  u32,
    avail_ring_off: u32,
    used_idx_off:   u32,
    page_count:     usize,
}

impl VringLayout {
    fn for_qsize(qsize: u16) -> Option<Self> {
        if qsize == 0 || (qsize & (qsize - 1)) != 0 {
            return None;
        }
        let q = qsize as u32;
        let desc_sz = 16 * q;
        let avail_ring_off = desc_sz + 4;
        let after_avail = avail_ring_off + 2 * q;
        let used_base = (after_avail + 4095) & !4095;
        let used_idx_off = used_base + 2;
        let used_ring_end = used_idx_off + 2 + 8 * q;
        let page_count = ((used_ring_end as usize) + 4095) / 4096;
        Some(Self {
            qsize,
            avail_idx_off: desc_sz + 2,
            avail_ring_off,
            used_idx_off,
            page_count,
        })
    }
}

struct BlkDma {
    vring_phys:  u64,
    layout:      VringLayout,
    req_phys:    u64,
    sector_phys: u64,
}

struct BlkState {
    ready:       bool,
    io_base:     u16,
    capacity:    u64,
    read_only:   bool,
    queue_size:  u16,
    avail_idx:   u16,
    used_idx:    u16,
    dma:         Option<BlkDma>,
}

static BLK: Mutex<BlkState> = Mutex::new(BlkState {
    ready:       false,
    io_base:     0,
    capacity:    0,
    read_only:   false,
    queue_size:  0,
    avail_idx:   0,
    used_idx:    0,
    dma:         None,
});

static BLK_READY: AtomicBool = AtomicBool::new(false);

fn hhdm() -> u64 {
    crate::mm::frame_allocator::hhdm_offset()
}

fn phys_to_virt(phys: u64) -> u64 {
    phys + hhdm()
}

unsafe fn req_mut(dma: &BlkDma) -> &mut BlkReqBuf {
    &mut *(phys_to_virt(dma.req_phys) as *mut BlkReqBuf)
}

unsafe fn sector_mut(dma: &BlkDma) -> &mut [u8; 512] {
    &mut *(phys_to_virt(dma.sector_phys) as *mut [u8; 512])
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

fn alloc_dma(qsize: u16) -> Option<BlkDma> {
    let layout = VringLayout::for_qsize(qsize)?;
    let vring_phys = crate::mm::frame_allocator::alloc_contiguous_frames(layout.page_count)?;
    let req_phys = crate::mm::frame_allocator::alloc_frame()?;
    let sector_phys = crate::mm::frame_allocator::alloc_frame()?;
    unsafe {
        core::ptr::write_bytes(
            phys_to_virt(vring_phys) as *mut u8,
            0,
            layout.page_count * 4096,
        );
        core::ptr::write_bytes(phys_to_virt(req_phys) as *mut u8, 0, 4096);
        core::ptr::write_bytes(phys_to_virt(sector_phys) as *mut u8, 0, 4096);
    }
    Some(BlkDma { vring_phys, layout, req_phys, sector_phys })
}

// ── PCI probe ─────────────────────────────────────────────────────────────────

pub fn init() {
    if !pci::LEGACY_IO_AVAILABLE {
        log::info!("[virtio-blk] skipped — no legacy PCI on this arch");
        return;
    }
    let Some((bus, dev)) = pci::find_virtio_legacy(0, 0x1AF4, 0x1001) else {
        log::info!("[virtio-blk] no device on PCI bus 0");
        return;
    };

    let io_base = pci::bar0_io_base(bus, dev, 0);
    if io_base == 0 {
        log::warn!("[virtio-blk] BAR0 is MMIO — only I/O BARs supported");
        return;
    }

    pci::enable_io_and_busmaster(bus, dev, 0);
    log::info!("[virtio-blk] found PCI {:02x}.{} I/O base={:#x}", bus, dev, io_base);

    if setup_device(io_base) {
        return;
    }
}

fn setup_device(io_base: u16) -> bool {
    unsafe {
        port_io::outb(io_base + VIRTIO_DEVICE_STATUS, VIRTIO_STATUS_RESET);
        port_io::outb(io_base + VIRTIO_DEVICE_STATUS, VIRTIO_STATUS_ACKNOWLEDGE);
        port_io::outb(io_base + VIRTIO_DEVICE_STATUS, VIRTIO_STATUS_ACKNOWLEDGE | VIRTIO_STATUS_DRIVER);

        let features = port_io::inl(io_base + VIRTIO_DEVICE_FEATURES);
        let read_only = (features & VIRTIO_BLK_F_RO) != 0;
        port_io::outl(io_base + VIRTIO_GUEST_FEATURES, 0);

        port_io::outb(io_base + VIRTIO_DEVICE_STATUS,
            VIRTIO_STATUS_ACKNOWLEDGE | VIRTIO_STATUS_DRIVER | VIRTIO_STATUS_FEATURES_OK);

        let cap_lo = port_io::inl(io_base + VIRTIO_BLK_CONFIG_CAP_LO) as u64;
        let cap_hi = port_io::inl(io_base + VIRTIO_BLK_CONFIG_CAP_HI) as u64;
        let capacity = (cap_hi << 32) | cap_lo;

        port_io::outw(io_base + VIRTIO_QUEUE_SELECT, 0);
        let qsize = port_io::inw(io_base + VIRTIO_QUEUE_SIZE);
        if VringLayout::for_qsize(qsize).is_none() {
            log::warn!("[virtio-blk] invalid queue size {}", qsize);
            return false;
        }

        let dma = match alloc_dma(qsize) {
            Some(d) => d,
            None => {
                log::warn!("[virtio-blk] DMA allocation failed (qsize={})", qsize);
                return false;
            }
        };

        port_io::outl(io_base + VIRTIO_QUEUE_ADDRESS, (dma.vring_phys >> 12) as u32);

        port_io::outb(io_base + VIRTIO_DEVICE_STATUS,
            VIRTIO_STATUS_ACKNOWLEDGE | VIRTIO_STATUS_DRIVER | VIRTIO_STATUS_FEATURES_OK | VIRTIO_STATUS_DRIVER_OK);

        let vring_phys = dma.vring_phys;
        let page_count = dma.layout.page_count;

        let mut blk = BLK.lock();
        blk.ready = true;
        blk.io_base = io_base;
        blk.capacity = capacity;
        blk.read_only = read_only;
        blk.queue_size = qsize;
        blk.avail_idx = 0;
        blk.used_idx = 0;
        blk.dma = Some(dma);
        BLK_READY.store(true, Ordering::Release);

        if read_only {
            log::warn!("[virtio-blk] device is read-only — bundle disk writes disabled");
        }
        log::info!(
            "[virtio-blk] ready — {} sectors ({} MiB) qsize={} vring_phys={:#x} pages={}",
            capacity,
            capacity / 2048,
            qsize,
            vring_phys,
            page_count
        );
    }

    match self_test() {
        Ok(()) => log::info!("[virtio-blk] self-test read/write OK"),
        Err(e) => log::warn!("[virtio-blk] self-test failed: {}", e),
    }
    true
}

fn self_test() -> Result<(), &'static str> {
    let mut pat = [0u8; 512];
    pat[..8].copy_from_slice(b"OSCTEST1");
    write_sectors(1, 1, &pat)?;
    let mut readback = [0u8; 512];
    read_sectors(1, 1, &mut readback)?;
    if readback[..8] != pat[..8] {
        return Err("readback mismatch");
    }
    Ok(())
}

// ── Public API ────────────────────────────────────────────────────────────────

pub fn is_ready() -> bool { BLK_READY.load(Ordering::Acquire) }

pub fn capacity_sectors() -> u64 {
    BLK.lock().capacity
}

pub fn read_sectors(sector: u64, count: u64, buf: &mut [u8]) -> Result<(), &'static str> {
    if !is_ready() { return Err("device not ready"); }
    if buf.len() < (count * 512) as usize { return Err("buffer too small"); }
    let mut blk = BLK.lock();
    for i in 0..count {
        do_sector_op(&mut blk, VIRTIO_BLK_T_IN, sector + i)?;
        let dma = blk.dma.as_ref().ok_or("no dma")?;
        let src = unsafe { sector_mut(dma) };
        let dst = &mut buf[(i as usize * 512)..(i as usize * 512 + 512)];
        dst.copy_from_slice(src);
    }
    Ok(())
}

pub fn write_sectors(sector: u64, count: u64, data: &[u8]) -> Result<(), &'static str> {
    if !is_ready() { return Err("device not ready"); }
    if blk_is_readonly() { return Err("read-only device"); }
    if data.len() < (count * 512) as usize { return Err("data too small"); }
    let mut blk = BLK.lock();
    for i in 0..count {
        let src = &data[(i as usize * 512)..(i as usize * 512 + 512)];
        let dma = blk.dma.as_ref().ok_or("no dma")?;
        unsafe { sector_mut(dma).copy_from_slice(src) };
        do_sector_op(&mut blk, VIRTIO_BLK_T_OUT, sector + i)?;
    }
    Ok(())
}

fn blk_is_readonly() -> bool {
    BLK.lock().read_only
}

pub fn info_text(out: &mut [u8]) -> usize {
    if !is_ready() {
        let msg = b"virtio-blk: not found\n";
        let n = msg.len().min(out.len());
        out[..n].copy_from_slice(&msg[..n]);
        return n;
    }
    let blk = BLK.lock();
    let cap = blk.capacity;
    let mib = cap / 2048;
    drop(blk);

    let mut pos = 0usize;
    let mut write = |s: &[u8]| {
        let n = s.len().min(out.len().saturating_sub(pos));
        if n > 0 { out[pos..pos+n].copy_from_slice(&s[..n]); pos += n; }
    };

    write(b"virtio-blk ready  sectors=");
    write(u64_to_dec(cap).as_bytes());
    write(b"  size=");
    write(u64_to_dec(mib).as_bytes());
    write(b" MiB\n");
    pos
}

fn do_sector_op(blk: &mut BlkState, typ: u32, sector: u64) -> Result<(), &'static str> {
    let dma = blk.dma.as_ref().ok_or("no dma")?;
    let layout = dma.layout;
    let vring_virt = phys_to_virt(dma.vring_phys);

    unsafe {
        let req = req_mut(dma);
        req.hdr.typ = typ;
        req.hdr._res = 0;
        req.hdr.sector = sector;
        req.status = 0xFF;

        let hdr_phys = dma.req_phys;
        let buf_phys = dma.sector_phys;
        let status_phys = dma.req_phys + core::mem::size_of::<BlkReqHdr>() as u64;

        vring_write_desc(vring_virt, 0, VirtqDesc {
            addr: hdr_phys,
            len: core::mem::size_of::<BlkReqHdr>() as u32,
            flags: VIRTQ_DESC_F_NEXT,
            next: 1,
        });
        let data_flags = if typ == VIRTIO_BLK_T_IN {
            VIRTQ_DESC_F_NEXT | VIRTQ_DESC_F_WRITE
        } else {
            VIRTQ_DESC_F_NEXT
        };
        vring_write_desc(vring_virt, 1, VirtqDesc {
            addr: buf_phys,
            len: 512,
            flags: data_flags,
            next: 2,
        });
        vring_write_desc(vring_virt, 2, VirtqDesc {
            addr: status_phys,
            len: 1,
            flags: VIRTQ_DESC_F_WRITE,
            next: 0,
        });

        let qmask = (blk.queue_size - 1) as u32;
        let ai = (blk.avail_idx as u32) & qmask;
        vring_write_u16(vring_virt, layout.avail_ring_off + ai * 2, 0);
        crate::arch::memory_fence();
        blk.avail_idx = blk.avail_idx.wrapping_add(1);
        vring_write_u16(vring_virt, layout.avail_idx_off, blk.avail_idx);
        crate::arch::memory_fence();

        port_io::outw(blk.io_base + VIRTIO_QUEUE_NOTIFY, 0);

        let mut spins = 0usize;
        loop {
            crate::arch::memory_fence();
            let _ = port_io::inb(blk.io_base + VIRTIO_ISR_STATUS);
            let used_idx = vring_read_u16(vring_virt, layout.used_idx_off);
            if used_idx != blk.used_idx {
                blk.used_idx = used_idx;
                break;
            }
            spins += 1;
            if spins > 5_000_000 {
                return Err("virtio-blk: timeout");
            }
            crate::arch::spin_pause();
        }

        if req.status != 0 {
            return Err("virtio-blk: I/O error");
        }
    }
    Ok(())
}

fn u64_to_dec(mut n: u64) -> &'static str {
    static mut BUF: [u8; 20] = [0u8; 20];
    let buf = unsafe { &mut BUF };
    if n == 0 {
        buf[0] = b'0';
        return core::str::from_utf8(&buf[..1]).unwrap_or("0");
    }
    let mut i = 19usize;
    while n > 0 {
        buf[i] = b'0' + (n % 10) as u8;
        n /= 10;
        if i == 0 { break; }
        i -= 1;
    }
    let start = if n > 0 { 0 } else { i + 1 };
    core::str::from_utf8(&buf[start..20]).unwrap_or("?")
}
