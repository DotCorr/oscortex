//! aarch64 bring-up milestone 2: enable the MMU.

use crate::arch::aarch64::mmu;
use crate::arch::aarch64::uart;
use crate::arch::aarch64::uart::a64println;

pub fn run() -> ! {
    a64println!("[boot] Milestone 2 (MMU): building translation tables...");

    // Identity-map device MMIO (low 1 GiB) + 2 GiB of RAM from 0x4000_0000.
    unsafe { mmu::enable(2) };

    // Read SCTLR_EL1 back to confirm M=1. If we get here at all, the MMU is on
    // and translating — the very next instruction fetch was a translated VA.
    let sctlr: u64;
    let ttbr0: u64;
    unsafe {
        core::arch::asm!("mrs {}, sctlr_el1", out(reg) sctlr, options(nomem, nostack));
        core::arch::asm!("mrs {}, ttbr0_el1", out(reg) ttbr0, options(nomem, nostack));
    }
    uart::puts("[boot] SCTLR_EL1 = ");
    uart::puthex_full(sctlr);
    uart::puts(" (MMU ");
    uart::puts(if sctlr & 1 != 0 { "ON" } else { "OFF" });
    uart::puts(")\n");
    uart::puts("[boot] TTBR0_EL1 = ");
    uart::puthex_full(ttbr0);
    uart::puts("\n");

    // Prove translation works: a read+write through a RAM VA round-trips. (If
    // the map were wrong this access would take a translation fault.)
    let probe: u64 = 0x4010_0000; // 1 MiB into RAM, page-aligned, scratch
    unsafe {
        core::ptr::write_volatile(probe as *mut u64, 0xCAFE_F00D_1234_5678);
        let v = core::ptr::read_volatile(probe as *const u64);
        uart::puts("[boot] RAM r/w probe @ 0x40100000 = ");
        uart::puthex_full(v);
        uart::puts(if v == 0xCAFE_F00D_1234_5678 { " OK\n" } else { " FAIL\n" });
    }

    a64println!("[boot] Milestone 2 (MMU) complete — translating.");

    crate::arch::aarch64::bringup_vectors::run();
}
