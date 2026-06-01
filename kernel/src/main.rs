//! OSCortex Kernel — entry point
//!
//! This is the AI-first kernel for OSCortex. It boots via the Limine protocol,
//! initialises all subsystems, then hands control to the AI Cortex runtime which
//! manages drivers, healing, and context growth at runtime.
//!
//! Ring model:
//!   Ring 0  — Kernel Core + AI Cortex (this crate)
//!   Ring 0' — Hot-loadable driver runtime (WASM sandbox, still Ring 0 memory but
//!             isolated by software fault isolation)
//!   Ring 3  — Userspace processes
//!
//! The AI Cortex lives inside Ring 0 but in a strictly bounded memory region
//! (the Cortex Arena). It communicates with the rest of the kernel through the
//! Cortex Kernel Interface (CKI), never touching raw hardware directly.

#![no_std]
#![no_main]
#![cfg_attr(target_arch = "x86_64", feature(abi_x86_interrupt))]
#![cfg_attr(target_arch = "x86_64", feature(naked_functions))]
#![feature(alloc_error_handler)]
#![feature(core_intrinsics)]
#![allow(dead_code)]

extern crate alloc;

// ── Module declarations ──────────────────────────────────────────────────────
mod arch;
mod app_registry;
mod app_store;
mod compositor;
mod cortex;
mod drivers;
mod embedder;
mod fs;
mod ipc;
mod isolate;
mod isolate_msg;
mod logger;
mod mm;
mod net;
mod panic;
mod pkg;
mod port_ns;
mod process;
mod sched;
mod security;
mod syscall;
mod wm;
mod platform_channel;

use core::sync::atomic::{AtomicBool, Ordering};

/// Global kernel initialisation lock — prevents double-init on SMP wake-up.
pub static KERNEL_INIT_DONE: AtomicBool = AtomicBool::new(false);

// ── Limine boot requests ─────────────────────────────────────────────────────
use limine::request::{
    FramebufferRequest, HhdmRequest, ExecutableAddressRequest, MemmapRequest, MpRequest,
    ModulesRequest,
};
use limine::BaseRevision;

/// Limine base revision — required for Limine 9+ to recognise this kernel.
#[used]
static BASE_REVISION: BaseRevision = BaseRevision::new();

static FB_REQUEST: FramebufferRequest = FramebufferRequest::new();
static HHDM_REQUEST: HhdmRequest = HhdmRequest::new();
static MMAP_REQUEST: MemmapRequest = MemmapRequest::new();
static KADDR_REQUEST: ExecutableAddressRequest = ExecutableAddressRequest::new();
static SMP_REQUEST: MpRequest = MpRequest::new(limine::mp::MP_FLAG_X2APIC);
static MODULES_REQUEST: ModulesRequest = ModulesRequest::new();

/// Slice of libflutter_engine.so as provided by Limine, stored once at boot.
/// The pointer is valid for the entire kernel lifetime (Limine maps modules
/// as reclaimable bootloader memory which we never reclaim).
pub static FLUTTER_ENGINE_BYTES: spin::Mutex<Option<&'static [u8]>> =
    spin::Mutex::new(None);

// ── Kernel entry ─────────────────────────────────────────────────────────────

/// Kernel main entry — called by Limine for the BSP (Bootstrap Processor).
///
/// Execution order:
///   1. Architecture early init (GDT, IDT, CPU features)
///   2. Memory management (frame allocator, virtual memory, kernel heap)
///   3. Logger (early serial + framebuffer)
///   4. Driver registry (empty, ready for AI-injection)
///   5. Scheduler init
///   6. Security / capability system
///   7. AI Cortex init  ← the magic
///   8. SMP wake-up (other cores join here after KERNEL_INIT_DONE)
///   9. Idle loop (Cortex takes over scheduling from here)
#[no_mangle]
pub extern "C" fn kernel_main() -> ! {
    // ── 0. Early serial — must be first so any crash below is visible ─────
    logger::early_print("\r\n[OSCORTEX] kernel_main reached\r\n");

    // Set HHDM offset early so arch code (APIC etc.) can do phys→virt mapping
    // before full mm::init() runs.
    let hhdm_early = HHDM_REQUEST
        .response()
        .expect("bootloader did not provide HHDM offset");
    mm::frame_allocator::set_hhdm_offset(hhdm_early.offset);
    {
        // Debug: print HHDM offset.
        use core::fmt::Write;
        let mut buf = [0u8; 32];
        let mut cursor = 0usize;
        let val = hhdm_early.offset;
        // Format as hex manually (no alloc available).
        let hex = b"0123456789abcdef";
        buf[cursor] = b'0'; cursor += 1;
        buf[cursor] = b'x'; cursor += 1;
        for i in (0..16).rev() {
            buf[cursor] = hex[((val >> (i * 4)) & 0xf) as usize];
            cursor += 1;
        }
        buf[cursor] = b'\r'; cursor += 1;
        buf[cursor] = b'\n'; cursor += 1;
        crate::logger::early_print("[OSCORTEX] HHDM offset: ");
        if let Ok(s) = core::str::from_utf8(&buf[..cursor]) {
            crate::logger::early_print(s);
        }
    }

    // ── 1. Architecture early init ────────────────────────────────────────
    arch::early_init();
    logger::early_print("[OSCORTEX] arch::early_init done\r\n");

    // ── 2. Memory management ──────────────────────────────────────────────
    let mmap = MMAP_REQUEST
        .response()
        .expect("bootloader did not provide memory map");
    let hhdm = HHDM_REQUEST
        .response()
        .expect("bootloader did not provide HHDM offset");
    let kaddr = KADDR_REQUEST
        .response()
        .expect("bootloader did not provide kernel address");

    mm::init(mmap, hhdm.offset, kaddr);

    // Finish xAPIC MMIO init now that the page allocator is available.
    // (No-op when x2APIC was already set up in early_init.)
    crate::arch::apic::finish_xapic_init();

    // ── 3. Logger ─────────────────────────────────────────────────────────
    let fb_response = FB_REQUEST.response();
    logger::init(fb_response);

    // Report framebuffer state on serial so we can verify it was found.
    if let Some(ref fbr) = fb_response {
        let fbs = fbr.framebuffers();
        if !fbs.is_empty() {
            let fb = fbs[0];
            log::info!("Framebuffer: {}x{} bpp={} pitch={} addr={:#x}",
                fb.width, fb.height, fb.bpp, fb.pitch, fb.address() as u64);
        } else {
            log::warn!("Framebuffer response present but 0 framebuffers");
        }
    } else {
        log::warn!("No framebuffer response from bootloader");
    }

    log::info!("OSCortex kernel {} booting...", env!("CARGO_PKG_VERSION"));
    log::info!("AI Cortex: ENABLED");

    // ── 3a. Scan Limine modules (Flutter engine .so) ─────────────────────
    if let Some(mods_resp) = MODULES_REQUEST.response() {
        for module in mods_resp.modules() {
            let cmdline = module.cmdline();
            if cmdline.contains("libflutter_engine") || module.path().contains("libflutter_engine") {
                let data = module.data();
                *FLUTTER_ENGINE_BYTES.lock() = Some(data);
                log::info!("[BOOT] Limine module: libflutter_engine.so ({} bytes)", data.len());
            }
        }
    } else {
        log::warn!("[BOOT] No Limine modules response — libflutter_engine.so unavailable");
    }

    // ── 4. Platform drivers (input probe) ───────────────────────────────
    let qemu_like = arch::cpu::is_qemu_like_hypervisor();
    drivers::platform::init_early(qemu_like);

    // ── 4b. Compositor scaffold (M13) ────────────────────────────────────
    compositor::init();

    // ── 5. Scheduler ─────────────────────────────────────────────────────
    sched::init();

    // ── 6. Security / capabilities ────────────────────────────────────────
    security::init();

    // ── 7. IPC subsystem ─────────────────────────────────────────────────
    ipc::init();

    // ── 7a. Window-manager event bridge ──────────────────────────────────
    wm::init();

    // ── 7b. Virtual filesystem (initramfs) ───────────────────────────────
    fs::init();

    // ── 7c. Block, serial, and networking ─────────────────────────────────
    drivers::platform::init_block_and_net();

    // ── 7d. On-demand package delivery ────────────────────────────────
    pkg::init();

    // ── 8. AI Cortex ─────────────────────────────────────────────────────
    // The Cortex boots last so every kernel subsystem is available to it.
    cortex::init();

    // ── 9. Signal APs + bring SMP online ────────────────────────────────
    KERNEL_INIT_DONE.store(true, Ordering::Release);
    arch::smp_init(SMP_REQUEST.response());

    // ── 9b. Spawn init process from initramfs ────────────────────────────
    match fs::lookup("/init") {
        Some(elf_bytes) => {
            let bootstrap = process::SpawnBootstrap {
                rdi: crate::app_registry::HOST_MODE_SHELL,
                rsi: 0,
                rdx: 0,
                parent_pid: 0,
            };
            match process::spawn_with_bootstrap(elf_bytes, "init", bootstrap) {
                Ok(pid) => {
                    process::schedule_user_launch(pid);
                    crate::wm::set_focus_pid(pid);
                    log::info!("[INIT] Spawned shell host as PID {}", pid);
                }
                Err(e)  => log::warn!("[INIT] Failed to spawn /init: {}", e),
            }
        }
        None => log::warn!("[INIT] /init not found in initramfs"),
    }

    log::info!("OSCortex kernel init complete — entering Cortex-managed loop");

    // ── 10. Cortex-managed idle loop ──────────────────────────────────────
    cortex::run()
}

