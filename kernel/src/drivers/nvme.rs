//! Phase 59 — NVMe (NVM Express) driver.
//!
//! Scans PCI for an NVMe controller (class 0x01, subclass 0x08, prog-if 0x02),
//! maps BAR0, sets up admin submission/completion queues, creates a single I/O
//! queue pair, and exposes `read_sectors` / `write_sectors`.
//!
//! This is a polling (non-interrupt) driver suitable for early kernel bring-up.
//! Each I/O operation submits a single NVM Read or Write command and spins on
//! the completion queue doorbell.

use core::sync::atomic::{AtomicBool, Ordering};
use spin::Mutex;

// ── NVMe register offsets (BAR0) ─────────────────────────────────────────────

const OFF_CAP:    usize = 0x00;   // Controller Capabilities (64-bit)
const OFF_VS:     usize = 0x08;   // Version
const OFF_INTMS:  usize = 0x0C;   // Interrupt Mask Set
const OFF_CC:     usize = 0x14;   // Controller Configuration
const OFF_CSTS:   usize = 0x1C;   // Controller Status
const OFF_AQA:    usize = 0x24;   // Admin Queue Attributes
const OFF_ASQ:    usize = 0x28;   // Admin Submission Queue Base (64-bit)
const OFF_ACQ:    usize = 0x30;   // Admin Completion Queue Base (64-bit)

// CC fields
const CC_EN:      u32 = 1 << 0;
const CC_CSS_NVM: u32 = 0 << 4;  // NVM command set
const CC_MPS0:    u32 = 0 << 7;  // host page size = 2^(12+0) = 4 KiB
const CC_AMS_RR:  u32 = 0 << 11; // Round-robin arbitration
const CC_IOSQES:  u32 = 6 << 16; // I/O SQ entry size = 2^6 = 64 bytes
const CC_IOCQES:  u32 = 4 << 20; // I/O CQ entry size = 2^4 = 16 bytes

// CSTS fields
const CSTS_RDY:   u32 = 1 << 0;
const CSTS_CFS:   u32 = 1 << 1;

// Admin command opcodes
const ADM_CREATE_IO_CQ: u8 = 0x05;
const ADM_CREATE_IO_SQ: u8 = 0x01;
const ADM_IDENTIFY:     u8 = 0x06;

// NVM command opcodes
const NVM_READ:  u8 = 0x02;
const NVM_WRITE: u8 = 0x01;

const QUEUE_DEPTH: usize = 64;

// ── Submission queue entry (64 bytes) ────────────────────────────────────────

#[repr(C)]
struct SqEntry {
    cdw0:   u32,   // opc[7:0] | fuse[9:8] | psdt[15:14] | cid[31:16]
    nsid:   u32,
    cdw2:   u32,
    cdw3:   u32,
    mptr:   u64,   // metadata pointer
    prp1:   u64,   // PRP 1
    prp2:   u64,   // PRP 2
    cdw10:  u32,
    cdw11:  u32,
    cdw12:  u32,
    cdw13:  u32,
    cdw14:  u32,
    cdw15:  u32,
}

impl SqEntry {
    const fn zeroed() -> Self {
        Self { cdw0: 0, nsid: 0, cdw2: 0, cdw3: 0, mptr: 0,
               prp1: 0, prp2: 0, cdw10: 0, cdw11: 0, cdw12: 0,
               cdw13: 0, cdw14: 0, cdw15: 0 }
    }
}

// ── Completion queue entry (16 bytes) ────────────────────────────────────────

#[repr(C)]
struct CqEntry {
    dw0: u32,
    dw1: u32,
    dw2: u32,
    dw3: u32,  // phase bit in bit 16, status in bits [31:17]
}

impl CqEntry {
    const fn zeroed() -> Self { Self { dw0: 0, dw1: 0, dw2: 0, dw3: 0 } }
}

// ── Static queue storage ──────────────────────────────────────────────────────

static mut ADM_SQ:  [SqEntry; QUEUE_DEPTH] = [const { SqEntry::zeroed() }; QUEUE_DEPTH];
static mut ADM_CQ:  [CqEntry; QUEUE_DEPTH] = [const { CqEntry::zeroed() }; QUEUE_DEPTH];
static mut IO_SQ:   [SqEntry; QUEUE_DEPTH] = [const { SqEntry::zeroed() }; QUEUE_DEPTH];
static mut IO_CQ:   [CqEntry; QUEUE_DEPTH] = [const { CqEntry::zeroed() }; QUEUE_DEPTH];

/// 4 KiB aligned transfer buffer (one sector at a time for simplicity).
#[repr(align(4096))]
struct TransferBuf([u8; 512 * 8]);
static mut XFER_BUF: TransferBuf = TransferBuf([0u8; 512 * 8]);

// ── Controller state ─────────────────────────────────────────────────────────

struct NvmeState {
    bar0:          u64,   // HHDM virtual address of BAR0
    hhdm:          u64,
    adm_sq_tail:   u16,
    adm_cq_head:   u16,
    adm_phase:     bool,
    io_sq_tail:    u16,
    io_cq_head:    u16,
    io_phase:      bool,
    cid:           u16,
    doorbell_stride: u32, // in bytes (2^(2 + DSTRD) from CAP)
    nsid:          u32,
    capacity_lba:  u64,
}

static NVME: Mutex<Option<NvmeState>> = Mutex::new(None);
static NVME_READY: AtomicBool = AtomicBool::new(false);

use crate::arch::mmio;

// ── Register accessors ───────────────────────────────────────────────────────

#[inline] unsafe fn reg32_read(bar0: u64, off: usize) -> u32 {
    mmio::read32(bar0, off)
}
#[inline] unsafe fn reg32_write(bar0: u64, off: usize, v: u32) {
    mmio::write32(bar0, off, v);
}
#[inline] unsafe fn reg64_write(bar0: u64, off: usize, v: u64) {
    mmio::write64(bar0, off, v);
}

fn doorbell_off(state: &NvmeState, qid: u16, tail: bool) -> usize {
    // Doorbell base = BAR0 + 0x1000.  Each queue has a pair of 32-bit
    // doorbells at stride bytes apart.
    let s = state.doorbell_stride as usize;
    0x1000 + (qid as usize * 2 + if tail { 0 } else { 1 }) * s
}

unsafe fn ring_adm_sq(state: &mut NvmeState) {
    let off = doorbell_off(state, 0, true);
    reg32_write(state.bar0, off, state.adm_sq_tail as u32);
}
unsafe fn ring_adm_cq(state: &mut NvmeState) {
    let off = doorbell_off(state, 0, false);
    reg32_write(state.bar0, off, state.adm_cq_head as u32);
}
unsafe fn ring_io_sq(state: &mut NvmeState) {
    let off = doorbell_off(state, 1, true);
    reg32_write(state.bar0, off, state.io_sq_tail as u32);
}
unsafe fn ring_io_cq(state: &mut NvmeState) {
    let off = doorbell_off(state, 1, false);
    reg32_write(state.bar0, off, state.io_cq_head as u32);
}

// ── Admin command helpers ─────────────────────────────────────────────────────

unsafe fn submit_admin_cmd(state: &mut NvmeState, entry: SqEntry) {
    let tail = state.adm_sq_tail as usize;
    ADM_SQ[tail] = entry;
    state.adm_sq_tail = ((tail + 1) % QUEUE_DEPTH) as u16;
    ring_adm_sq(state);
}

/// Poll admin completion queue, returns status DW3.
unsafe fn poll_admin_cq(state: &mut NvmeState) -> u32 {
    let head = state.adm_cq_head as usize;
    let phase = state.adm_phase;
    for _ in 0..1_000_000 {
        let entry = &ADM_CQ[head];
        let dw3 = core::ptr::read_volatile(&entry.dw3);
        let entry_phase = (dw3 >> 16) & 1 != 0;
        if entry_phase == phase {
            let status = (dw3 >> 17) & 0x7FFF;
            state.adm_cq_head = ((head + 1) % QUEUE_DEPTH) as u16;
            if state.adm_cq_head == 0 { state.adm_phase = !state.adm_phase; }
            ring_adm_cq(state);
            return status;
        }
        core::hint::spin_loop();
    }
    0xFFFF // timeout
}

fn next_cid(state: &mut NvmeState) -> u16 {
    let c = state.cid;
    state.cid = state.cid.wrapping_add(1);
    c
}

// ── Public init ───────────────────────────────────────────────────────────────

/// Scan PCI for an NVMe controller and initialise it.
/// Should be called once during kernel init after PCI enumeration is available.
pub fn init() {
    let hhdm = crate::mm::frame_allocator::hhdm_offset();

    // Scan PCI for NVMe: class=0x01, sub=0x08, progif=0x02
    let bar0_phys = match crate::arch::pci::find_device_bar0(0x01, 0x08, 0x02) {
        Some(b) => b,
        None => {
            log::info!("[NVMe] no controller found");
            return;
        }
    };
    let bar0 = bar0_phys + hhdm;
    log::info!("[NVMe] controller BAR0 phys={:#x}", bar0_phys);

    // Map the NVMe MMIO region into the kernel address space before any access.
    // NVMe BAR0: base registers (0–0xFFF) + doorbell page (0x1000–0x1FFF) = 8 KiB.
    // Map 4 pages (16 KiB) to cover all doorbells for up to 4 queue pairs.
    unsafe { crate::mm::paging::map_mmio(bar0_phys, bar0, 0x4000); }

    unsafe {
        // Read capability register for doorbell stride and minimum page size.
        let cap_lo = reg32_read(bar0, OFF_CAP);
        let dstrd = (reg32_read(bar0, OFF_CAP + 4) >> 0) & 0xF;
        let doorbell_stride = 4u32 << dstrd;

        // Disable controller.
        reg32_write(bar0, OFF_CC, 0);
        // Mask all interrupts.
        reg32_write(bar0, OFF_INTMS, 0xFFFF_FFFF);
        // Wait for CSTS.RDY = 0.
        for _ in 0..100_000 {
            if reg32_read(bar0, OFF_CSTS) & CSTS_RDY == 0 { break; }
            core::hint::spin_loop();
        }

        // Set up admin queue sizes (0-based: depth-1).
        let aqa = ((QUEUE_DEPTH as u32 - 1) << 16) | (QUEUE_DEPTH as u32 - 1);
        reg32_write(bar0, OFF_AQA, aqa);

        let asq_phys = ADM_SQ.as_ptr() as u64 - hhdm;
        let acq_phys = ADM_CQ.as_ptr() as u64 - hhdm;
        reg64_write(bar0, OFF_ASQ, asq_phys);
        reg64_write(bar0, OFF_ACQ, acq_phys);

        // Enable controller with NVM command set.
        let cc = CC_EN | CC_CSS_NVM | CC_MPS0 | CC_AMS_RR | CC_IOSQES | CC_IOCQES;
        reg32_write(bar0, OFF_CC, cc);

        // Wait for CSTS.RDY = 1.
        let mut ready = false;
        for _ in 0..1_000_000 {
            let csts = reg32_read(bar0, OFF_CSTS);
            if csts & CSTS_CFS != 0 {
                log::error!("[NVMe] controller fatal status");
                return;
            }
            if csts & CSTS_RDY != 0 { ready = true; break; }
            core::hint::spin_loop();
        }
        if !ready {
            log::error!("[NVMe] controller did not become ready");
            return;
        }

        let mut state = NvmeState {
            bar0,
            hhdm,
            adm_sq_tail: 0,
            adm_cq_head: 0,
            adm_phase: true,
            io_sq_tail: 0,
            io_cq_head: 0,
            io_phase: true,
            cid: 0,
            doorbell_stride,
            nsid: 1,
            capacity_lba: 0,
        };

        // Create I/O Completion Queue (qid=1).
        let io_cq_phys = IO_CQ.as_ptr() as u64 - hhdm;
        let cid = next_cid(&mut state);
        let mut cmd = SqEntry::zeroed();
        cmd.cdw0 = (ADM_CREATE_IO_CQ as u32) | ((cid as u32) << 16);
        cmd.prp1 = io_cq_phys;
        cmd.cdw10 = ((QUEUE_DEPTH as u32 - 1) << 16) | 1; // QID=1
        cmd.cdw11 = 1; // PC=1 (physically contiguous)
        submit_admin_cmd(&mut state, cmd);
        let st = poll_admin_cq(&mut state);
        if st != 0 { log::error!("[NVMe] CREATE_IO_CQ failed status={:#x}", st); return; }

        // Create I/O Submission Queue (qid=1, cqid=1).
        let io_sq_phys = IO_SQ.as_ptr() as u64 - hhdm;
        let cid = next_cid(&mut state);
        let mut cmd = SqEntry::zeroed();
        cmd.cdw0 = (ADM_CREATE_IO_SQ as u32) | ((cid as u32) << 16);
        cmd.prp1 = io_sq_phys;
        cmd.cdw10 = ((QUEUE_DEPTH as u32 - 1) << 16) | 1; // QID=1
        cmd.cdw11 = (1 << 16) | 1; // CQID=1, PC=1
        submit_admin_cmd(&mut state, cmd);
        let st = poll_admin_cq(&mut state);
        if st != 0 { log::error!("[NVMe] CREATE_IO_SQ failed status={:#x}", st); return; }

        // Identify namespace 1 to get LBA count.
        let ident_phys = XFER_BUF.0.as_ptr() as u64 - hhdm;
        let cid = next_cid(&mut state);
        let mut cmd = SqEntry::zeroed();
        cmd.cdw0 = (ADM_IDENTIFY as u32) | ((cid as u32) << 16);
        cmd.nsid = 1;
        cmd.prp1 = ident_phys;
        cmd.cdw10 = 0; // CNS=0: identify namespace
        submit_admin_cmd(&mut state, cmd);
        let st = poll_admin_cq(&mut state);
        if st == 0 {
            // NSZE is at bytes 0..8 of identify namespace data.
            let nsze = u64::from_le_bytes(XFER_BUF.0[0..8].try_into().unwrap_or([0u8; 8]));
            state.capacity_lba = nsze;
            log::info!("[NVMe] namespace 1 capacity = {} LBAs", nsze);
        }

        let _ = cap_lo; // used indirectly via dstrd
        *NVME.lock() = Some(state);
        NVME_READY.store(true, Ordering::Release);
        log::info!("[NVMe] driver ready (doorbell_stride={})", doorbell_stride);
    }
}

pub fn is_ready() -> bool {
    NVME_READY.load(Ordering::Acquire)
}

pub fn info_text(out: &mut [u8]) -> usize {
    let mut g = NVME.lock();
    if let Some(s) = g.as_ref() {
        let msg = alloc::format!("nvme: ready, ns1_lba={}, dbs={}\n",
            s.capacity_lba, s.doorbell_stride);
        let n = msg.len().min(out.len());
        out[..n].copy_from_slice(&msg.as_bytes()[..n]);
        n
    } else {
        let msg = b"nvme: not present\n";
        let n = msg.len().min(out.len());
        out[..n].copy_from_slice(&msg[..n]);
        n
    }
}

pub fn read_sectors(lba: u64, count: u32, buf: &mut [u8]) -> Result<(), &'static str> {
    if !is_ready() { return Err("nvme not ready"); }
    if buf.len() < count as usize * 512 { return Err("buffer too small"); }
    let mut g = NVME.lock();
    let s = g.as_mut().ok_or("nvme state missing")?;
    unsafe { do_io(s, lba, count, buf, false) }
}

pub fn write_sectors(lba: u64, count: u32, data: &[u8]) -> Result<(), &'static str> {
    if !is_ready() { return Err("nvme not ready"); }
    if data.len() < count as usize * 512 { return Err("buffer too small"); }
    let mut g = NVME.lock();
    let s = g.as_mut().ok_or("nvme state missing")?;
    // Copy user data into aligned xfer buffer.
    let len = (count as usize * 512).min(unsafe { XFER_BUF.0.len() });
    unsafe { XFER_BUF.0[..len].copy_from_slice(&data[..len]); }
    unsafe { do_io(s, lba, count, &mut [], true) }
}

unsafe fn do_io(state: &mut NvmeState, lba: u64, count: u32,
                buf: &mut [u8], is_write: bool) -> Result<(), &'static str> {
    let xfer_phys = XFER_BUF.0.as_ptr() as u64 - state.hhdm;

    if !is_write {
        // Zero xfer buf so stale data isn't returned on timeout.
        XFER_BUF.0.fill(0);
    }

    let cid = next_cid(state);
    let opc = if is_write { NVM_WRITE } else { NVM_READ };
    let mut cmd = SqEntry::zeroed();
    cmd.cdw0 = (opc as u32) | ((cid as u32) << 16);
    cmd.nsid = state.nsid;
    cmd.prp1 = xfer_phys;
    cmd.cdw10 = lba as u32;
    cmd.cdw11 = (lba >> 32) as u32;
    cmd.cdw12 = count - 1; // NLB is 0-based

    let tail = state.io_sq_tail as usize;
    IO_SQ[tail] = cmd;
    state.io_sq_tail = ((tail + 1) % QUEUE_DEPTH) as u16;
    ring_io_sq(state);

    // Poll I/O completion queue.
    let head = state.io_cq_head as usize;
    let phase = state.io_phase;
    let mut ok = false;
    for _ in 0..2_000_000 {
        let entry = &IO_CQ[head];
        let dw3 = core::ptr::read_volatile(&entry.dw3);
        let entry_phase = (dw3 >> 16) & 1 != 0;
        if entry_phase == phase {
            let status = (dw3 >> 17) & 0x7FFF;
            state.io_cq_head = ((head + 1) % QUEUE_DEPTH) as u16;
            if state.io_cq_head == 0 { state.io_phase = !state.io_phase; }
            ring_io_cq(state);
            if status != 0 { return Err("nvme io error"); }
            ok = true;
            break;
        }
        core::hint::spin_loop();
    }
    if !ok { return Err("nvme io timeout"); }

    if !is_write {
        let len = (count as usize * 512).min(buf.len()).min(unsafe { XFER_BUF.0.len() });
        buf[..len].copy_from_slice(unsafe { &XFER_BUF.0[..len] });
    }
    Ok(())
}
