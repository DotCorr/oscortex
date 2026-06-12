//! Kernel heap — linked-list allocator backed by the frame allocator.
//!
//! We use the `linked_list_allocator` crate (no_std compatible) for the
//! global allocator. The initial heap is carved out of the physical memory
//! map at init time. The Cortex can expand the heap at runtime.

use linked_list_allocator::LockedHeap;
use core::alloc::{GlobalAlloc, Layout};

/// The raw linked-list heap. Its lock is a plain spinlock — NOT interrupt-safe.
static HEAP: LockedHeap = LockedHeap::empty();

/// Interrupt-safe global allocator wrapper.
///
/// The kernel allocates from interrupt context: the generic-timer ISR
/// (`check_timerfds_and_wake_try`, `handle_pending_wakes_try`) builds `Vec`s and
/// touches `BTreeMap`s, all of which hit this global allocator. `LockedHeap`'s
/// inner lock is a bare spinlock, so if a timer tick lands while the interrupted
/// thread holds the heap lock, the ISR's allocation either deadlocks on the lock
/// or (worse) races the hole-list bookkeeping — observed as nondeterministic
/// `linked_list_allocator` panics ("Hole list out of order", "subtract with
/// overflow", `assertion left == right failed`) once the engine starts doing
/// real heap-heavy work. Masking IRQs around every heap operation makes alloc/
/// dealloc atomic w.r.t. the timer ISR, so the ISR can never observe or mutate a
/// half-updated hole list. (x86 is implicitly safe because its allocation-heavy
/// ISR paths run with interrupts masked; aarch64 runs the syscall/ISR with IRQs
/// unmasked, exposing the race.)
struct IrqSafeHeap;

unsafe fn with_irqs_off<R>(f: impl FnOnce() -> R) -> R {
    let flags = crate::arch::interrupts_save_and_disable();
    let r = f();
    crate::arch::interrupts_restore(flags);
    r
}

unsafe impl GlobalAlloc for IrqSafeHeap {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        with_irqs_off(|| HEAP.alloc(layout))
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        with_irqs_off(|| HEAP.dealloc(ptr, layout))
    }
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        with_irqs_off(|| HEAP.alloc_zeroed(layout))
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        with_irqs_off(|| HEAP.realloc(ptr, layout, new_size))
    }
}

#[global_allocator]
static ALLOCATOR: IrqSafeHeap = IrqSafeHeap;

const INITIAL_HEAP_SIZE: usize = 64 * 1024 * 1024; // 64 MiB — needed for Flutter double-buffers
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

/// Expand the kernel heap when free space is low (best-effort).
pub fn expand(extra_bytes: usize) -> Result<(), &'static str> {
    let (_used, free, _total) = stats();
    if free >= extra_bytes {
        log::info!("[MM::Heap] expand({}) — sufficient free space ({} bytes)", extra_bytes, free);
        return Ok(());
    }
    Err("heap expansion requires contiguous frame grow (not implemented)")
}

#[alloc_error_handler]
fn alloc_error(layout: core::alloc::Layout) -> ! {
    panic!("Kernel heap allocation failed: size={} align={}", layout.size(), layout.align());
}
