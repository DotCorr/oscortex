//! Physical frame allocator — bitmap-based.
//!
//! Uses a static bitmap where each bit represents a 4 KiB physical frame.
//! The bitmap itself is placed in the first usable memory region.

use spin::Mutex;
use core::sync::atomic::{AtomicU64, Ordering};

const FRAME_SIZE: usize = 4096;
const MAX_FRAMES: usize = 1 << 20; // 4 GiB / 4 KiB = 1 M frames
const MIN_ALLOC_PHYS: u64 = 0x10_0000; // keep allocator out of legacy low memory
const MIN_ALLOC_FRAME: usize = (MIN_ALLOC_PHYS as usize) / FRAME_SIZE;

/// Global HHDM offset — set early so arch code can access physical memory
/// via HHDM before full mm::init() runs.
static HHDM_OFFSET: AtomicU64 = AtomicU64::new(0);

pub fn set_hhdm_offset(offset: u64) {
    HHDM_OFFSET.store(offset, Ordering::Release);
}

static BITMAP: Mutex<FrameBitmap> = Mutex::new(FrameBitmap::new());

struct FrameBitmap {
    bits:   [u64; MAX_FRAMES / 64],
    total:  usize,
    used:   usize,
    hhdm:   u64,
}

impl FrameBitmap {
    const fn new() -> Self {
        Self {
            bits: [0u64; MAX_FRAMES / 64],
            total: 0,
            used: 0,
            hhdm: 0,
        }
    }

    fn mark_free(&mut self, frame: usize) {
        self.bits[frame / 64] |= 1 << (frame % 64);
        self.total += 1;
    }

    fn mark_used(&mut self, frame: usize) {
        self.bits[frame / 64] &= !(1 << (frame % 64));
        self.used += 1;
    }

    fn alloc(&mut self) -> Option<u64> {
        for (i, word) in self.bits.iter_mut().enumerate().skip(MIN_ALLOC_FRAME / 64) {
            if *word != 0 {
                let bit = word.trailing_zeros() as usize;
                *word &= !(1u64 << bit);
                self.used += 1;
                let frame = i * 64 + bit;
                if frame < MIN_ALLOC_FRAME {
                    continue;
                }
                return Some((frame as u64) * FRAME_SIZE as u64);
            }
        }
        None
    }

    fn alloc_contiguous(&mut self, count: usize) -> Option<u64> {
        if count == 0 {
            return None;
        }

        let mut run_start = 0usize;
        let mut run_len = 0usize;

        for frame in MIN_ALLOC_FRAME..MAX_FRAMES {
            let is_free = (self.bits[frame / 64] & (1u64 << (frame % 64))) != 0;
            if is_free {
                if run_len == 0 {
                    run_start = frame;
                }
                run_len += 1;
                if run_len == count {
                    for f in run_start..(run_start + count) {
                        self.bits[f / 64] &= !(1u64 << (f % 64));
                    }
                    self.used += count;
                    return Some((run_start as u64) * FRAME_SIZE as u64);
                }
            } else {
                run_len = 0;
            }
        }

        None
    }

    fn free(&mut self, phys: u64) {
        let frame = (phys / FRAME_SIZE as u64) as usize;
        if frame < MAX_FRAMES {
            let mask = 1u64 << (frame % 64);
            let idx = frame / 64;
            if (self.bits[idx] & mask) == 0 {
                self.bits[idx] |= mask;
                self.used = self.used.saturating_sub(1);
            }
        }
    }
}

/// Initialise the frame bitmap from an arch-neutral usable-region list.
///
/// Both the x86_64 (Limine) and aarch64 (device-tree) boot paths funnel through
/// here via [`crate::mm::init_from_regions`].
pub fn init_from_regions(map: &crate::mm::BootMemMap, hhdm_offset: u64) {
    set_hhdm_offset(hhdm_offset);
    let mut bm = BITMAP.lock();
    bm.hhdm = hhdm_offset;
    for region in map.regions() {
        let start_frame = (region.base as usize) / FRAME_SIZE;
        let end_frame = ((region.base + region.len) as usize) / FRAME_SIZE;
        for frame in start_frame.max(MIN_ALLOC_FRAME)..end_frame.min(MAX_FRAMES) {
            bm.mark_free(frame);
        }
    }
    log::info!("[MM::FrameAlloc] {} MiB usable physical memory",
        (bm.total * FRAME_SIZE) / (1024 * 1024));
}

/// Reserve a physical address range `[base, base+len)` so the allocator never
/// hands it out (e.g. the loaded kernel image, the DTB, or a device-claimed
/// buffer). Frames are rounded outward to 4 KiB boundaries. Idempotent.
///
/// Must be called AFTER `init_from_regions` (which marks RAM free) and BEFORE
/// the first allocation, otherwise a free-marked frame inside the range could be
/// handed out — on aarch64 the kernel is loaded into the same RAM region the
/// device tree reports as usable, so without this the heap can overlap the
/// kernel image.
pub fn reserve_range(base: u64, len: u64) {
    if len == 0 {
        return;
    }
    let start_frame = (base / FRAME_SIZE as u64) as usize;
    let end_frame = (((base + len + FRAME_SIZE as u64 - 1) / FRAME_SIZE as u64) as usize)
        .min(MAX_FRAMES);
    let mut bm = BITMAP.lock();
    for frame in start_frame..end_frame {
        let mask = 1u64 << (frame % 64);
        let idx = frame / 64;
        // Only flip frames currently free (bit set) so `used`/`total` stay sane.
        if (bm.bits[idx] & mask) != 0 {
            bm.bits[idx] &= !mask;
            bm.used += 1;
        }
    }
}

/// Allocate a single 4 KiB physical frame. Returns the physical address.
pub fn alloc_frame() -> Option<u64> {
    let res = BITMAP.lock().alloc();
    if res.is_none() {
        let bm = BITMAP.lock();
        log::error!("[MM::FrameAlloc] alloc_frame failed! used={} total={}", bm.used, bm.total);
    }
    res
}

/// Allocate `count` contiguous 4 KiB physical frames.
pub fn alloc_contiguous_frames(count: usize) -> Option<u64> {
    BITMAP.lock().alloc_contiguous(count)
}

/// Free a physical frame by its physical address.
pub fn free_frame(phys: u64) {
    BITMAP.lock().free(phys);
}

/// Return the HHDM offset (physical → virtual mapping base).
pub fn hhdm_offset() -> u64 {
    HHDM_OFFSET.load(Ordering::Acquire)
}

/// Return number of frames currently in use.
pub fn frames_used() -> usize {
    BITMAP.lock().used
}

/// Return total usable frames (set at init).
pub fn frames_total() -> usize {
    BITMAP.lock().total
}

