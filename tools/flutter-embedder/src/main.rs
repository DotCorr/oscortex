//! Flutter engine host embedder — OSCortex Phase 32-B.
//!
//! This is the userspace process that:
//!   1. Registers with the kernel as the Flutter engine host.
//!   2. Opens `libflutter_engine.so` (stub or real) via `sys_dlopen`.
//!   3. Resolves the `FlutterEngineGetProcAddresses` entry point.
//!   4. Builds a `FlutterEngineProcTable` and registers it with the kernel.
//!   5. Creates a compositor surface sized to the physical framebuffer.
//!   6. Calls `FlutterEngineRun` with a software renderer config.
//!   7. Runs the event loop: dispatch vsync / pointer / key / platform-channel.
//!
//! The binary is a `no_std` / `no_main` ELF loaded at 0x400000 and launched
//! by the kernel's process loader (Phase 29+).

#![no_std]
#![no_main]
#![allow(unused)]

mod sys;
mod aot_loader;

use core::arch::asm;
use core::sync::atomic::{AtomicU32, Ordering};
use sys::*;

// ── Entry point ───────────────────────────────────────────────────────────────

/// Bare-metal userspace entry. The kernel sets up RBP=0 and RSP pointing to
/// a fresh user stack before jumping here.
#[unsafe(no_mangle)]
#[unsafe(naked)]
unsafe extern "C" fn _start() -> ! {
    unsafe {
        core::arch::naked_asm!(
            "xor rbp, rbp",
            // SysV AMD64 ABI requires rsp to be 16-byte aligned BEFORE a CALL
            // instruction (so on function entry rsp ≡ 8 mod 16). The kernel
            // does not guarantee this on userspace entry, and any
            // misalignment cascades through every downstream call — notably
            // libflutter_engine.so uses `movaps` against 16-aligned stack
            // slots and #GPs on a misaligned rsp. Force-align here.
            "and rsp, -16",
            // Earliest possible userspace breadcrumb: write one byte to serial
            // before calling into Rust code.
            "mov rax, 1",
            "mov rdi, 1",
            "lea rsi, [rip + 2f]",
            "mov rdx, 1",
            "syscall",
            "call {main}",
            // If main returns, exit(0).
            "mov rax, 60",  // SYS_EXIT
            "xor rdi, rdi",
            "syscall",
            "2:",
            ".ascii \"!\"",
            main = sym main_embedder,
        );
    }
}

// ── Flutter ABI types (repr(C), matches kernel/src/embedder/abi.rs) ──────────

/// Kernel-registered proc-table of resolved Flutter engine fn pointers.
/// All fields are VAs in this process's address space.
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct FlutterEngineProcTable {
    pub run:                   u64,
    pub shutdown:              u64,
    pub send_window_metrics:   u64,
    pub send_pointer_event:    u64,
    pub send_key_event:        u64,
    pub on_vsync:              u64,
    pub schedule_frame:        u64,
    pub send_platform_message: u64,
    pub _reserved:             [u64; 8],
}

/// Engine-facing proc table used with `FlutterEngineGetProcAddresses`.
/// Field order MUST match `FlutterEngineProcTable` in `embedder.h` exactly —
/// the engine fills entries by offset, not by name.
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct FlutterEngineProcTableApi {
    pub struct_size:                            usize,
    pub create_aot_data:                        u64,
    pub collect_aot_data:                       u64,
    pub run:                                    u64,
    pub shutdown:                               u64,
    pub initialize:                             u64,
    pub deinitialize:                           u64,
    pub run_initialized:                        u64,
    pub send_window_metrics:                    u64,
    pub send_pointer_event:                     u64,
    pub send_key_event:                         u64,
    pub send_platform_message:                  u64,
    pub platform_message_create_response_handle: u64,
    pub platform_message_release_response_handle: u64,
    pub send_platform_message_response:         u64,
    pub register_external_texture:              u64,
    pub unregister_external_texture:            u64,
    pub mark_external_texture_frame_available:  u64,
    pub update_semantics_enabled:               u64,
    pub update_accessibility_features:          u64,
    pub dispatch_semantics_action:              u64,
    pub on_vsync:                               u64,
    pub reload_system_fonts:                    u64,
    pub trace_event_duration_begin:             u64,
    pub trace_event_duration_end:               u64,
    pub trace_event_instant:                    u64,
    pub post_render_thread_task:                u64,
    pub get_current_time:                       u64,
    pub run_task:                               u64,
    pub update_locales:                         u64,
    pub runs_aot_compiled_dart_code:            u64,
    pub post_dart_object:                       u64,
    pub notify_low_memory_warning:              u64,
    pub post_callback_on_all_native_threads:    u64,
    pub notify_display_update:                  u64,
    pub schedule_frame:                         u64,
    pub set_next_frame_callback:                u64,
    pub add_view:                               u64,
    pub remove_view:                            u64,
    pub send_view_focus_event:                  u64,
    pub send_semantics_action:                  u64,
}

/// Raw `FlutterProjectArgs` blob written using authoritative offsets from
/// `tools/flutter-engine/flutter_embedder.h` (Flutter 3.29 headers in-tree).
///
/// This avoids field drift when the upstream struct evolves.
#[repr(C)]
#[derive(Clone, Copy)]
struct FlutterProjectArgsRaw {
    bytes: [u8; FLUTTER_PROJECT_ARGS_SIZE],
}

const FLUTTER_PROJECT_ARGS_SIZE: usize = 312;
const OFF_PROJECT_ARGS_STRUCT_SIZE: usize = 0;
const OFF_PROJECT_ARGS_ASSETS_PATH: usize = 8;
const OFF_PROJECT_ARGS_ICU_DATA_PATH: usize = 32;
const OFF_PROJECT_ARGS_PLATFORM_MESSAGE_CALLBACK: usize = 56;
const OFF_PROJECT_ARGS_VSYNC_CALLBACK: usize = 168;
const OFF_PROJECT_ARGS_AOT_DATA: usize = 216;
const OFF_PROJECT_ARGS_DART_ENTRYPOINT_ARGC: usize = 232;
const OFF_PROJECT_ARGS_DART_ENTRYPOINT_ARGV: usize = 240;
const OFF_PROJECT_ARGS_LOG_MESSAGE_CALLBACK: usize = 248;

// Engine command-line switches (offset relative to FlutterProjectArgs).
// From flutter_embedder.h: assets_path(8) + main_path(8) + packages_path(8)
// + icu_data_path(8) starting at offset 8 → ends at 40. Then int + pad + ptr.
const OFF_PROJECT_ARGS_COMMAND_LINE_ARGC: usize = 40;
const OFF_PROJECT_ARGS_COMMAND_LINE_ARGV: usize = 48;

// Legacy Dart AOT snapshot pointer offsets (FlutterProjectArgs).
// Verified against tools/flutter-engine/flutter_embedder.h (Flutter 3.29).
const OFF_PA_VM_SNAPSHOT_DATA:               usize =  64;
const OFF_PA_VM_SNAPSHOT_DATA_SIZE:          usize =  72;
const OFF_PA_VM_SNAPSHOT_INSTRUCTIONS:       usize =  80;
const OFF_PA_VM_SNAPSHOT_INSTRUCTIONS_SIZE:  usize =  88;
const OFF_PA_ISO_SNAPSHOT_DATA:              usize =  96;
const OFF_PA_ISO_SNAPSHOT_DATA_SIZE:         usize = 104;
const OFF_PA_ISO_SNAPSHOT_INSTRUCTIONS:      usize = 112;
const OFF_PA_ISO_SNAPSHOT_INSTRUCTIONS_SIZE: usize = 120;

#[inline]
fn write_u64_at(buf: &mut [u8], off: usize, value: u64) {
    buf[off..off + 8].copy_from_slice(&value.to_le_bytes());
}

#[inline]
fn write_i32_at(buf: &mut [u8], off: usize, value: i32) {
    buf[off..off + 4].copy_from_slice(&value.to_le_bytes());
}

/// Flutter software renderer config.
#[repr(C)]
#[derive(Clone, Copy)]
struct FlutterSoftwareRendererConfig {
    struct_size:             usize,
    surface_present_callback: u64,
}

/// Flutter renderer config union (type=1 → software).
#[repr(C)]
#[derive(Clone, Copy)]
struct FlutterRendererConfig {
    renderer_type: u32,  // 1 = software
    _pad:          u32,
    software:      FlutterSoftwareRendererConfig,
}

/// Flutter window-metrics event.  Sent once after `FlutterEngineRun` and on
/// every resize so the engine knows the physical viewport dimensions.
#[repr(C)]
#[derive(Clone, Copy)]
struct FlutterWindowMetricsEvent {
    struct_size:                usize,
    width:                      usize,
    height:                     usize,
    pixel_ratio:                f64,
    left:                       usize,
    top:                        usize,
    physical_view_inset_top:    f64,
    physical_view_inset_right:  f64,
    physical_view_inset_bottom: f64,
    physical_view_inset_left:   f64,
    display_id:                 u64,
    view_id:                    i64,
}

/// Flutter pointer (mouse / touch / stylus) event.
/// Matches the ABI layout of `FlutterPointerEvent` in `flutter_embedder.h`.
#[repr(C)]
#[derive(Clone, Copy)]
struct FlutterPointerEvent {
    struct_size:    usize, // 8
    phase:          i32,   // 4  (kAdd=0 kHover=1 kDown=2 kMove=3 kUp=4)
    _pad0:          u32,   // 4  (alignment padding)
    timestamp:      u64,   // 8  (microseconds)
    x:              f64,   // 8
    y:              f64,   // 8
    device:         i32,   // 4
    signal_kind:    i32,   // 4  (kNone=0 kScroll=1)
    scroll_delta_x: f64,   // 8
    scroll_delta_y: f64,   // 8
    device_kind:    i32,   // 4  (kMouse=1 kTouch=2)
    _pad1:          u32,   // 4  (alignment padding)
    buttons:        i64,   // 8
    pan_x:          f64,   // 8
    pan_y:          f64,   // 8
    scale:          f64,   // 8
    rotation:       f64,   // 8
    view_id:        i64,   // 8
}

/// Flutter keyboard event.
#[repr(C)]
#[derive(Clone, Copy)]
struct FlutterKeyEvent {
    struct_size:  usize,    // 8
    timestamp:    f64,      // 8  (milliseconds)
    kind:         u32,      // 4  (kDown=0 kUp=1 kRepeat=2)
    _pad:         u32,      // 4  (alignment)
    physical:     u64,      // 8  (HID usage page/id)
    logical:      u64,      // 8  (unicode codepoint / key code)
    character:    u64,      // 8  (*const u8, nullable)
    synthesized:  bool,     // 1
    _tail:        [u8; 7],  // 7  (padding)
}

/// Incoming platform-channel message from the Flutter engine.
#[repr(C)]
#[derive(Clone, Copy)]
struct FlutterPlatformMessage {
    struct_size:     usize, // 8
    channel:         u64,  // 8  *const u8 (NUL-terminated channel name)
    message:         u64,  // 8  *const u8 (raw message bytes)
    message_size:    usize, // 8
    response_handle: u64,  // 8  *mut FlutterPlatformMessageResponseHandle
}

#[repr(C)]
#[derive(Clone, Copy)]
struct FlutterEngineAotDataSource {
    source_type: u32,
    _pad: u32,
    elf_path: u64,
}

const K_FLUTTER_ENGINE_AOT_DATA_SOURCE_TYPE_ELF_PATH: u32 = 0;

// ── Engine function-pointer types ─────────────────────────────────────────────

type GetProcAddressesFn  = unsafe extern "C" fn(table: *mut FlutterEngineProcTableApi) -> i32;
type CreateAotDataFn     = unsafe extern "C" fn(source: *const FlutterEngineAotDataSource, data_out: *mut u64) -> i32;
type RunFn               = unsafe extern "C" fn(
    version: u32,
    config:  *const FlutterRendererConfig,
    args:    *const FlutterProjectArgsRaw,
    ud:      *mut (),
    engine:  *mut u64,
) -> i32;
type SendWindowMetricsFn = unsafe extern "C" fn(engine: u64, evt: *const FlutterWindowMetricsEvent) -> i32;
type SendPointerEventFn  = unsafe extern "C" fn(engine: u64, evts: *const FlutterPointerEvent, n: usize) -> i32;
type SendKeyEventFn      = unsafe extern "C" fn(engine: u64, evt: *const FlutterKeyEvent, cb: u64, ud: u64) -> i32;
type OnVsyncFn           = unsafe extern "C" fn(engine: u64, baton: usize, start_ns: u64, target_ns: u64) -> i32;

// ── Callbacks (called by the engine) ─────────────────────────────────────────

/// Framebuffer surface ID shared between callbacks and the event loop.
static mut SURFACE_ID: u32 = 0;

/// Width of the compositor surface (pixels).
static mut SURFACE_W: u32 = 0;
/// Height of the compositor surface (pixels).
static mut SURFACE_H: u32 = 0;

/// Handle returned by `FlutterEngineRun`; 0 until the engine starts.
static mut ENGINE: u64 = 0;

static PRESENT_TRACE_COUNT: AtomicU32 = AtomicU32::new(0);
static VSYNC_TRACE_COUNT: AtomicU32 = AtomicU32::new(0);

/// Present a rendered frame.  Called by the engine on the raster thread.
/// `allocation` is a row-major RGBA8 buffer; `row_bytes` may be > width*4.
unsafe extern "C" fn present_callback(
    _user_data: *mut (),
    allocation: *const u8,
    row_bytes:  usize,
    height:     usize,
) -> bool {
    unsafe {
        let count = PRESENT_TRACE_COUNT.fetch_add(1, Ordering::Relaxed);
        if count < 3 {
            write(b"[embedder] present_callback\n");
        }
        let surface_id = SURFACE_ID;
        let w = SURFACE_W as usize;
        let pixel_len  = row_bytes * height;
        let pixels = core::slice::from_raw_parts(allocation, pixel_len);
        gpu_submit_strided(surface_id, pixels, row_bytes) >= 0
    }
}

/// Vsync callback — the Flutter engine calls this when it needs a vsync
/// notification (passing an opaque `baton`).  We forward the baton to the
/// kernel; the APIC ISR returns it in the next `EV_VSYNC` event so the event
/// loop can call `FlutterEngineOnVsync(engine, baton, start_ns, target_ns)`.
unsafe extern "C" fn vsync_callback(
    _user_data: *mut (),
    baton:       usize,
) {
    let count = VSYNC_TRACE_COUNT.fetch_add(1, Ordering::Relaxed);
    if count < 3 {
        write(b"[embedder] vsync_callback\n");
    }
    engine_vsync_baton_post(baton as u64);
}

/// Platform-message callback — the engine calls this when Dart sends a
/// platform-channel message.  We parse the `FlutterPlatformMessage` struct
/// and post the payload to the kernel's platform-channel bridge.
unsafe extern "C" fn platform_message_callback(
    _engine:  u64,
    msg_ptr:  *const FlutterPlatformMessage,
) {
    if msg_ptr.is_null() { return; }
    let msg = unsafe { &*msg_ptr };
    if msg.message == 0 || msg.message_size == 0 { return; }
    let channel_slice = if msg.channel != 0 {
        unsafe { cstr_to_slice(msg.channel as *const u8) }
    } else {
        b"unknown"
    };
    let payload = unsafe {
        core::slice::from_raw_parts(msg.message as *const u8, msg.message_size)
    };
    platform_msg_post(channel_slice, payload);
}

/// Log callback — writes to the kernel's serial debug output.
unsafe extern "C" fn log_message_callback(
    tag: *const u8,
    msg: *const u8,
    _ud: *mut (),
) {
    // Write tag + ": " + message to fd 1.
    if !tag.is_null() {
        let tag_slice = unsafe { cstr_to_slice(tag) };
        write(tag_slice);
        write(b": ");
    }
    if !msg.is_null() {
        let msg_slice = unsafe { cstr_to_slice(msg) };
        write(msg_slice);
        write(b"\n");
    }
}

unsafe fn cstr_to_slice<'a>(ptr: *const u8) -> &'a [u8] {
    let mut len = 0usize;
    unsafe {
        while *ptr.add(len) != 0 && len < 4096 { len += 1; }
        core::slice::from_raw_parts(ptr, len)
    }
}

// ── Engine library path constant ──────────────────────────────────────────────

const ENGINE_LIB_PATH: &[u8] = b"/system/lib/libflutter_engine.so";

// 0 = force standalone, 1 = ctor/TLS canary then standalone, 2 = full engine path.
const ENGINE_CANARY_MODE: u8 = 2;

// ── Main embedder logic ───────────────────────────────────────────────────────

extern "C" fn main_embedder() {
    write(b"[embedder] starting\n");

    // 1. Register as the engine host.
    let host_pid = engine_host_register();
    if host_pid < 0 {
        write(b"[embedder] engine_host_register failed\n");
        exit(-1);
    }
    write(b"[embedder] host_register ok\n");

    // 1b. Phase 33-A: request 60 Hz vsync cadence from the compositor.
    vsync_set_hz(60);

    if ENGINE_CANARY_MODE == 0 {
        write(b"[embedder] FORCE_STANDALONE=1 (engine path disabled)\n");
        run_standalone_demo();
        exit(0);
    }

    if ENGINE_CANARY_MODE == 1 {
        write(b"[embedder] ENGINE_CANARY_MODE=1 (ctors-only)\n");
    } else {
        write(b"[embedder] ENGINE_CANARY_MODE=2 (full engine)\n");
    }

    // 2. Open the engine library.
    write(b"[embedder] calling dlopen...\n");
    let handle = dlopen(ENGINE_LIB_PATH, 0);
    if handle <= 0 {
        write(b"[embedder] dlopen failed -- entering standalone demo mode\n");
        run_standalone_demo();
        exit(0);
    }
    let handle = handle as u32;
    write(b"[embedder] dlopen OK\n");
    write(b"[embedder] running C++ ctors\n");

    // 2b. Call DT_INIT and DT_INIT_ARRAY constructors.
    //     Flutter's global heap/mutex state is uninitialized until these run.
    {
        let mut init_fn:  u64 = 0;
        let mut array_va: u64 = 0;
        let mut count:    u64 = 0;
        dl_get_init_array(handle, &mut init_fn, &mut array_va, &mut count);
        if init_fn != 0 {
            write(b"[embedder] calling DT_INIT\n");
            let f: unsafe extern "C" fn() = unsafe { core::mem::transmute(init_fn) };
            unsafe { f(); }
        }
        write(b"[embedder] calling DT_INIT_ARRAY\n");
        for i in 0..count as usize {
            let fn_ptr_addr = array_va + (i * 8) as u64;
            let fn_va = unsafe { core::ptr::read_unaligned(fn_ptr_addr as *const u64) };
            if fn_va != 0 && fn_va != u64::MAX {
                let f: unsafe extern "C" fn() = unsafe { core::mem::transmute(fn_va) };
                unsafe { f(); }
            }
        }
        write(b"[embedder] ctors done\n");
    }

    if ENGINE_CANARY_MODE == 1 {
        write(b"[embedder] canary: ctors completed, entering standalone\n");
        run_standalone_demo();
        exit(0);
    }

    write(b"[embedder] resolving symbols...\n");

    // 3. Resolve FlutterEngineGetProcAddresses.
    let get_procs_va = dlsym(handle, b"FlutterEngineGetProcAddresses");
    write(b"[embedder] dlsym done\n");

    // 4. Build and fill the proc table.
    let mut proctable = FlutterEngineProcTable::default();

    if get_procs_va != 0 {
        write(b"[embedder] using GetProcAddresses path\n");
        let mut api_table = FlutterEngineProcTableApi::default();
        api_table.struct_size = core::mem::size_of::<FlutterEngineProcTableApi>();
        // SAFETY: we resolved this VA from our own address space via dlsym.
        let get_procs: GetProcAddressesFn = unsafe { core::mem::transmute(get_procs_va) };
        let rc = unsafe { get_procs(&mut api_table as *mut FlutterEngineProcTableApi) };
        if rc != 0 {
            write(b"[embedder] GetProcAddresses returned non-zero\n");
        }
        proctable.run                   = api_table.run;
        proctable.shutdown              = api_table.shutdown;
        proctable.send_window_metrics   = api_table.send_window_metrics;
        proctable.send_pointer_event    = api_table.send_pointer_event;
        proctable.send_key_event        = api_table.send_key_event;
        proctable.on_vsync              = api_table.on_vsync;
        proctable.schedule_frame        = api_table.schedule_frame;
        proctable.send_platform_message = api_table.send_platform_message;
    } else {
        write(b"[embedder] manual symbol resolution path\n");
        // Stub path: resolve each symbol manually.
        proctable.run                   = dlsym(handle, b"FlutterEngineRun");
        proctable.shutdown              = dlsym(handle, b"FlutterEngineShutdown");
        proctable.send_window_metrics   = dlsym(handle, b"FlutterEngineSendWindowMetricsEvent");
        proctable.send_pointer_event    = dlsym(handle, b"FlutterEngineSendPointerEvent");
        proctable.send_key_event        = dlsym(handle, b"FlutterEngineSendKeyEvent");
        proctable.on_vsync              = dlsym(handle, b"FlutterEngineOnVsync");
        proctable.schedule_frame        = dlsym(handle, b"FlutterEngineScheduleFrame");
        proctable.send_platform_message = dlsym(handle, b"FlutterEngineSendPlatformMessage");
    }
    write(b"[embedder] proctable ready\n");

    // 5. Register proc table with the kernel.
    engine_proctable_set(
        &proctable as *const FlutterEngineProcTable as u64,
        core::mem::size_of::<FlutterEngineProcTable>(),
    );
    write(b"[embedder] proctable registered\n");

    // 6. Create a compositor surface sized to the framebuffer.
    write(b"[embedder] creating surface...\n");
    let fb_packed = fb_size_packed();
    let fb_w      = ((fb_packed >> 32) & 0xFFFF_FFFF) as u32;
    let fb_h      = (fb_packed & 0xFFFF_FFFF) as u32;
    let (mut w, mut h) = if fb_w > 0 && fb_h > 0 { (fb_w, fb_h) } else { (1280, 720) };

    // Keep the initial software surface modest to avoid early large contiguous
    // kernel-heap allocations during engine bootstrap.
    const MAX_BOOT_SURFACE_PIXELS: u32 = 640 * 360;
    if w.saturating_mul(h) > MAX_BOOT_SURFACE_PIXELS {
        w = 640;
        h = 360;
        write(b"[embedder] surface capped to 640x360 for bootstrap\n");
    }

    let surface_id = surface_create(w, h);
    if surface_id < 0 {
        write(b"[embedder] surface_create failed\n");
        exit(-1);
    }
    let surface_id = surface_id as u32;

    unsafe {
        SURFACE_ID = surface_id;
        SURFACE_W  = w;
        SURFACE_H  = h;
    }

    // 7. Build project args + renderer config.
    let renderer_config = FlutterRendererConfig {
        renderer_type: 1, // software
        _pad: 0,
        software: FlutterSoftwareRendererConfig {
            struct_size:             core::mem::size_of::<FlutterSoftwareRendererConfig>(),
            surface_present_callback: present_callback as *const () as u64,
        },
    };

    let assets_path  = b"/system/flutter/flutter_assets\0";
    let icu_path     = b"/system/flutter/icudtl.dat\0";

    // Engine command-line switches. The first argv item is the executable
    // name (engine skips it). We disable Impeller because the Impeller
    // backend requires GPU contexts we don't provide and otherwise destroys
    // an unfulfilled `std::promise<RuntimeStageBackend>`, throwing
    // `std::future_error` which abort()s under our no-libunwind runtime.
    static ARG0: &[u8] = b"oscortex-flutter\0";
    static ARG1: &[u8] = b"--enable-impeller=false\0";
    static ARG2: &[u8] = b"--enable-software-rendering=true\0";
    static ARG3: &[u8] = b"--disable-vm-service\0";
    #[repr(transparent)]
    struct ArgvPtrs([*const u8; 4]);
    unsafe impl Sync for ArgvPtrs {}
    static ENGINE_ARGV: ArgvPtrs = ArgvPtrs([
        ARG0.as_ptr(),
        ARG1.as_ptr(),
        ARG2.as_ptr(),
        ARG3.as_ptr(),
    ]);

    let mut project_args = FlutterProjectArgsRaw { bytes: [0; FLUTTER_PROJECT_ARGS_SIZE] };
    write_u64_at(&mut project_args.bytes, OFF_PROJECT_ARGS_STRUCT_SIZE, FLUTTER_PROJECT_ARGS_SIZE as u64);
    write_u64_at(&mut project_args.bytes, OFF_PROJECT_ARGS_ASSETS_PATH, assets_path.as_ptr() as u64);
    write_u64_at(&mut project_args.bytes, OFF_PROJECT_ARGS_ICU_DATA_PATH, icu_path.as_ptr() as u64);
    write_i32_at(&mut project_args.bytes, OFF_PROJECT_ARGS_COMMAND_LINE_ARGC, ENGINE_ARGV.0.len() as i32);
    write_u64_at(&mut project_args.bytes, OFF_PROJECT_ARGS_COMMAND_LINE_ARGV, ENGINE_ARGV.0.as_ptr() as u64);
    write_u64_at(
        &mut project_args.bytes,
        OFF_PROJECT_ARGS_PLATFORM_MESSAGE_CALLBACK,
        platform_message_callback as *const () as u64,
    );
    write_u64_at(
        &mut project_args.bytes,
        OFF_PROJECT_ARGS_VSYNC_CALLBACK,
        vsync_callback as *const () as u64,
    );
    write_i32_at(&mut project_args.bytes, OFF_PROJECT_ARGS_DART_ENTRYPOINT_ARGC, 0);
    write_u64_at(&mut project_args.bytes, OFF_PROJECT_ARGS_DART_ENTRYPOINT_ARGV, 0);
    write_u64_at(
        &mut project_args.bytes,
        OFF_PROJECT_ARGS_LOG_MESSAGE_CALLBACK,
        log_message_callback as *const () as u64,
    );

    let aot_snapshot = aot_loader::load_dart_snapshot(b"/system/flutter/app.aot\0");
    // NOTE: our shipped libflutter_engine.so is the linux-x64 (debug/JIT) build.
    // Feeding it AOT snapshot pointers triggers kInvalidArguments ("JIT runtime
    // cannot run a precompiled snapshot"). Force JIT mode regardless.
    let _ = aot_snapshot;
    if false {
        // (legacy AOT branch kept disabled; re-enable if/when we ship the AOT engine)
    } else {
        // JIT mode: our libflutter_engine.so is the linux-x64 (debug/JIT)
        // build that Flutter publishes as `linux-x64-embedder.zip`. The engine
        // does *not* auto-load `vm_snapshot_data` / `isolate_snapshot_data`
        // from `assets_path` — those files contain the VM/isolate snapshots
        // and MUST be provided via the legacy `vm_snapshot_*` /
        // `isolate_snapshot_*` pointer fields in `FlutterProjectArgs`. (The
        // engine *will* still load `kernel_blob.bin` from `assets_path`.)
        //
        // CRITICAL: In this libflutter_engine.so build, the
        // `FlutterProjectArgs.{vm,isolate}_snapshot_data` fields are
        // interpreted as NULL-TERMINATED FILE PATH STRINGS, NOT as binary
        // buffers. The engine's PopulateJITSnapshotMappingCallbacks wraps
        // each pointer in a lambda that calls
        // `fml::FileMapping::CreateReadOnly(std::string{ptr})`.
        //
        // Disassembly evidence (libflutter_engine.so):
        //   0x196c0dd  callq strlen(0x8(this))                # treat as cstr
        //   0x196c1ee  callq fml::FileMapping::CreateReadOnly # open+mmap
        //
        // So we pass the asset PATHS here and let the engine open() and
        // mmap() them via our file-backed mmap path (already proven to
        // work for kernel_blob.bin + icudtl.dat).
        write(b"[embedder] JIT mode: passing snapshot PATHS to engine (it will open+mmap)\n");

        // Static, NUL-terminated path strings — must outlive FlutterEngineRun.
        static VM_PATH:  &[u8] = b"/system/flutter/flutter_assets/vm_snapshot_data\0";
        static ISO_PATH: &[u8] = b"/system/flutter/flutter_assets/isolate_snapshot_data\0";

        let vm_ptr  = VM_PATH.as_ptr()  as u64;
        let iso_ptr = ISO_PATH.as_ptr() as u64;

        write_u64_at(&mut project_args.bytes, OFF_PA_VM_SNAPSHOT_DATA,       vm_ptr);
        write_u64_at(&mut project_args.bytes, OFF_PA_VM_SNAPSHOT_DATA_SIZE,  (VM_PATH.len()  - 1) as u64);
        write_u64_at(&mut project_args.bytes, OFF_PA_ISO_SNAPSHOT_DATA,      iso_ptr);
        write_u64_at(&mut project_args.bytes, OFF_PA_ISO_SNAPSHOT_DATA_SIZE, (ISO_PATH.len() - 1) as u64);
        // JIT instructions are NULL — the kernel_blob carries the executable
        // bytecode, not native machine code.

        let _ = aot_snapshot;
        write(b"[embedder] JIT snapshot paths installed; engine will open them via file mmap\n");
    }

    // 8. Start the Flutter engine if the stub resolved a real `run` pointer.
    let mut engine_out: u64 = 0;

    if proctable.run != 0 {
        write(b"[embedder] calling FlutterEngineRun...\n");
        let run_fn: RunFn = unsafe { core::mem::transmute(proctable.run) };
        let result = unsafe {
            run_fn(
                1, // FLUTTER_ENGINE_VERSION = 1
                &renderer_config as *const FlutterRendererConfig,
                &project_args    as *const FlutterProjectArgsRaw,
                core::ptr::null_mut(),
                &mut engine_out  as *mut u64,
            )
        };
        if result != 0 {
            // Print the result code so we can map it to FlutterEngineResult:
            // 1 = InvalidLibraryVersion, 2 = InvalidArguments, 3 = InternalInconsistency.
            let mut hex = *b"[embedder] FlutterEngineRun returned error 0x________\n";
            let digits = b"0123456789abcdef";
            let r = result as u32;
            for i in 0..8 {
                let nyb = ((r >> ((7 - i) * 4)) & 0xF) as usize;
                hex[44 + i] = digits[nyb];
            }
            write(&hex);
            // Also print engine_out (should still be 0 on failure).
            let mut hx = *b"[embedder] engine_out = 0x________________\n";
            let eo = engine_out;
            for i in 0..16 {
                let nyb = ((eo >> ((15 - i) * 4)) & 0xF) as usize;
                hx[25 + i] = digits[nyb];
            }
            write(&hx);
            // On error, engine_out may be partially set by the engine but the
            // handle is unusable. Force it to 0 so we skip all engine calls
            // below — otherwise we crash dereferencing junk inside the
            // engine. We still enter the event loop so the embedder stays
            // alive (for kernel triage / next-step diagnosis).
            engine_out = 0;
        } else {
            write(b"[embedder] FlutterEngineRun OK\n");
        }
    } else {
        write(b"[embedder] WARNING: proctable.run == 0, engine NOT started\n");
    }

    // 8b. Phase 34-A: store engine handle and send initial window metrics.
    if engine_out != 0 {
        unsafe { ENGINE = engine_out; }
        if proctable.send_window_metrics != 0 {
            let metrics = FlutterWindowMetricsEvent {
                struct_size:                core::mem::size_of::<FlutterWindowMetricsEvent>(),
                width:                      w as usize,
                height:                     h as usize,
                pixel_ratio:                1.0,
                left:                       0,
                top:                        0,
                physical_view_inset_top:    0.0,
                physical_view_inset_right:  0.0,
                physical_view_inset_bottom: 0.0,
                physical_view_inset_left:   0.0,
                display_id:                 0,
                view_id:                    0,
            };
            let f: SendWindowMetricsFn =
                unsafe { core::mem::transmute(proctable.send_window_metrics) };
            unsafe { f(engine_out, &metrics as *const _) };
            write(b"[embedder] window metrics sent\n");
        }
    }

    // 8c. (Snapshot loading happens before FlutterEngineRun — see above.)

    write(b"[embedder] entering event loop\n");

    // 9. Event loop: dispatch vsync / pointer / key / platform-channel events.
    let mut ev = WmEvent::default();
    let mut platform_buf = [0u8; 512];

    loop {
        let r = wm_event_wait(&mut ev, 16 /* ms */);
        if r <= 0 {
            // No event or error — check for pending platform messages.
            let n = platform_msg_recv(&mut platform_buf);
            if n > 0 {
                // Simple echo-reply for testing.
                // Wire format: [seq:u64][ch_len:u16][data_len:u32][ch...][data...]
                if (n as usize) >= 8 + 2 + 4 {
                    let seq = u64::from_le_bytes(platform_buf[0..8].try_into().unwrap_or([0; 8]));
                    // Reply with an empty OK response.
                    platform_msg_reply(seq, b"ok");
                }
            }
            continue;
        }

        match ev.kind {
            EV_VSYNC => {
                // ev.b carries the baton posted by vsync_callback via
                // engine_vsync_baton_post.  Call FlutterEngineOnVsync so the
                // engine can schedule the next frame.
                let baton  = ev.b as usize;
                let engine = unsafe { ENGINE };
                if engine != 0 && proctable.on_vsync != 0 {
                    let now_ns    = rdtsc_ns();
                    let target_ns = now_ns + 16_666_666; // ~60 Hz budget
                    let f: OnVsyncFn =
                        unsafe { core::mem::transmute(proctable.on_vsync) };
                    unsafe { f(engine, baton, now_ns, target_ns) };
                }
            }
            EV_POINTER => {
                let buttons = ev.flags as i64;
                let engine  = unsafe { ENGINE };
                if engine != 0 && proctable.send_pointer_event != 0 {
                    // i16→f64 casts use the __floatsidf compiler-builtin which
                    // LLD failed to relax from GOTPCREL; keep them inside the
                    // engine-valid guard so a stub-engine run can't reach the
                    // unresolved relocation.
                    let x       = ((ev.a >> 48) as i16) as f64;
                    let y       = (((ev.a >> 32) & 0xFFFF) as i16) as f64;
                    let phase = if buttons != 0 { 2i32 } else { 1i32 }; // kDown / kHover
                    let evt = FlutterPointerEvent {
                        struct_size:    core::mem::size_of::<FlutterPointerEvent>(),
                        phase,
                        _pad0:          0,
                        timestamp:      rdtsc_ns() / 1000, // microseconds
                        x,
                        y,
                        device:         0,
                        signal_kind:    0,
                        scroll_delta_x: 0.0,
                        scroll_delta_y: 0.0,
                        device_kind:    1, // kMouse
                        _pad1:          0,
                        buttons,
                        pan_x:          0.0,
                        pan_y:          0.0,
                        scale:          1.0,
                        rotation:       0.0,
                        view_id:        0,
                    };
                    let f: SendPointerEventFn =
                        unsafe { core::mem::transmute(proctable.send_pointer_event) };
                    unsafe { f(engine, &evt as *const _, 1) };
                }
            }
            EV_KEY => {
                let scancode = ev.a as u32;
                let pressed  = (ev.flags & 1) != 0;
                let engine   = unsafe { ENGINE };
                if engine != 0 && proctable.send_key_event != 0 {
                    let evt = FlutterKeyEvent {
                        struct_size:  core::mem::size_of::<FlutterKeyEvent>(),
                        timestamp:    (rdtsc_ns() / 1_000_000) as f64, // milliseconds
                        kind:         if pressed { 0 } else { 1 },     // kDown / kUp
                        _pad:         0,
                        physical:     scancode as u64,
                        logical:      scancode as u64, // identity map (no keymap yet)
                        character:    0,
                        synthesized:  false,
                        _tail:        [0; 7],
                    };
                    let f: SendKeyEventFn =
                        unsafe { core::mem::transmute(proctable.send_key_event) };
                    unsafe { f(engine, &evt as *const _, 0, 0) };
                }
            }
            EV_PLATFORM_MSG => {
                // A native kernel module sent us a platform-channel message.
                let _seq          = ev.a;
                let _channel_hash = ev.flags;
                let n = platform_msg_recv(&mut platform_buf);
                if n > 8 + 2 + 4 {
                    let seq = u64::from_le_bytes(
                        platform_buf[0..8].try_into().unwrap_or([0; 8]),
                    );
                    // Echo-reply OK.
                    platform_msg_reply(seq, b"ok");
                }
            }
            _ => {}
        }
    }
}

// ── Standalone Flutter-style demo renderer ────────────────────────────────────
//
// Runs when libflutter_engine.so is not present.  Demonstrates the full
// compositor surface pipeline:  surface_create → upload_rgba32 → surface_flip.
// Renders an animated Material Design style UI to show what Flutter-on-OSCortex
// will look like once the real engine is ported.

const CARD_W: u32 = 280;
const CARD_H: u32 = 180;

fn run_standalone_demo() {
    write(b"[embedder] standalone demo start\n");
    let mut fb = FbInfo::default();
    if fb_map(&mut fb) < 0 || fb.addr == 0 || fb.width == 0 || fb.height == 0 {
        exit(-1);
    }
    write(b"[embedder] fb_map ok\n");
    let sw = fb.width;
    let sh = fb.height;
    let stride_px = (fb.pitch / 4) as usize;
    let npix = stride_px * sh as usize;
    let buf = unsafe { core::slice::from_raw_parts_mut(fb.addr as *mut u32, npix) };
    write(b"[embedder] framebuffer slice ready\n");

    let mut ev = WmEvent::default();
    let mut frame: u32 = 0;
    let mut heartbeat: u32 = 0;
    let mut hovered_card: i32 = -1;
    let mut mx: i32 = 0;
    let mut my: i32 = 0;

    // Accent colours for 3 demo cards.
    const CARD_COLORS: [u32; 3] = [0xFF3B82F6, 0xFF3FB950, 0xFFC084FC];
    const CARD_LABELS: [&[u8]; 3] = [b"Flutter", b"Dart VM", b"AOT"];

    loop {
        frame = frame.wrapping_add(1);
        heartbeat = heartbeat.wrapping_add(1);
        if frame == 1 {
            write(b"[embedder] first frame enter\n");
        }
        if heartbeat % 10 == 0 {
            write(b".");
        }
        if heartbeat % 200 == 0 {
            write(b"\n");
        }

        // ── Draw background ──────────────────────────────────────────────────
        for p in buf.iter_mut() { *p = 0xFF0D1117; }

        // ── Title ────────────────────────────────────────────────────────────
        let title = b"OSCortex Flutter Embedder";
        let subtitle = b"Engine: loading...  |  Dart VM: standby";
        draw_str_buf(buf, stride_px, sw, sh, (sw / 2).saturating_sub(title.len() as u32 * 4), 24,
                     title, 0xFFE6EDF3, 0xFF0D1117);
        draw_str_buf(buf, stride_px, sw, sh, (sw / 2).saturating_sub(subtitle.len() as u32 * 4), 36,
                     subtitle, 0xFF8B949E, 0xFF0D1117);

        // ── Animated wave underline ──────────────────────────────────────────
        for x in 0..sw {
            let phase = ((frame as i32 * 2 + x as i32) & 31) - 16;
            let dy = phase / 2;
            let y  = 48i32 + dy;
            if y >= 0 && (y as u32) < sh {
                buf[y as usize * stride_px + x as usize] = 0xFF3B82F6;
            }
        }

        // ── 3 Material cards ─────────────────────────────────────────────────
        let total_w = 3 * CARD_W + 2 * 24;
        let cx0 = ((sw.saturating_sub(total_w)) / 2) as i32;
        let cy0 = (sh / 2 - CARD_H / 2) as i32;

        for i in 0..3usize {
            let cx = cx0 + (i as i32) * (CARD_W as i32 + 24);
            let is_hov = hovered_card == i as i32;
            let bg = if is_hov { 0xFF2D3748 } else { 0xFF161B22 };
            // Card shadow (darker rect offset by 4px)
            fill_buf(buf, stride_px, sw, sh, cx + 4, cy0 + 4, CARD_W, CARD_H, 0xFF040810);
            // Card body
            fill_buf(buf, stride_px, sw, sh, cx, cy0, CARD_W, CARD_H, bg);
            // Accent top stripe (animated width)
            let accent_w = if is_hov {
                CARD_W
            } else {
                let t = ((frame as u32).wrapping_mul(3).wrapping_add((i as u32) * 19)) % (CARD_W.max(1));
                let tri = if t < CARD_W / 2 { t } else { CARD_W - t };
                CARD_W / 3 + tri
            };
            fill_buf(buf, stride_px, sw, sh, cx, cy0, accent_w, 4, CARD_COLORS[i]);
            // Card label
            let lbl = CARD_LABELS[i];
            let lx = (cx + (CARD_W as i32 - lbl.len() as i32 * 8) / 2) as u32;
            draw_str_buf(buf, stride_px, sw, sh, lx, (cy0 + 16) as u32, lbl, CARD_COLORS[i], bg);
            // Status text
            let status: &[u8] = match i {
                0 => b"surface API: ready",
                1 => b"AOT loader: ready",
                _ => b"isolate IPC: ready",
            };
            let sx = (cx + (CARD_W as i32 - status.len() as i32 * 8) / 2) as u32;
            draw_str_buf(buf, stride_px, sw, sh, sx, (cy0 + 32) as u32, status, 0xFF8B949E, bg);
            // Animated progress bar inside each card (integer-only).
            let bar_w = CARD_W - 32;
            let t = ((frame as u32).wrapping_mul(5).wrapping_add((i as u32) * 11)) % bar_w.max(1);
            let prog = if t < bar_w / 2 { t * 2 } else { (bar_w - t) * 2 };
            fill_buf(buf, stride_px, sw, sh, cx + 16, cy0 + CARD_H as i32 - 26, bar_w, 8, 0xFF0D1117);
            fill_buf(buf, stride_px, sw, sh, cx + 16, cy0 + CARD_H as i32 - 26, prog.min(bar_w), 8, CARD_COLORS[i]);
        }

        // ── Bottom status bar ─────────────────────────────────────────────────
        fill_buf(buf, stride_px, sw, sh, 0, sh as i32 - 24, sw, 24, 0xFF161B22);
        let status = b"OSCortex Flutter Embedder  |  Phase 61  |  Compositor surface pipeline ACTIVE";
        draw_str_buf(buf, stride_px, sw, sh, 8, sh - 16, status, 0xFF3B82F6, 0xFF161B22);
        let fps_lbl = b"60Hz";
        draw_str_buf(buf, stride_px, sw, sh, sw.saturating_sub(fps_lbl.len() as u32 * 8 + 8), sh - 16,
                     fps_lbl, 0xFF3FB950, 0xFF161B22);

        // ── Input ─────────────────────────────────────────────────────────────
        let r = wm_next_event(&mut ev);
        if r >= 0 {
            match ev.kind {
                EV_POINTER => {
                    mx = (ev.a >> 32) as i32;
                    my = (ev.a & 0xFFFF_FFFF) as i32;
                    let total_w = 3 * CARD_W + 2 * 24;
                    let cx0 = ((sw.saturating_sub(total_w)) / 2) as i32;
                    let cy0 = (sh / 2 - CARD_H / 2) as i32;
                    hovered_card = -1;
                    for i in 0..3i32 {
                        let cx = cx0 + i * (CARD_W as i32 + 24);
                        if mx >= cx && mx < cx + CARD_W as i32
                            && my >= cy0 && my < cy0 + CARD_H as i32 {
                            hovered_card = i;
                        }
                    }
                }
                EV_KEY => {
                    // Escape: exit back to launcher
                    if ev.flags != 0 && ev.a as u8 == 0x01 { return; }
                }
                _ => {}
            }
        }

        // Yield to scheduler between frames.
        sched_yield();
    }
}

// ── Minimal pixel drawing helpers for standalone demo ─────────────────────────

fn fill_buf(buf: &mut [u32], stride_px: usize, sw: u32, sh: u32, x: i32, y: i32, w: u32, h: u32, color: u32) {
    let x0 = x.max(0) as u32;
    let y0 = y.max(0) as u32;
    let x1 = ((x + w as i32) as u32).min(sw);
    let y1 = ((y + h as i32) as u32).min(sh);
    for py in y0..y1 {
        for px in x0..x1 {
            buf[py as usize * stride_px + px as usize] = color;
        }
    }
}

fn draw_char_buf(buf: &mut [u32], stride_px: usize, sw: u32, sh: u32, x: u32, y: u32, ch: u8, fg: u32, bg: u32) {
    let glyph = DEMO_FONT[(ch as usize).min(127)];
    for row in 0u32..8 {
        for col in 0u32..8 {
            let px = x + col;
            let py = y + row;
            if px >= sw || py >= sh { continue; }
            let bit = (glyph[row as usize] >> (7 - col)) & 1;
            buf[py as usize * stride_px + px as usize] = if bit != 0 { fg } else { bg };
        }
    }
}

fn draw_str_buf(buf: &mut [u32], stride_px: usize, sw: u32, sh: u32, x: u32, y: u32, s: &[u8], fg: u32, bg: u32) {
    let mut cx = x;
    for &b in s {
        draw_char_buf(buf, stride_px, sw, sh, cx, y, b, fg, bg);
        cx += 8;
        if cx + 8 > sw { break; }
    }
}

fn approx_sin_deg(deg: i32) -> f32 {
    let rad = (deg as f32) * 3.14159 / 180.0;
    approx_sin(rad)
}
fn approx_cos_deg(deg: i32) -> f32 {
    approx_sin_deg(deg + 90)
}
fn approx_sin(x: f32) -> f32 {
    let pi = 3.14159f32;
    let x = x % (2.0 * pi);
    let x = if x > pi { x - 2.0 * pi } else if x < -pi { x + 2.0 * pi } else { x };
    let sign = if x < 0.0 { -1.0f32 } else { 1.0f32 };
    let x = if x < 0.0 { -x } else { x };
    sign * (16.0 * x * (pi - x)) / (5.0 * pi * pi - 4.0 * x * (pi - x))
}

// Minimal 8×8 font for standalone demo (same as launcher font).
static DEMO_FONT: [[u8; 8]; 128] = make_demo_font();

const fn make_demo_font() -> [[u8; 8]; 128] {
    let mut f = [[0u8; 8]; 128];
    macro_rules! g {
        ($c:literal, $b0:literal,$b1:literal,$b2:literal,$b3:literal,
                     $b4:literal,$b5:literal,$b6:literal,$b7:literal) => {
            f[$c] = [$b0,$b1,$b2,$b3,$b4,$b5,$b6,$b7];
        };
    }
    g!(0x30,0x3C,0x66,0x6E,0x76,0x66,0x66,0x3C,0x00); g!(0x31,0x18,0x38,0x18,0x18,0x18,0x18,0x7E,0x00);
    g!(0x32,0x3C,0x66,0x06,0x1C,0x30,0x66,0x7E,0x00); g!(0x33,0x3C,0x66,0x06,0x1C,0x06,0x66,0x3C,0x00);
    g!(0x34,0x0E,0x1E,0x36,0x66,0x7F,0x06,0x06,0x00); g!(0x35,0x7E,0x60,0x7C,0x06,0x06,0x66,0x3C,0x00);
    g!(0x36,0x1C,0x30,0x60,0x7C,0x66,0x66,0x3C,0x00); g!(0x37,0x7E,0x66,0x0C,0x18,0x18,0x18,0x18,0x00);
    g!(0x38,0x3C,0x66,0x66,0x3C,0x66,0x66,0x3C,0x00); g!(0x39,0x3C,0x66,0x66,0x3E,0x06,0x0C,0x38,0x00);
    g!(0x41,0x18,0x3C,0x66,0x7E,0x66,0x66,0x66,0x00); g!(0x42,0x7C,0x66,0x66,0x7C,0x66,0x66,0x7C,0x00);
    g!(0x43,0x3C,0x66,0x60,0x60,0x60,0x66,0x3C,0x00); g!(0x44,0x78,0x6C,0x66,0x66,0x66,0x6C,0x78,0x00);
    g!(0x45,0x7E,0x60,0x60,0x78,0x60,0x60,0x7E,0x00); g!(0x46,0x7E,0x60,0x60,0x78,0x60,0x60,0x60,0x00);
    g!(0x47,0x3C,0x66,0x60,0x6E,0x66,0x66,0x3C,0x00); g!(0x48,0x66,0x66,0x66,0x7E,0x66,0x66,0x66,0x00);
    g!(0x49,0x3C,0x18,0x18,0x18,0x18,0x18,0x3C,0x00); g!(0x4A,0x1E,0x0C,0x0C,0x0C,0x0C,0x6C,0x38,0x00);
    g!(0x4B,0x66,0x6C,0x78,0x70,0x78,0x6C,0x66,0x00); g!(0x4C,0x60,0x60,0x60,0x60,0x60,0x60,0x7E,0x00);
    g!(0x4D,0x63,0x77,0x7F,0x6B,0x63,0x63,0x63,0x00); g!(0x4E,0x66,0x76,0x7E,0x7E,0x6E,0x66,0x66,0x00);
    g!(0x4F,0x3C,0x66,0x66,0x66,0x66,0x66,0x3C,0x00); g!(0x50,0x7C,0x66,0x66,0x7C,0x60,0x60,0x60,0x00);
    g!(0x51,0x3C,0x66,0x66,0x66,0x6E,0x3C,0x06,0x00); g!(0x52,0x7C,0x66,0x66,0x7C,0x6C,0x66,0x66,0x00);
    g!(0x53,0x3C,0x66,0x60,0x3C,0x06,0x66,0x3C,0x00); g!(0x54,0x7E,0x18,0x18,0x18,0x18,0x18,0x18,0x00);
    g!(0x55,0x66,0x66,0x66,0x66,0x66,0x66,0x3C,0x00); g!(0x56,0x66,0x66,0x66,0x66,0x66,0x3C,0x18,0x00);
    g!(0x57,0x63,0x63,0x63,0x6B,0x7F,0x77,0x63,0x00); g!(0x58,0x66,0x66,0x3C,0x18,0x3C,0x66,0x66,0x00);
    g!(0x59,0x66,0x66,0x66,0x3C,0x18,0x18,0x18,0x00); g!(0x5A,0x7E,0x06,0x0C,0x18,0x30,0x60,0x7E,0x00);
    g!(0x61,0x00,0x00,0x3C,0x06,0x3E,0x66,0x3E,0x00); g!(0x62,0x60,0x60,0x7C,0x66,0x66,0x66,0x7C,0x00);
    g!(0x63,0x00,0x00,0x3C,0x66,0x60,0x66,0x3C,0x00); g!(0x64,0x06,0x06,0x3E,0x66,0x66,0x66,0x3E,0x00);
    g!(0x65,0x00,0x00,0x3C,0x66,0x7E,0x60,0x3C,0x00); g!(0x66,0x1C,0x30,0x30,0x7C,0x30,0x30,0x30,0x00);
    g!(0x67,0x00,0x00,0x3E,0x66,0x66,0x3E,0x06,0x7C); g!(0x68,0x60,0x60,0x7C,0x66,0x66,0x66,0x66,0x00);
    g!(0x69,0x18,0x00,0x38,0x18,0x18,0x18,0x3C,0x00); g!(0x6A,0x0C,0x00,0x1C,0x0C,0x0C,0x0C,0x6C,0x38);
    g!(0x6B,0x60,0x60,0x66,0x6C,0x78,0x6C,0x66,0x00); g!(0x6C,0x38,0x18,0x18,0x18,0x18,0x18,0x3C,0x00);
    g!(0x6D,0x00,0x00,0x63,0x77,0x7F,0x6B,0x63,0x00); g!(0x6E,0x00,0x00,0x7C,0x66,0x66,0x66,0x66,0x00);
    g!(0x6F,0x00,0x00,0x3C,0x66,0x66,0x66,0x3C,0x00); g!(0x70,0x00,0x00,0x7C,0x66,0x66,0x7C,0x60,0x60);
    g!(0x71,0x00,0x00,0x3E,0x66,0x66,0x3E,0x06,0x06); g!(0x72,0x00,0x00,0x6C,0x76,0x60,0x60,0x60,0x00);
    g!(0x73,0x00,0x00,0x3E,0x60,0x3C,0x06,0x7C,0x00); g!(0x74,0x30,0x30,0x7C,0x30,0x30,0x30,0x1C,0x00);
    g!(0x75,0x00,0x00,0x66,0x66,0x66,0x66,0x3E,0x00); g!(0x76,0x00,0x00,0x66,0x66,0x66,0x3C,0x18,0x00);
    g!(0x77,0x00,0x00,0x63,0x6B,0x7F,0x77,0x63,0x00); g!(0x78,0x00,0x00,0x66,0x3C,0x18,0x3C,0x66,0x00);
    g!(0x79,0x00,0x00,0x66,0x66,0x66,0x3E,0x06,0x7C); g!(0x7A,0x00,0x00,0x7E,0x0C,0x18,0x30,0x7E,0x00);
    g!(0x20,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0x00);
    g!(0x2D,0x00,0x00,0x00,0x7E,0x00,0x00,0x00,0x00); g!(0x2E,0x00,0x00,0x00,0x00,0x00,0x18,0x18,0x00);
    g!(0x3A,0x00,0x00,0x18,0x18,0x00,0x18,0x18,0x00); g!(0x7C,0x18,0x18,0x18,0x00,0x18,0x18,0x18,0x00);
    g!(0x2F,0x03,0x06,0x0C,0x18,0x30,0x60,0x40,0x00); g!(0x5F,0x00,0x00,0x00,0x00,0x00,0x00,0x00,0xFF);
    f
}

// ── TSC timestamp helper ──────────────────────────────────────────────────────

/// Read the x86 TSC and return an approximate nanosecond timestamp.
/// Uses a nominal 2 GHz frequency (Phase 33-A keeps the kernel calibrated).
/// Dividing by 2 is fast and accurate enough for Flutter frame scheduling.
#[inline(always)]
fn rdtsc_ns() -> u64 {
    let lo: u32;
    let hi: u32;
    unsafe {
        core::arch::asm!(
            "rdtsc",
            out("eax") lo,
            out("edx") hi,
            options(nomem, nostack, pure),
        );
    }
    let tsc = ((hi as u64) << 32) | (lo as u64);
    tsc / 2 // ~2 GHz → ~0.5 ns/tick ≈ nanoseconds
}

// ── Panic handler ─────────────────────────────────────────────────────────────

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    write(b"[embedder] PANIC\n");
    exit(1)
}

