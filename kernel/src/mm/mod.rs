//! Memory management subsystem.

pub mod frame_allocator;
pub mod heap;
pub mod paging;

pub use paging::{map_mmio, map_page, PageFlags};

#[cfg(target_arch = "x86_64")]
use limine::request::{MemmapResponse, ExecutableAddressResponse};

// ── Architecture-neutral boot memory map ─────────────────────────────────────
//
// The frame allocator needs a list of *usable* physical RAM regions. x86_64
// gets that from the Limine memory map; aarch64 (QEMU `-M virt`, `-kernel`,
// no bootloader) gets it from the device tree. To keep the shared mm init free
// of any single boot protocol, both arches translate their source into this
// small, fixed-capacity, no-alloc region list and hand it to `frame_allocator`.

/// One usable physical RAM region: `[base, base+len)`.
#[derive(Clone, Copy, Debug)]
pub struct BootMemRegion {
    pub base: u64,
    pub len: u64,
}

/// Maximum distinct usable regions we retain. Limine emits a handful; QEMU virt
/// reports one or two. 32 is comfortable headroom.
pub const MAX_BOOT_REGIONS: usize = 32;

/// A fixed-capacity list of usable physical RAM regions, built by the arch boot
/// path and consumed by `frame_allocator::init_from_regions`.
#[derive(Clone, Copy)]
pub struct BootMemMap {
    regions: [BootMemRegion; MAX_BOOT_REGIONS],
    count: usize,
}

impl BootMemMap {
    pub const fn new() -> Self {
        BootMemMap {
            regions: [BootMemRegion { base: 0, len: 0 }; MAX_BOOT_REGIONS],
            count: 0,
        }
    }

    /// Append a usable region (silently dropped if capacity is exceeded or the
    /// region is empty).
    pub fn push(&mut self, base: u64, len: u64) {
        if len == 0 || self.count >= MAX_BOOT_REGIONS {
            return;
        }
        self.regions[self.count] = BootMemRegion { base, len };
        self.count += 1;
    }

    /// The usable regions discovered at boot.
    pub fn regions(&self) -> &[BootMemRegion] {
        &self.regions[..self.count]
    }
}

impl Default for BootMemMap {
    fn default() -> Self {
        Self::new()
    }
}

// ── x86_64 boot path (Limine) ────────────────────────────────────────────────

/// Initialise memory management from the Limine boot info (x86_64 path).
///
/// Behaviour is byte-equivalent to the prior direct-Limine implementation: it
/// simply translates the Limine memory map into the neutral [`BootMemMap`] and
/// forwards to the shared [`init_from_regions`].
#[cfg(target_arch = "x86_64")]
pub fn init(
    mmap: &MemmapResponse,
    hhdm_offset: u64,
    _kaddr: &ExecutableAddressResponse,
) {
    use limine::memmap::MEMMAP_USABLE;
    let mut map = BootMemMap::new();
    for entry in mmap.entries() {
        if entry.type_ == MEMMAP_USABLE {
            map.push(entry.base, entry.length);
        }
    }
    init_from_regions(&map, hhdm_offset);
}

// ── Shared init (arch-neutral) ───────────────────────────────────────────────

/// Initialise memory management from an arch-provided usable-region list.
///
/// `hhdm_offset` is the physical→virtual direct-map offset (Limine HHDM on
/// x86_64; 0 on aarch64 where the bring-up identity-maps RAM so VA == PA).
pub fn init_from_regions(map: &BootMemMap, hhdm_offset: u64) {
    frame_allocator::init_from_regions(map, hhdm_offset);
    paging::init(hhdm_offset);
    heap::init();
    log::info!("[MM] Memory management online");
}

/// Report reclaimable headroom (free heap bytes) for the self-healer.
pub fn reclaim_best_effort() -> usize {
    let (_used, free, _total) = heap::stats();
    free
}
