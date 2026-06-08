//! aarch64 production boot path — route the ARM `-kernel` entry into the SAME
//! shared `kernel_main` the x86 (Limine) path uses.
//!
//! The x86 production kernel boots via Limine, which hands the kernel a memory
//! map, an HHDM offset, a framebuffer, and modules. On `qemu-system-aarch64 -M
//! virt` there is no Limine: QEMU loads us via `-kernel` and passes a Flattened
//! Device Tree (DTB) pointer in x0. This module bridges the gap:
//!
//!   1. Bring up the ARM platform primitives the shared kernel assumes are
//!      already live by the time `kernel_main` runs: PL011 serial, the MMU
//!      (identity map of devices + RAM), the EL1 exception vectors, and the
//!      GIC + generic timer.
//!   2. Parse the DTB to discover usable RAM and build the architecture-neutral
//!      [`crate::mm::BootMemMap`].
//!   3. Call the shared `kernel_main` entry (`crate::kernel_main_arch`), which
//!      runs the same subsystem init (heap, scheduler, VFS, process table, …)
//!      and ultimately spawns the init/host process.
//!
//! Limine-only inputs the shared path expects (framebuffer, engine module) are
//! provided as "absent" on ARM for now — those are follow-on work; the goal of
//! this path is to reach userspace.

use crate::arch::aarch64::{fdt, uart};
use crate::arch::aarch64::uart::a64println;

/// Production ARM boot entry. Called from `aarch64_start` with the DTB pointer.
/// Never returns — hands off to the shared `kernel_main`.
pub fn aarch64_kernel_boot(dtb_ptr: u64) -> ! {
    // ── Serial first: the debug lifeline for everything below ───────────────
    uart::init();
    a64println!();
    a64println!("========================================");
    a64println!("  OSCortex aarch64 — PRODUCTION boot");
    a64println!("========================================");
    uart::puts("[arm-boot] DTB pointer (x0) = ");
    uart::puthex_full(dtb_ptr);
    uart::puts("\n");

    // ── MMU: identity-map devices + RAM so the shared kernel can touch memory.
    // The bring-up MMU covers the low device GiB + 2 GiB of RAM from 0x4000_0000,
    // which is enough to reach userspace (QEMU virt default is 2 GiB).
    a64println!("[arm-boot] enabling MMU (identity map: devices + 2 GiB RAM)...");
    unsafe { crate::arch::aarch64::mmu::enable(2) };
    a64println!("[arm-boot] MMU on.");

    // ── EL1 exception vectors (VBAR_EL1): needed for SVC syscalls + faults ──
    a64println!("[arm-boot] installing EL1 exception vectors...");
    crate::arch::aarch64::vectors::install();

    // ── GIC: distributor + CPU interface live so IRQs can be delivered ──────
    a64println!("[arm-boot] bringing up GIC (distributor + CPU interface)...");
    crate::arch::aarch64::gic::init_distributor();
    crate::arch::aarch64::gic::init_cpu_interface();
    // The generic timer (scheduler tick) is armed later by the shared kernel via
    // `arch::apic::init_bsp`; IRQ delivery stays masked until then.

    // ── Discover RAM from the device tree → neutral BootMemMap ──────────────
    let mut map = crate::mm::BootMemMap::new();
    match unsafe { fdt::parse(dtb_ptr) } {
        Some(info) => {
            for i in 0..info.mem_count {
                let r = info.mem[i];
                uart::puts("[arm-boot] DTB /memory region: base=");
                uart::puthex_full(r.base);
                uart::puts(" size=");
                uart::puthex_full(r.size);
                uart::puts("\n");
                map.push(r.base, r.size);
            }
            a64println!(
                "[arm-boot] device tree: {} MiB RAM across {} region(s)",
                info.ram_total() / (1024 * 1024),
                info.mem_count
            );
        }
        None => {
            // No usable DTB — fall back to the QEMU virt default (RAM @ 0x4000_0000).
            // 2 GiB matches `scripts/run-aarch64.sh -m 2G` and the MMU map above.
            a64println!("[arm-boot] WARNING: no valid DTB /memory node; assuming 2 GiB @ 0x40000000");
            map.push(0x4000_0000, 2u64 * 1024 * 1024 * 1024);
        }
    }

    // ── HHDM offset: 0 on aarch64. The bring-up MMU identity-maps RAM, so the
    // direct-map virtual address equals the physical address (VA == PA). The
    // frame allocator / paging layer therefore use a zero phys→virt offset.
    let hhdm_offset: u64 = 0;

    a64println!("[arm-boot] handing off to shared kernel_main ...");
    crate::kernel_main_arch(map, hhdm_offset)
}
