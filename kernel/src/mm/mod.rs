//! Memory management subsystem.

pub mod frame_allocator;
pub mod heap;
pub mod paging;

pub use paging::{map_mmio, map_page, PageFlags};

use limine::request::{MemmapResponse, ExecutableAddressResponse};

pub fn init(
    mmap: &MemmapResponse,
    hhdm_offset: u64,
    _kaddr: &ExecutableAddressResponse,
) {
    frame_allocator::init(mmap, hhdm_offset);
    paging::init(hhdm_offset);
    heap::init();
    log::info!("[MM] Memory management online");
}

/// Report reclaimable headroom (free heap bytes) for the self-healer.
pub fn reclaim_best_effort() -> usize {
    let (_used, free, _total) = heap::stats();
    free
}
