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

/// Maximum distinct usable regions we retain. Limine typically emits well under
/// a dozen usable entries (and QEMU virt one or two), but a fragmented memory map
/// can have more; 128 is generous headroom so no usable RAM is ever dropped.
pub const MAX_BOOT_REGIONS: usize = 128;

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

    // aarch64: the kernel is loaded by `-kernel` INTO the same RAM region the
    // device tree reports as usable (QEMU virt: image at 0x40080000, RAM from
    // 0x40000000). The frame allocator just marked all of that free, so without
    // reserving the kernel image the very first contiguous allocation (the 64 MiB
    // heap) is handed 0x40000000 and its span overwrites live kernel code — a
    // latent corruption that surfaces as a hang on the first large heap alloc.
    // Reserve [RAM base .. __kernel_end] before the heap is carved out.
    // (x86 gets this for free from the Limine memory-map types.)
    #[cfg(target_arch = "aarch64")]
    {
        extern "C" {
            static __kernel_end: u8;
        }
        const RAM_BASE: u64 = 0x4000_0000;
        let kernel_end = unsafe { core::ptr::addr_of!(__kernel_end) as u64 };
        frame_allocator::reserve_range(RAM_BASE, kernel_end.saturating_sub(RAM_BASE));
        log::info!(
            "[MM] aarch64: reserved kernel image [{:#x}..{:#x}] ({} KiB)",
            RAM_BASE,
            kernel_end,
            (kernel_end - RAM_BASE) / 1024
        );
    }

    paging::init(hhdm_offset);
    heap::init();
    log::info!("[MM] Memory management online");
}

/// Report reclaimable headroom (free heap bytes) for the self-healer.
pub fn reclaim_best_effort() -> usize {
    let (_used, free, _total) = heap::stats();
    free
}
