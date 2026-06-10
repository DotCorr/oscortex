//! Architecture abstraction layer.
//!
//! All arch-specific code lives behind this module. The kernel never accesses
//! CPU instructions directly — always through `arch::*`.

cfg_if::cfg_if! {
    if #[cfg(target_arch = "x86_64")] {
        mod x86_64;
        pub use x86_64::*;
    } else if #[cfg(target_arch = "aarch64")] {
        pub mod aarch64;
        pub use aarch64::*;
    } else if #[cfg(target_arch = "riscv64")] {
        mod riscv64;
        pub use riscv64::*;
    } else {
        compile_error!("Unsupported architecture");
    }
}

/// PCI configuration-space access (arch backend in `arch/pci.rs`).
pub mod pci;
/// Legacy I/O port IN/OUT (arch backend in `arch/port_io.rs`).
pub mod port_io;
/// MMIO load/store helpers for mapped device BARs.
pub mod mmio;

cfg_if::cfg_if! {
    if #[cfg(target_arch = "x86_64")] {
        pub use x86_64::cpu::{interrupts_disable, interrupts_restore, interrupts_save_and_disable, memory_fence, spin_pause};
    } else if #[cfg(target_arch = "aarch64")] {
        pub fn memory_fence() {
            core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);
        }
        pub fn spin_pause() {
            core::hint::spin_loop();
        }
        /// Mask IRQ/FIQ at the current EL (DAIF.I/F).
        pub fn interrupts_disable() {
            unsafe { core::arch::asm!("msr daifset, #3", options(nomem, nostack, preserves_flags)); }
        }
        // Wire to the REAL DAIF save/restore in aarch64::cpu. These were no-op
        // stubs (return 0 / do nothing), so every critical section built on
        // interrupts_save_and_disable()/interrupts_restore() ran with IRQs still
        // ENABLED on aarch64 — the timer ISR could preempt mid-update and corrupt
        // whatever the section guarded (e.g. the kernel heap's hole list, surfacing
        // as nondeterministic linked_list_allocator panics). The real versions
        // mask IRQ+FIQ and restore the prior DAIF.
        pub use aarch64::cpu::{interrupts_save_and_disable, interrupts_restore};
    } else {
        pub fn memory_fence() {
            core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);
        }
        pub fn spin_pause() {
            core::hint::spin_loop();
        }
        pub fn interrupts_disable() {}
        pub fn interrupts_save_and_disable() -> u64 {
            0
        }
        pub fn interrupts_restore(_rflags: u64) {}
    }
}

/// Read the current frame-pointer register (for best-effort backtraces).
///
/// On x86_64 this is RBP; on aarch64 it is X29.  Returns 0 on architectures
/// where no fixed frame pointer is available.
#[inline(always)]
pub fn read_frame_pointer() -> u64 {
    #[cfg(target_arch = "x86_64")]
    {
        let fp: u64;
        unsafe { core::arch::asm!("mov {}, rbp", out(reg) fp, options(nomem, nostack)); }
        fp
    }
    #[cfg(target_arch = "aarch64")]
    {
        let fp: u64;
        unsafe { core::arch::asm!("mov {}, x29", out(reg) fp, options(nomem, nostack)); }
        fp
    }
    #[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
    {
        0
    }
}

/// Trigger an ACPI S5 soft-off (architecture-specific implementation).
pub fn acpi_shutdown() -> ! {
    #[cfg(target_arch = "x86_64")]
    x86_64::acpi::shutdown();
    #[cfg(not(target_arch = "x86_64"))]
    loop { unsafe { core::arch::asm!("wfi", options(nomem, nostack)); } }
}

