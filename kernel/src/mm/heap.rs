//! Kernel heap — linked-list allocator backed by the frame allocator.
//!
//! We use the `linked_list_allocator` crate (no_std compatible) for the
//! global allocator. The initial heap is carved out of the physical memory
//! map at init time. The Cortex can expand the heap at runtime.

use linked_list_allocator::LockedHeap;

#[global_allocator]
static HEAP: LockedHeap = LockedHeap::empty();

const INITIAL_HEAP_SIZE: usize = 16 * 1024 * 1024; // 16 MiB initial heap
const HEAP_ALIGN: usize = 4096;

fn early_print_hex_u64(v: u64) {
    let mut buf = [0u8; 18];
    buf[0] = b'0';
    buf[1] = b'x';
    let hex = b"0123456789abcdef";
    for i in 0..16 {
        buf[2 + i] = hex[((v >> (60 - i * 4)) & 0xF) as usize];
    }
    if let Ok(s) = core::str::from_utf8(&buf) {
        crate::logger::early_print(s);
    }
}

pub fn init() {
    // The heap allocator requires one contiguous backing span.
    let frames = INITIAL_HEAP_SIZE / HEAP_ALIGN;
    let heap_phys = crate::mm::frame_allocator::alloc_contiguous_frames(frames)
        .expect("Not enough contiguous physical memory for kernel heap");
    let heap_start = heap_phys + crate::mm::frame_allocator::hhdm_offset();

    crate::logger::early_print("[MM::Heap] init base=");
    early_print_hex_u64(heap_start);
    crate::logger::early_print(" phys=");
    early_print_hex_u64(heap_phys);
    crate::logger::early_print("\r\n");

    unsafe {
        HEAP.lock().init(heap_start as *mut u8, INITIAL_HEAP_SIZE);
    }

    log::info!("[MM::Heap] Kernel heap initialised ({} MiB at {:#x})",
        INITIAL_HEAP_SIZE / (1024 * 1024), heap_start);
}

/// Report current heap stats (used, free, total) in bytes.
pub fn stats() -> (usize, usize, usize) {
    let h = HEAP.lock();
    (h.used(), h.free(), h.size())
}

#[alloc_error_handler]
fn alloc_error(layout: core::alloc::Layout) -> ! {
    panic!("Kernel heap allocation failed: size={} align={}", layout.size(), layout.align());
}
