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

// ── Limine boot requests (x86_64 boot protocol) ──────────────────────────────
//
// Only the x86_64 production path boots via Limine. The aarch64 path boots via
// `qemu-system-aarch64 -M virt -kernel` (no bootloader) and discovers hardware
// from the device tree — see `arch::aarch64::boot_prod`. The Limine request
// statics are therefore compiled only for x86_64 so the ARM kernel carries no
// dead Limine boot section.
#[cfg(target_arch = "x86_64")]
use limine::request::{
    FramebufferRequest, HhdmRequest, ExecutableAddressRequest, MemmapRequest, MpRequest,
    ModulesRequest,
};
#[cfg(target_arch = "x86_64")]
use limine::BaseRevision;

/// Limine base revision — required for Limine 9+ to recognise this kernel.
#[cfg(target_arch = "x86_64")]
#[used]
static BASE_REVISION: BaseRevision = BaseRevision::new();

#[cfg(target_arch = "x86_64")]
static FB_REQUEST: FramebufferRequest = FramebufferRequest::new();
#[cfg(target_arch = "x86_64")]
static HHDM_REQUEST: HhdmRequest = HhdmRequest::new();
#[cfg(target_arch = "x86_64")]
static MMAP_REQUEST: MemmapRequest = MemmapRequest::new();
#[cfg(target_arch = "x86_64")]
static KADDR_REQUEST: ExecutableAddressRequest = ExecutableAddressRequest::new();
#[cfg(target_arch = "x86_64")]
static SMP_REQUEST: MpRequest = MpRequest::new(limine::mp::MP_FLAG_X2APIC);
#[cfg(target_arch = "x86_64")]
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
#[cfg(target_arch = "x86_64")]
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

    log::warn!(
        "[MM::FrameAlloc] capacity stats at boot: total_usable_frames={} ({} MiB), used_at_boot={} ({} MiB)",
        mm::frame_allocator::frames_total(),
        (mm::frame_allocator::frames_total() * 4096) / (1024 * 1024),
        mm::frame_allocator::frames_used(),
        (mm::frame_allocator::frames_used() * 4096) / (1024 * 1024)
    );

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

    // The architecture-neutral subsystem init + process spawn + idle loop is
    // shared with the aarch64 boot path.
    shared_init_and_run()
}

/// aarch64 production kernel entry.
///
/// Called from `arch::aarch64::boot_prod::aarch64_kernel_boot` after the ARM
/// platform primitives (serial, MMU, EL1 vectors, GIC) are live and the device
/// tree has been parsed into the neutral [`mm::BootMemMap`]. This mirrors the
/// x86 `kernel_main` prologue using arch-provided inputs instead of Limine, then
/// joins the same `shared_init_and_run`.
#[cfg(target_arch = "aarch64")]
pub fn kernel_main_arch(map: mm::BootMemMap, hhdm_offset: u64) -> ! {
    logger::early_print("[OSCORTEX] kernel_main_arch reached (aarch64)\r\n");

    // HHDM offset is 0 on aarch64 (RAM is identity-mapped → VA == PA).
    mm::frame_allocator::set_hhdm_offset(hhdm_offset);

    // ── 1. Architecture early init (FPU/SIMD, syscall/SVC readiness) ──────
    // The interrupt controller + exception vectors are already live (set up in
    // boot_prod), but early_init also enables FP/SIMD and logs syscall setup.
    arch::early_init();
    logger::early_print("[OSCORTEX] arch::early_init done (aarch64)\r\n");

    // ── 2. Memory management from the device-tree region list ────────────
    mm::init_from_regions(&map, hhdm_offset);

    // ── 3. Logger (serial only — no Limine framebuffer on ARM yet) ───────
    logger::init(None);

    log::warn!(
        "[MM::FrameAlloc] capacity at boot: total_usable_frames={} ({} MiB), used_at_boot={} ({} MiB)",
        mm::frame_allocator::frames_total(),
        (mm::frame_allocator::frames_total() * 4096) / (1024 * 1024),
        mm::frame_allocator::frames_used(),
        (mm::frame_allocator::frames_used() * 4096) / (1024 * 1024)
    );
    log::info!("OSCortex kernel {} booting (aarch64)...", env!("CARGO_PKG_VERSION"));

    // ── 3b. Framebuffer via QEMU ramfb (fw_cfg) ──────────────────────────
    // `-M virt` has no Limine framebuffer; configure QEMU's `-device ramfb`
    // through fw_cfg and publish the buffer to the shared fb console so the
    // compositor path works unchanged.  Requires the frame allocator (online
    // after `mm::init_from_regions`).
    match arch::aarch64::ramfb::init() {
        Some((addr, w, h, pitch)) => {
            // Identity-mapped RAM → the physical framebuffer base is directly
            // writable as a virtual address (VA == PA on aarch64).
            drivers::fb::init_raw(addr, w, h, pitch);
            // Paint a visible boot fill so the display is provably driven even
            // before any userspace renders (top band teal, body dark navy).
            drivers::fb::fill_rect(0, 0, w, 48, 0x0014_B8A6);
            drivers::fb::fill_rect(0, 48, w, h - 48, 0x001A_1A2E);
            drivers::fb::write_str("\n  OSCortex aarch64 — ramfb framebuffer up\n");
            // Stop mirroring kernel logs into the framebuffer console: under TCG,
            // glyph rendering into the 1280x800 buffer (64 volatile writes/glyph)
            // is so slow per log line it dwarfs the rest of boot. The serial log
            // is the lifeline; the fb stays driven (the boot fill is visible) and
            // userspace/compositor renders into it normally.
            drivers::fb::disable_fb_logging();
            log::info!("[BOOT] aarch64 framebuffer online: {}x{} via ramfb", w, h);
        }
        None => {
            log::warn!("[BOOT] aarch64 ramfb unavailable (need -device ramfb); serial-only.");
        }
    }

    shared_init_and_run()
}

/// aarch64 Limine kernel entry (UEFI / UTM boot).
///
/// Called from `arch::aarch64::boot_limine::limine_kernel_entry` after the Limine
/// path has installed our low-half TTBR0 identity map, brought up serial, the EL1
/// vectors, and the GIC. Mirrors `kernel_main_arch` but takes its memory map from
/// the Limine memory map (already translated into the neutral `BootMemMap` by the
/// caller) and its framebuffer + engine module from Limine instead of ramfb.
#[cfg(all(target_arch = "aarch64", feature = "limine-boot"))]
pub fn kernel_main_arch_limine(map: mm::BootMemMap, hhdm_offset: u64) -> ! {
    logger::early_print("[OSCORTEX] kernel_main_arch_limine reached (aarch64/limine)\r\n");

    mm::frame_allocator::set_hhdm_offset(hhdm_offset);

    // ── 1. Architecture early init (FPU/SIMD, SVC readiness) ─────────────────
    // Vectors + GIC are already live (set up in boot_limine).
    arch::early_init();
    logger::early_print("[OSCORTEX] arch::early_init done (aarch64/limine)\r\n");

    // ── 2. Memory management from the Limine memory map ──────────────────────
    mm::init_from_regions(&map, hhdm_offset);

    // ── 3. Logger (serial only here; the Limine framebuffer comes up next) ──
    logger::init(None);

    log::warn!(
        "[MM::FrameAlloc] capacity at boot: total_usable_frames={} ({} MiB), used_at_boot={} ({} MiB)",
        mm::frame_allocator::frames_total(),
        (mm::frame_allocator::frames_total() * 4096) / (1024 * 1024),
        mm::frame_allocator::frames_used(),
        (mm::frame_allocator::frames_used() * 4096) / (1024 * 1024)
    );
    log::info!("OSCortex kernel {} booting (aarch64/limine)...", env!("CARGO_PKG_VERSION"));

    // ── 3b. Framebuffer + engine module via Limine ───────────────────────────
    arch::aarch64::boot_limine::init_framebuffer();
    arch::aarch64::boot_limine::scan_modules();

    shared_init_and_run()
}

/// Architecture-neutral subsystem init, init-process spawn, and idle loop.
///
/// Runs after each arch's boot prologue has brought up serial, the memory
/// manager, and the logger. This is the part of `kernel_main` that does not
/// touch any boot protocol directly.
fn shared_init_and_run() -> ! {
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

    // aarch64: bring up virtio-input (pointer) now that the WM queue + GIC are
    // live. On `-M virt` there is no PS/2; pointer input arrives via virtio-mmio.
    #[cfg(target_arch = "aarch64")]
    crate::drivers::virtio_input::init();

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
    #[cfg(target_arch = "x86_64")]
    arch::smp_init(SMP_REQUEST.response());
    #[cfg(not(target_arch = "x86_64"))]
    arch::smp_init(None);

    // ── 9b. Spawn init process from initramfs ────────────────────────────
    match fs::lookup("/init") {
        Some(elf_bytes) => {
            // The init/host process is the Flutter shell host on BOTH arches now:
            // seed rdi = HOST_MODE_SHELL so `_start`/`main_embedder` boots the
            // shell host (registers as the engine host, dlopen's the engine, runs
            // the shell). When the staged `/init` is the preemption self-test
            // (no engine present), HOST_MODE_SHELL is ignored by that program.
            let first_rdi: u64 = crate::app_registry::HOST_MODE_SHELL;
            let bootstrap = process::SpawnBootstrap {
                rdi: first_rdi,
                rsi: 0,
                rdx: 0,
                parent_pid: 0,
            };
            match process::spawn_with_bootstrap(elf_bytes, "init", bootstrap) {
                Ok(pid) => {
                    crate::wm::set_focus_pid(pid);
                    log::info!("[INIT] Spawned shell host as PID {}", pid);

                    // x86: defer the user launch to a scheduler kernel task; the
                    // APIC-timer-driven cooperative scheduler switches into it.
                    #[cfg(target_arch = "x86_64")]
                    process::schedule_user_launch(pid);

                    // aarch64: the generic-timer ISR drives the SAME shared
                    // cooperative-scheduler hand-off the x86 APIC timer uses.
                    #[cfg(target_arch = "aarch64")]
                    {
                        // Detect whether the staged `/init` is the real Flutter
                        // shell host (engine .so present) or the bring-up
                        // preemption self-test (engine absent). The host runs as a
                        // single foreground EL0 process and spawns its own engine
                        // threads via SYS_THREAD_CREATE; the self-test needs a
                        // SECOND compute-loop thread to prove timer preemption.
                        let engine_present =
                            fs::lookup("/system/lib/libflutter_engine.so").is_some();

                        if engine_present {
                            log::info!(
                                "[INIT] aarch64: Flutter engine present — entering shell host PID {} (single foreground)",
                                pid
                            );
                        } else {
                            let bootstrap_b = process::SpawnBootstrap {
                                rdi: 1, // selects message B in the self-test program
                                rsi: 0,
                                rdx: 0,
                                parent_pid: 0,
                            };
                            match process::spawn_with_bootstrap(elf_bytes, "init-b", bootstrap_b) {
                                Ok(pid_b) => log::info!(
                                    "[INIT] aarch64: spawned 2nd EL0 thread PID {} (preemption test)",
                                    pid_b
                                ),
                                Err(e) => log::warn!("[INIT] aarch64: 2nd thread spawn failed: {}", e),
                            }
                            log::info!(
                                "[INIT] aarch64: entering PID {} at EL0; timer will preempt between threads",
                                pid
                            );
                        }
                        // Arm the generic-timer scheduler tick now (kernel fully
                        // initialised) so preemption begins the moment we drop to EL0.
                        // The engine NEEDS preemption: pid 1 busy-spins in EL0 after
                        // creating its UI/raster/IO worker threads (it waits on a
                        // userspace flag the workers set), and with the nested
                        // immediate-child-enter disabled on arm those workers only
                        // get the CPU when a timer tick preempts pid 1.
                        let _ = engine_present;
                        // Clear the pending-init guard: aarch64 launches the init
                        // process directly (below) instead of via the x86
                        // user_launch_task, which is the only other site that clears
                        // it. Left set, next_runnable_pid (the wrapper) returns None
                        // forever → sys_exit halts the whole engine when any thread
                        // exits. See process::mark_init_launched.
                        process::mark_init_launched();
                        arch::aarch64::apic::start_scheduler_tick();
                        process::enter_user_by_pid_noreturn(pid);
                    }
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

