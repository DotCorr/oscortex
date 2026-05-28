//! Architecture abstraction layer.
//!
//! All arch-specific code lives behind this module. The kernel never accesses
//! CPU instructions directly — always through `arch::*`.

cfg_if::cfg_if! {
    if #[cfg(target_arch = "x86_64")] {
        mod x86_64;
        pub use x86_64::*;
    } else if #[cfg(target_arch = "aarch64")] {
        mod aarch64;
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

/// Trigger an ACPI S5 soft-off (architecture-specific implementation).
pub fn acpi_shutdown() -> ! {
    #[cfg(target_arch = "x86_64")]
    x86_64::acpi::shutdown();
    #[cfg(not(target_arch = "x86_64"))]
    loop { unsafe { core::arch::asm!("wfi", options(nomem, nostack)); } }
}

