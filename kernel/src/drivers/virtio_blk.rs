//! Phase 49 — virtio-blk PCI driver (minimal sector read/write path).
//!
//! Probes PCI for a virtio block device (vendor 0x1AF4, device 0x1001),
//! negotiates legacy virtio features, and provides synchronous sector-level
//! read/write via the legacy I/O BAR (BAR0).
//!
//! Sector size is always 512 bytes.  The driver maintains a single-request
//! virtqueue (virtq 0) and uses polling (no MSI/IRQ) for simplicity.
//!
//! ## Syscall surface
//!   SYS_BLK_INFO  (0x385) → capacity_sectors: u64
//!   SYS_BLK_READ  (0x386, sector, count, buf_ptr, buf_len)
//!   SYS_BLK_WRITE (0x387, sector, count, buf_ptr, buf_len)

use core::sync::atomic::{AtomicBool, Ordering};
use spin::Mutex;

// ── PCI helpers (same as virtio_net) ─────────────────────────────────────────

#[inline]
fn pci_cfg_read32(bus: u8, dev: u8, func: u8, offset: u8) -> u32 {
    let addr: u32 = (1 << 31)
        | ((bus  as u32) << 16)
        | ((dev  as u32) << 11)
        | ((func as u32) <<  8)
        | ((offset & 0xFC) as u32);
    unsafe {
        core::arch::asm!("out dx, eax", in("dx") 0xCF8u16, in("eax") addr, options(nostack, nomem));
        let v: u32;
        core::arch::asm!("in eax, dx",  in("dx") 0xCFCu16, out("eax") v,   options(nostack, nomem));
        v
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
        core::arch::asm!("out dx, eax", in("dx") 0xCF8u16, in("eax") addr, options(nostack, nomem));
        core::arch::asm!("out dx, eax", in("dx") 0xCFCu16, in("eax") val,  options(nostack, nomem));
    }
}

#[inline]
unsafe fn inb(port: u16) -> u8 {
    let v: u8;
    core::arch::asm!("in al, dx", out("al") v, in("dx") port, options(nomem, nostack));
    v
}
#[inline]
unsafe fn inw(port: u16) -> u16 {
    let v: u16;
    core::arch::asm!("in ax, dx", out("ax") v, in("dx") port, options(nomem, nostack));
    v
}
#[inline]
unsafe fn inl(port: u16) -> u32 {
    let v: u32;
    core::arch::asm!("in eax, dx", out("eax") v, in("dx") port, options(nomem, nostack));
    v
}
#[inline]
unsafe fn outb(port: u16, val: u8) {
    core::arch::asm!("out dx, al", in("dx") port, in("al") val, options(nomem, nostack));
}
#[inline]
unsafe fn outl(port: u16, val: u32) {
    core::arch::asm!("out dx, eax", in("dx") port, in("eax") val, options(nomem, nostack));
}

// ── virtio legacy I/O register offsets ───────────────────────────────────────
// All relative to I/O BAR0 base.
const VIRTIO_DEVICE_FEATURES:      u16 = 0x00; // R  (32)
const VIRTIO_GUEST_FEATURES:       u16 = 0x04; // W  (32)
const VIRTIO_QUEUE_ADDRESS:        u16 = 0x08; // W  (32): page number (shifted by 12)
const VIRTIO_QUEUE_SIZE:           u16 = 0x0C; // R  (16)
const VIRTIO_QUEUE_SELECT:         u16 = 0x0E; // W  (16)
const VIRTIO_QUEUE_NOTIFY:         u16 = 0x10; // W  (16)
const VIRTIO_DEVICE_STATUS:        u16 = 0x12; // RW (8)
const VIRTIO_ISR_STATUS:           u16 = 0x13; // R  (8)
// Device-specific config starts at 0x14 for legacy block
const VIRTIO_BLK_CONFIG_CAP_LO:   u16 = 0x14; // R  (32) lower capacity
const VIRTIO_BLK_CONFIG_CAP_HI:   u16 = 0x18; // R  (32) upper capacity

// virtio status bits
const VIRTIO_STATUS_RESET:         u8  = 0x00;
const VIRTIO_STATUS_ACKNOWLEDGE:   u8  = 0x01;
const VIRTIO_STATUS_DRIVER:        u8  = 0x02;
const VIRTIO_STATUS_DRIVER_OK:     u8  = 0x04;
const VIRTIO_STATUS_FEATURES_OK:   u8  = 0x08;
const VIRTIO_STATUS_FAILED:        u8  = 0x80;

// virtio-blk feature bits
const VIRTIO_BLK_F_RO: u32 = 1 << 5; // read-only device

// virtio-blk request types
const VIRTIO_BLK_T_IN:  u32 = 0; // read
const VIRTIO_BLK_T_OUT: u32 = 1; // write

// virtq constants for the single-descriptor ring
const VRING_SIZE: usize = 16; // must be power of two

// ── virtio virtqueue ring structures ─────────────────────────────────────────

/// virtq descriptor entry (16 bytes).
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

/// virtq available ring header.
#[repr(C)]
struct VirtqAvail {
    flags: u16,
    idx:   u16,
    ring:  [u16; VRING_SIZE],
}

/// virtq used element.
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct VirtqUsedElem {
    id:  u32,
    len: u32,
}

/// virtq used ring header.
#[repr(C)]
struct VirtqUsed {
    flags: u16,
    idx:   u16,
    ring:  [VirtqUsedElem; VRING_SIZE],
}

// ── virtio-blk request header ─────────────────────────────────────────────────
#[repr(C)]
struct BlkReqHdr {
    typ:    u32,
    _res:   u32,
    sector: u64,
}

// ── Per-request DMA buffers (static; single-threaded polling) ─────────────────
// We allocate everything statically so we never need a DMA allocator.

#[repr(C, align(4096))]
struct BlkVring {
    desc:  [VirtqDesc;    VRING_SIZE],
    avail: VirtqAvail,
    _pad:  [u8; 4096 - core::mem::size_of::<[VirtqDesc; VRING_SIZE]>() - core::mem::size_of::<VirtqAvail>()],
    used:  VirtqUsed,
}

static mut BLK_VRING: BlkVring = BlkVring {
    desc:  [VirtqDesc { addr: 0, len: 0, flags: 0, next: 0 }; VRING_SIZE],
    avail: VirtqAvail { flags: 0, idx: 0, ring: [0; VRING_SIZE] },
    _pad:  [0u8; 4096 - core::mem::size_of::<[VirtqDesc; VRING_SIZE]>() - core::mem::size_of::<VirtqAvail>()],
    used:  VirtqUsed  { flags: 0, idx: 0, ring: [VirtqUsedElem { id: 0, len: 0 }; VRING_SIZE] },
};

// Request header + status byte live here between requests.
#[repr(C, align(512))]
struct BlkReqBuf {
    hdr:    BlkReqHdr,
    status: u8,
}
static mut BLK_REQ_BUF: BlkReqBuf = BlkReqBuf {
    hdr:    BlkReqHdr { typ: 0, _res: 0, sector: 0 },
    status: 0,
};

// 512-byte sector staging buffer.
#[repr(C, align(512))]
struct SectorBuf([u8; 512]);
static mut SECTOR_BUF: SectorBuf = SectorBuf([0u8; 512]);

// ── Driver state ──────────────────────────────────────────────────────────────

struct BlkState {
    ready:    bool,
    io_base:  u16,
    capacity: u64,   // in 512-byte sectors
    read_only: bool,
    avail_idx: u16,  // shadow avail->idx
    used_idx:  u16,  // last consumed used->idx
}
static BLK: Mutex<BlkState> = Mutex::new(BlkState {
    ready:    false,
    io_base:  0,
    capacity: 0,
    read_only: false,
    avail_idx: 0,
    used_idx:  0,
});

static BLK_READY: AtomicBool = AtomicBool::new(false);

// ── PCI probe ─────────────────────────────────────────────────────────────────

pub fn init() {
    // Scan PCI bus 0 for virtio-blk (vendor=0x1AF4, device=0x1001).
    'outer: for dev in 0u8..32 {
        let id = pci_cfg_read32(0, dev, 0, 0x00);
        if id == 0xFFFF_FFFF { continue; }
        let vendor = (id & 0xFFFF) as u16;
        let device = (id >> 16) as u16;
        if vendor != 0x1AF4 { continue; }
        if device != 0x1001 { continue; } // virtio-blk legacy

        let bar0_raw = pci_cfg_read32(0, dev, 0, 0x10);
        if bar0_raw & 1 == 0 {
            log::warn!("[virtio-blk] BAR0 is MMIO — only I/O BARs supported");
            continue;
        }
        let io_base = (bar0_raw & !0x3) as u16;

        // Enable bus mastering + I/O space.
        let cmd = pci_cfg_read32(0, dev, 0, 0x04);
        pci_cfg_write32(0, dev, 0, 0x04, cmd | 0x5);

        log::info!("[virtio-blk] found PCI {:02x}.{} I/O base={:#x}", dev, 0, io_base);

        if setup_device(io_base) {
            break 'outer;
        }
    }
}

fn setup_device(io_base: u16) -> bool {
    unsafe {
        // 1. Reset device.
        outb(io_base + VIRTIO_DEVICE_STATUS, VIRTIO_STATUS_RESET);
        // 2. Acknowledge.
        outb(io_base + VIRTIO_DEVICE_STATUS, VIRTIO_STATUS_ACKNOWLEDGE);
        // 3. Driver present.
        outb(io_base + VIRTIO_DEVICE_STATUS, VIRTIO_STATUS_ACKNOWLEDGE | VIRTIO_STATUS_DRIVER);

        // 4. Negotiate features — accept none beyond baseline.
        let _features = inl(io_base + VIRTIO_DEVICE_FEATURES);
        outl(io_base + VIRTIO_GUEST_FEATURES, 0);

        // 5. Features OK.
        outb(io_base + VIRTIO_DEVICE_STATUS,
            VIRTIO_STATUS_ACKNOWLEDGE | VIRTIO_STATUS_DRIVER | VIRTIO_STATUS_FEATURES_OK);

        // 6. Read capacity.
        let cap_lo = inl(io_base + VIRTIO_BLK_CONFIG_CAP_LO) as u64;
        let cap_hi = inl(io_base + VIRTIO_BLK_CONFIG_CAP_HI) as u64;
        let capacity = (cap_hi << 32) | cap_lo;

        // 7. Set up virtqueue 0.
        outw_ext(io_base + VIRTIO_QUEUE_SELECT, 0);
        let qsize = inw(io_base + VIRTIO_QUEUE_SIZE);
        if qsize == 0 {
            log::warn!("[virtio-blk] virtq 0 size=0, giving up");
            return false;
        }
        // Write physical page number of descriptor ring.
        let vring_phys = &BLK_VRING as *const BlkVring as u64;
        outl(io_base + VIRTIO_QUEUE_ADDRESS, (vring_phys >> 12) as u32);

        // 8. Driver OK.
        outb(io_base + VIRTIO_DEVICE_STATUS,
            VIRTIO_STATUS_ACKNOWLEDGE | VIRTIO_STATUS_DRIVER | VIRTIO_STATUS_FEATURES_OK | VIRTIO_STATUS_DRIVER_OK);

        let mut blk = BLK.lock();
        blk.ready    = true;
        blk.io_base  = io_base;
        blk.capacity = capacity;
        blk.read_only = false;
        blk.avail_idx = 0;
        blk.used_idx  = 0;
        BLK_READY.store(true, Ordering::Release);

        log::info!("[virtio-blk] ready — {} sectors ({} MiB)",
            capacity, capacity / 2048);
        true
    }
}

// outw helper (not in scope from std).
unsafe fn outw_ext(port: u16, val: u16) {
    core::arch::asm!("out dx, ax", in("dx") port, in("ax") val, options(nomem, nostack));
}

// ── Public API ────────────────────────────────────────────────────────────────

pub fn is_ready() -> bool { BLK_READY.load(Ordering::Acquire) }

/// Return device capacity in 512-byte sectors.
pub fn capacity_sectors() -> u64 {
    let blk = BLK.lock();
    blk.capacity
}

/// Read `count` sectors starting at `sector` into `buf`.
/// `buf` must be at least `count * 512` bytes.
pub fn read_sectors(sector: u64, count: u64, buf: &mut [u8]) -> Result<(), &'static str> {
    if !is_ready() { return Err("device not ready"); }
    if buf.len() < (count * 512) as usize { return Err("buffer too small"); }
    let mut blk = BLK.lock();
    for i in 0..count {
        do_sector_op(&mut blk, VIRTIO_BLK_T_IN,  sector + i, None)?;
        let dst = &mut buf[(i as usize * 512)..(i as usize * 512 + 512)];
        dst.copy_from_slice(unsafe { &SECTOR_BUF.0 });
    }
    Ok(())
}

/// Write `count` sectors starting at `sector` from `data`.
pub fn write_sectors(sector: u64, count: u64, data: &[u8]) -> Result<(), &'static str> {
    if !is_ready() { return Err("device not ready"); }
    if blk_is_readonly() { return Err("read-only device"); }
    if data.len() < (count * 512) as usize { return Err("data too small"); }
    let mut blk = BLK.lock();
    for i in 0..count {
        let src = &data[(i as usize * 512)..(i as usize * 512 + 512)];
        unsafe { SECTOR_BUF.0.copy_from_slice(src); }
        do_sector_op(&mut blk, VIRTIO_BLK_T_OUT, sector + i, Some(src))?;
    }
    Ok(())
}

fn blk_is_readonly() -> bool {
    BLK.lock().read_only
}

/// Fill info text into `out` buffer (for SYS_BLK_INFO).
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

// ── Internal virtq helpers ────────────────────────────────────────────────────

/// Submit a single-sector read or write via the virtqueue.
fn do_sector_op(blk: &mut BlkState, typ: u32, sector: u64, _data: Option<&[u8]>) -> Result<(), &'static str> {
    unsafe {
        // Fill request header.
        BLK_REQ_BUF.hdr.typ    = typ;
        BLK_REQ_BUF.hdr._res   = 0;
        BLK_REQ_BUF.hdr.sector = sector;
        BLK_REQ_BUF.status     = 0xFF; // sentinel — device writes 0 on success

        let hdr_phys    = &BLK_REQ_BUF.hdr    as *const BlkReqHdr  as u64;
        let buf_phys    = &SECTOR_BUF.0        as *const u8         as u64;
        let status_phys = &BLK_REQ_BUF.status  as *const u8         as u64;

        // Descriptor 0: request header (read by device)
        BLK_VRING.desc[0] = VirtqDesc {
            addr:  hdr_phys,
            len:   core::mem::size_of::<BlkReqHdr>() as u32,
            flags: VIRTQ_DESC_F_NEXT,
            next:  1,
        };
        // Descriptor 1: data buffer (write if READ, read if WRITE)
        let data_flags = if typ == VIRTIO_BLK_T_IN { VIRTQ_DESC_F_NEXT | VIRTQ_DESC_F_WRITE } else { VIRTQ_DESC_F_NEXT };
        BLK_VRING.desc[1] = VirtqDesc {
            addr:  buf_phys,
            len:   512,
            flags: data_flags,
            next:  2,
        };
        // Descriptor 2: status byte (always writable by device)
        BLK_VRING.desc[2] = VirtqDesc {
            addr:  status_phys,
            len:   1,
            flags: VIRTQ_DESC_F_WRITE,
            next:  0,
        };

        // Put descriptor chain head (0) into available ring.
        let ai = (blk.avail_idx as usize) % VRING_SIZE;
        BLK_VRING.avail.ring[ai] = 0;
        core::arch::asm!("mfence", options(nostack, nomem));
        BLK_VRING.avail.idx = blk.avail_idx.wrapping_add(1);
        blk.avail_idx = blk.avail_idx.wrapping_add(1);
        core::arch::asm!("mfence", options(nostack, nomem));

        // Notify device (kick virtq 0).
        outw_ext(blk.io_base + VIRTIO_QUEUE_NOTIFY, 0);

        // Poll used ring until device acknowledges.
        let mut spins = 0usize;
        loop {
            core::arch::asm!("mfence", options(nostack, nomem));
            if BLK_VRING.used.idx != blk.used_idx {
                blk.used_idx = BLK_VRING.used.idx;
                break;
            }
            spins += 1;
            if spins > 10_000_000 {
                return Err("virtio-blk: timeout");
            }
        }

        if BLK_REQ_BUF.status != 0 {
            return Err("virtio-blk: I/O error");
        }
    }
    Ok(())
}

// ── Tiny dec formatter (no alloc needed) ─────────────────────────────────────

fn u64_to_dec(mut n: u64) -> &'static str {
    // Thread-unsafe but fine for single-CPU kernel display.
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
