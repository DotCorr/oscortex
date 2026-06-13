//! aarch64 Limine boot path — UEFI / UTM boot via the Limine protocol.
//!
//! Unlike the bare `-kernel`/DTB path (boot.rs + boot_prod.rs), here the Limine
//! bootloader has already:
//!   * loaded the kernel at its higher-half link address (TTBR1, MMU on),
//!   * set up an HHDM direct map of physical RAM,
//!   * probed a framebuffer,
//!   * answered our request statics (memory map, kernel address, framebuffer).
//!
//! Limine does NOT bring up the interrupt controller, the generic timer, or the
//! EL1 exception vectors — that is still our job, and we reuse the exact same
//! aarch64 code the `-kernel` path uses (gic / timer / vectors).
//!
//! MMU strategy (see `mmu::limine_setup_ttbr0`): we keep Limine's TTBR1 (the
//! running kernel high-half) untouched and install our OWN low-half (TTBR0)
//! identity map — device MMIO + RAM, T0SZ = 25, L1 start. That makes the whole
//! rest of the port (raw-PA MMIO, the per-process TTBR0 paging, enter_user)
//! behave byte-for-byte like the `-kernel` path with HHDM offset 0.
//!
//! This module is compiled only with the `limine-boot` feature (which also
//! selects the higher-half linker script). The `-kernel` path is then absent.

use crate::arch::aarch64::uart;
use crate::arch::aarch64::uart::a64println;

// ── Limine request statics (aarch64) ─────────────────────────────────────────
//
// The bootloader scans the loaded image for these magic numbers and fills in the
// responses before jumping to our entry. We declare the same set x86 uses (minus
// the x2APIC-specific MP flag): base revision, HHDM, memory map, framebuffer,
// kernel address, modules, and an explicit 4-level paging request.
use limine::request::{
    FramebufferRequest, HhdmRequest, ExecutableAddressRequest, MemmapRequest, ModulesRequest,
    PagingModeRequest,
};
use limine::paging::PagingMode;
use limine::BaseRevision;

#[used]
static BASE_REVISION: BaseRevision = BaseRevision::new();

#[used]
static FB_REQUEST: FramebufferRequest = FramebufferRequest::new();
#[used]
static HHDM_REQUEST: HhdmRequest = HhdmRequest::new();
#[used]
static MMAP_REQUEST: MemmapRequest = MemmapRequest::new();
#[used]
static KADDR_REQUEST: ExecutableAddressRequest = ExecutableAddressRequest::new();
#[used]
static MODULES_REQUEST: ModulesRequest = ModulesRequest::new();
// Pin a 4-level (48-bit VA) translation regime so the higher-half kernel mapping
// is deterministic (request exactly AARCH64_4LVL: min == mode == max).
#[used]
static PAGING_REQUEST: PagingModeRequest = PagingModeRequest::new(
    PagingMode::AARCH64_4LVL,
    PagingMode::AARCH64_4LVL,
    PagingMode::AARCH64_4LVL,
);

// ── Boot stack (BSS-resident, 64 KiB) ────────────────────────────────────────
core::arch::global_asm!(
    r#"
.section .bss.limine_boot_stack, "aw", @nobits
.balign 16
LIMINE_BOOT_STACK_BOTTOM:
    .skip 65536
.globl LIMINE_BOOT_STACK_TOP
LIMINE_BOOT_STACK_TOP:
"#
);

extern "C" {
    static LIMINE_BOOT_STACK_TOP: u8;
}

// ── Entry trampoline ─────────────────────────────────────────────────────────
//
// Limine jumps to the ELF entry point (`_limine_aarch64_entry`, first in .text)
// at EL1 (or EL2, which it normally drops for us), MMU on, on a bootloader stack.
// We enable FP/SIMD (Rust codegen emits SIMD even in early bring-up), drop EL2→EL1
// if needed, switch to our own BSS boot stack, then branch into Rust. The kernel
// runs from its higher-half link address, which Limine has mapped via TTBR1.
core::arch::global_asm!(
    r#"
.section .text.limine_entry, "ax"
.globl _limine_aarch64_entry
_limine_aarch64_entry:
    // If we somehow entered at EL2, drop to EL1h with a sane SPSR.
    mrs     x0, CurrentEL
    lsr     x0, x0, #2
    cmp     x0, #2
    b.ne    1f
    mov     x0, #(1 << 31)          // HCR_EL2.RW = 1 → EL1 is AArch64
    msr     hcr_el2, x0
    mov     x0, #0x3c5              // D,A,I,F masked + EL1h
    msr     spsr_el2, x0
    adr     x0, 1f
    msr     elr_el2, x0
    eret
1:
    // Mask interrupts during bring-up.
    msr     daifset, #0xf
    // Enable FP/AdvSIMD at EL1/EL0 (CPACR_EL1.FPEN = 0b11).
    mrs     x0, cpacr_el1
    orr     x0, x0, #(3 << 20)
    msr     cpacr_el1, x0
    isb
    // Force EL1h (SPSel=1): the kernel must run on SP_EL1. Limine enters us at
    // EL1t (SPSel=0), where `sp` aliases SP_EL0 — every kernel stack write then
    // goes through SP_EL0, and enter_user's kernel-stack reset (`mov sp, ...`)
    // clobbers the user SP it just delivered: pid1 entered EL0 with SP = the
    // kernel syscall-stack top and its first push faulted into kernel RAM.
    msr     spsel, #1
    // Switch to our own boot stack (Limine's stack is reclaimable).
    adrp    x0, {boot_stack_top}
    add     x0, x0, :lo12:{boot_stack_top}
    mov     sp, x0
    bl      {entry}
2:  wfe
    b       2b
"#,
    boot_stack_top = sym LIMINE_BOOT_STACK_TOP,
    entry = sym limine_kernel_entry,
);

/// First Rust function on the Limine aarch64 path. Runs at EL1 with Limine's MMU
/// still active (kernel higher-half via TTBR1). Brings the ARM platform up onto
/// our own low-half identity map, then joins the shared kernel.
#[no_mangle]
pub extern "C" fn limine_kernel_entry() -> ! {
    // ── 0. Kernel virt→phys (from Limine) so we can program TTBR0 ────────────
    // The page-table walker addresses tables by PHYSICAL address; our L1 table is
    // a kernel static at a higher-half VA. Limine's executable-address response
    // gives us the (physical_base, virtual_base) pair to translate.
    let kaddr = KADDR_REQUEST
        .response()
        .expect("limine: no executable-address response");
    let phys_base = kaddr.physical_base;
    let virt_base = kaddr.virtual_base;
    let virt_to_phys = move |va: u64| -> u64 { phys_base + (va - virt_base) };

    // ── 1. Install our low-half (TTBR0) identity map: devices + 2 GiB RAM ────
    // Keeps Limine's TTBR1 (the running higher-half kernel) intact. After this,
    // raw-PA device MMIO (UART/GIC) and the per-process TTBR0 paging work exactly
    // as on the `-kernel` path, with HHDM offset 0 in the low half.
    unsafe { crate::arch::aarch64::mmu::limine_setup_ttbr0(2, virt_to_phys) };

    // ── 2. Serial: the debug lifeline (PL011 @ 0x0900_0000, now in our map) ──
    uart::init();
    a64println!();
    a64println!("========================================");
    a64println!("  OSCortex aarch64 — LIMINE boot");
    a64println!("========================================");
    uart::puts("[limine-boot] kernel phys_base = ");
    uart::puthex_full(phys_base);
    uart::puts(" virt_base = ");
    uart::puthex_full(virt_base);
    uart::puts("\n");
    a64println!("[limine-boot] low-half TTBR0 identity map installed (T0SZ=25)");

    // ── 3. EL1 exception vectors (VBAR_EL1): SVC syscalls + faults ───────────
    a64println!("[limine-boot] installing EL1 exception vectors...");
    crate::arch::aarch64::vectors::install();

    // ── 4. GIC: distributor + this core's CPU interface ──────────────────────
    a64println!("[limine-boot] bringing up GIC (distributor + CPU interface)...");
    crate::arch::aarch64::gic::init_distributor();
    crate::arch::aarch64::gic::init_cpu_interface();

    // ── 5. Memory map from Limine → neutral BootMemMap ───────────────────────
    use limine::memmap::MEMMAP_USABLE;
    let mmap = MMAP_REQUEST
        .response()
        .expect("limine: no memory-map response");
    let mut map = crate::mm::BootMemMap::new();
    let mut total: u64 = 0;
    for entry in mmap.entries() {
        if entry.type_ == MEMMAP_USABLE {
            map.push(entry.base, entry.length);
            total += entry.length;
        }
    }
    a64println!(
        "[limine-boot] Limine memory map: {} MiB usable RAM",
        total / (1024 * 1024)
    );

    // HHDM offset is 0 for our LOW-half identity map (VA == PA there). The kernel
    // code/heap live in TTBR1 (higher-half) but every PA the frame allocator /
    // paging layer dereferences (page tables, user frames) is in the low-half
    // identity map, so a zero phys→virt offset is correct for that machinery.
    let hhdm_offset: u64 = 0;

    // ── 6. Hand off to the shared kernel (heap, scheduler, VFS, process, …) ──
    crate::kernel_main_arch_limine(map, hhdm_offset)
}

/// Scan Limine modules for the Flutter engine .so and stash it (mirrors x86).
pub fn scan_modules() {
    if let Some(mods) = MODULES_REQUEST.response() {
        for module in mods.modules() {
            if module.cmdline().contains("libflutter_engine")
                || module.path().contains("libflutter_engine")
            {
                let data = module.data();
                *crate::FLUTTER_ENGINE_BYTES.lock() = Some(data);
                log::info!(
                    "[limine-boot] module: libflutter_engine.so ({} bytes)",
                    data.len()
                );
            }
        }
    } else {
        log::warn!("[limine-boot] no Limine modules response");
    }
}

/// Bring up the framebuffer from Limine's framebuffer response and publish it to
/// the shared fb console (same hook ramfb uses on the `-kernel` path).
pub fn init_framebuffer() {
    let Some(resp) = FB_REQUEST.response() else {
        log::warn!("[limine-boot] no framebuffer response — serial-only");
        return;
    };
    let fbs = resp.framebuffers();
    let Some(fb) = fbs.first() else {
        log::warn!("[limine-boot] framebuffer response but 0 framebuffers");
        return;
    };
    let addr = fb.address() as u64;
    let w = fb.width as u32;
    let h = fb.height as u32;
    let pitch = fb.pitch as u32;
    log::info!(
        "[limine-boot] framebuffer {}x{} bpp={} pitch={} addr={:#x}",
        w, h, fb.bpp, pitch, addr
    );
    // The framebuffer physical address is covered by our low-half identity map
    // (device + RAM, 0..2 GiB) on QEMU/UTM virt, so the raw address is directly
    // writable. Publish it to the shared fb console.
    crate::drivers::fb::init_raw(addr, w, h, pitch);
    // Clean boot screen from the first frame (silences on-screen log spam).
    crate::drivers::bootscreen::init();
    crate::drivers::bootscreen::render();
    log::info!("[limine-boot] framebuffer online: {}x{} via Limine", w, h);
}
