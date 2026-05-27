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
use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};
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
/// sizeof(FlutterSoftwareRendererConfig) == 16 (verified against flutter_embedder.h).
#[repr(C)]
#[derive(Clone, Copy)]
struct FlutterSoftwareRendererConfig {
    struct_size:              usize,  // = 16
    surface_present_callback: u64,
}

/// Flutter renderer config.
///
/// The C layout is:
///   FlutterRendererType type  (4 bytes, padded to 8)
///   union { OpenGL(104), Software(16), Metal(?), Vulkan(?) } (112 bytes, size = max member padded)
///   Total = 120 bytes.
///
/// We use an explicit [u8; 112] padding array to fill the union space correctly,
/// then overlay the software config at the start (offset 0 within the union).
#[repr(C)]
#[derive(Clone, Copy)]
struct FlutterRendererConfig {
    renderer_type: u32,   // kSoftware = 1
    _pad_type:     u32,   // align type field to 8 bytes
    // Union payload: 112 bytes.  Software config is:
    //   [0..8]  struct_size (usize = 16)
    //   [8..16] surface_present_callback (fn ptr)
    // Remaining bytes zeroed.
    union_payload: [u8; 112],
}

impl FlutterRendererConfig {
    fn new_software(present_cb: u64) -> Self {
        let mut cfg = Self {
            renderer_type: 1, // kSoftware
            _pad_type:     0,
            union_payload:  [0u8; 112],
        };
        // Write struct_size at union offset 0.
        let sz: usize = 16; // sizeof(FlutterSoftwareRendererConfig)
        cfg.union_payload[0..8].copy_from_slice(&sz.to_ne_bytes());
        // Write surface_present_callback at union offset 8.
        cfg.union_payload[8..16].copy_from_slice(&present_cb.to_ne_bytes());
        cfg
    }
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

/// Display specification for FlutterEngineNotifyDisplayUpdate.
#[repr(C)]
#[derive(Clone, Copy)]
struct FlutterEngineDisplay {
    pub struct_size:         usize,
    pub display_id:          u64,
    pub single_display:      bool,
    pub _pad0:               [u8; 7],
    pub refresh_rate:        f64,
    pub width:               usize,
    pub height:              usize,
    pub device_pixel_ratio:  f64,
}

type NotifyDisplayUpdateFn = unsafe extern "C" fn(
    engine: u64,
    update_type: u32,
    displays: *const FlutterEngineDisplay,
    display_count: usize,
) -> i32;

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
type ScheduleFrameFn    = unsafe extern "C" fn(engine: u64) -> i32;
type SendPlatformMessageFn = unsafe extern "C" fn(engine: u64, msg: *const FlutterPlatformMessage) -> i32;
type SendPointerEventFn  = unsafe extern "C" fn(engine: u64, evts: *const FlutterPointerEvent, n: usize) -> i32;
type SendKeyEventFn      = unsafe extern "C" fn(engine: u64, evt: *const FlutterKeyEvent, cb: u64, ud: u64) -> i32;
type OnVsyncFn           = unsafe extern "C" fn(engine: u64, baton: usize, start_ns: u64, target_ns: u64) -> i32;

type InitializeFn = unsafe extern "C" fn(
    version: usize,
    config:  *const FlutterRendererConfig,
    args:    *const FlutterProjectArgsRaw,
    ud:      *mut (),
    engine:  *mut u64,
) -> i32;

type RunInitializedFn = unsafe extern "C" fn(
    engine: u64,
) -> i32;

// ── Callbacks (called by the engine) ─────────────────────────────────────────

/// Framebuffer surface ID shared between callbacks and the event loop.
static mut SURFACE_ID: u32 = 0;

/// Width of the compositor surface (pixels).
static mut SURFACE_W: u32 = 0;
/// Height of the compositor surface (pixels).
static mut SURFACE_H: u32 = 0;

/// Handle returned by `FlutterEngineRun`; 0 until the engine starts.
static ENGINE: AtomicU64 = AtomicU64::new(0);
static NOTIFY_DISPLAY_UPDATE: AtomicU64 = AtomicU64::new(0);

// Custom task runner ABI structs
#[repr(C)]
#[derive(Clone, Copy)]
struct FlutterTaskRunnerDescription {
    pub struct_size: usize,
    pub user_data: *mut (),
    pub runs_task_on_current_thread_callback: unsafe extern "C" fn(*mut ()) -> bool,
    pub post_task_callback: unsafe extern "C" fn(FlutterTask, u64, *mut ()),
    pub identifier: usize,
    pub destruction_callback: unsafe extern "C" fn(*mut ()),
}
unsafe impl Sync for FlutterTaskRunnerDescription {}

#[repr(C)]
#[derive(Clone, Copy)]
struct FlutterCustomTaskRunners {
    pub struct_size: usize,
    pub platform_task_runner: *const FlutterTaskRunnerDescription,
    pub render_task_runner: *const FlutterTaskRunnerDescription,
    pub thread_priority_setter: u64,
    pub ui_task_runner: *const FlutterTaskRunnerDescription,
}
unsafe impl Sync for FlutterCustomTaskRunners {}

#[repr(C)]
#[derive(Clone, Copy)]
struct FlutterTask {
    pub runner: u64,
    pub task: u64,
}

#[derive(Clone, Copy)]
struct PlatformTask {
    pub task: FlutterTask,
    pub target_time_ns: u64,
    pub active: bool,
}

static mut PLATFORM_TASKS: [PlatformTask; 256] = [PlatformTask {
    task: FlutterTask { runner: 0, task: 0 },
    target_time_ns: 0,
    active: false,
}; 256];

struct Spinlock {
    locked: core::sync::atomic::AtomicBool,
}

impl Spinlock {
    const fn new() -> Self {
        Self { locked: core::sync::atomic::AtomicBool::new(false) }
    }

    fn lock(&self) -> SpinlockGuard {
        while self.locked.compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed).is_err() {
            core::hint::spin_loop();
        }
        SpinlockGuard { locked: &self.locked }
    }
}

struct SpinlockGuard<'a> {
    locked: &'a core::sync::atomic::AtomicBool,
}

impl Drop for SpinlockGuard<'_> {
    fn drop(&mut self) {
        self.locked.store(false, Ordering::Release);
    }
}

static PLATFORM_TASKS_LOCK: Spinlock = Spinlock::new();
static PLATFORM_THREAD_RSP: AtomicU64 = AtomicU64::new(0);
static RUN_TASK_FN: AtomicU64 = AtomicU64::new(0);
static SEND_PLATFORM_MESSAGE_FN: AtomicU64 = AtomicU64::new(0);

const APPS_REQUEST_CHANNEL: &[u8] = b"oscortex/apps/request";
const APPS_CATALOG_CHANNEL_Z: &[u8] = b"oscortex/apps/catalog\0";
const APPS_REGISTRY_PATH_Z: &[u8] = b"/system/apps/registry.json\0";
const AT_FDCWD: i64 = -100;
const O_RDONLY: u64 = 0;
const EMPTY_APPS_JSON: &[u8] = b"{\"apps\":[]}";

unsafe extern "C" fn runs_task_on_current_thread_callback(_user_data: *mut ()) -> bool {
    let tid = unsafe { syscall0(186) };
    static CALL_COUNT: AtomicU32 = AtomicU32::new(0);
    let count = CALL_COUNT.fetch_add(1, Ordering::Relaxed);
    if count < 10 {
        write_hex_u64(b"[embedder] runs_task_on_current_thread tid=", tid as u64);
    }
    tid == 1
}

unsafe extern "C" fn post_task_callback(
    task: FlutterTask,
    target_time_ns: u64,
    _user_data: *mut (),
) {
    static POST_TASK_LOG: AtomicU32 = AtomicU32::new(0);
    let log_n = POST_TASK_LOG.fetch_add(1, Ordering::Relaxed);
    if log_n < 5 || log_n % 256 == 255 {
        write(b"[embedder] post_task_callback #");
        write_hex_u64(b"n=", log_n as u64);
        write_hex_u64(b"  target=", target_time_ns);
    }
    let _guard = PLATFORM_TASKS_LOCK.lock();
    for slot in unsafe { &mut PLATFORM_TASKS } {
        if !slot.active {
            slot.task = task;
            slot.target_time_ns = target_time_ns;
            slot.active = true;
            return;
        }
    }
    write(b"[embedder] ERROR: PLATFORM_TASKS queue full!\n");
}

unsafe extern "C" fn platform_destruction_callback(_user_data: *mut ()) {}

static PLATFORM_TASK_RUNNER_DESC: FlutterTaskRunnerDescription = FlutterTaskRunnerDescription {
    struct_size: core::mem::size_of::<FlutterTaskRunnerDescription>(),
    user_data: core::ptr::null_mut(),
    runs_task_on_current_thread_callback,
    post_task_callback,
    identifier: 1,
    destruction_callback: platform_destruction_callback,
};

static CUSTOM_TASK_RUNNERS: FlutterCustomTaskRunners = FlutterCustomTaskRunners {
    struct_size: core::mem::size_of::<FlutterCustomTaskRunners>(),
    platform_task_runner: &PLATFORM_TASK_RUNNER_DESC,
    render_task_runner: core::ptr::null(),
    thread_priority_setter: 0,
    ui_task_runner: core::ptr::null(),
};

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
        if count < 16 {
            write(b"[embedder] present_callback\n");
            // Log dimensions and allocation pointer
            write_hex_u64(b"[embedder] present row_bytes=", row_bytes as u64);
            write_hex_u64(b"[embedder] present height=", height as u64);
            write_hex_u64(b"[embedder] present alloc_ptr=", allocation as u64);
            // Sample first pixel and center pixel directly from Flutter's allocation.
            let b0 = *allocation;
            let b1 = *allocation.add(1);
            let b2 = *allocation.add(2);
            let b3 = *allocation.add(3);
            let mid_off = (height / 2) * row_bytes + (SURFACE_W as usize / 2) * 4;
            let c0 = *allocation.add(mid_off);
            let c1 = *allocation.add(mid_off + 1);
            let c2 = *allocation.add(mid_off + 2);
            let c3 = *allocation.add(mid_off + 3);
            let d = b"0123456789abcdef";
            // "[embedder] rawpix#0 p0=00000000 pm=00000000\n"
            let mut msg = *b"[embedder] rawpix#0 p0=00000000 pm=00000000\n";
            msg[18] = b'0' + (count as u8 % 10);
            msg[23] = d[(b0 >> 4) as usize];
            msg[24] = d[(b0 & 0xf) as usize];
            msg[25] = d[(b1 >> 4) as usize];
            msg[26] = d[(b1 & 0xf) as usize];
            msg[27] = d[(b2 >> 4) as usize];
            msg[28] = d[(b2 & 0xf) as usize];
            msg[29] = d[(b3 >> 4) as usize];
            msg[30] = d[(b3 & 0xf) as usize];
            msg[35] = d[(c0 >> 4) as usize];
            msg[36] = d[(c0 & 0xf) as usize];
            msg[37] = d[(c1 >> 4) as usize];
            msg[38] = d[(c1 & 0xf) as usize];
            msg[39] = d[(c2 >> 4) as usize];
            msg[40] = d[(c2 & 0xf) as usize];
            msg[41] = d[(c3 >> 4) as usize];
            msg[42] = d[(c3 & 0xf) as usize];
            write(&msg);

            // Count non-zero bytes (sample every 64th byte) to detect if Flutter
            // rendered anything at all.
            let pixel_len_scan = row_bytes * height;
            let scan = core::slice::from_raw_parts(allocation, pixel_len_scan);
            let mut nnz: u64 = 0;
            let mut i = 0usize;
            while i < scan.len() {
                if scan[i] != 0 { nnz += 1; }
                i += 64;
            }
            write_hex_u64(b"[embedder] nnz_sample=", nnz);

            // PIPELINE TEST: overwrite first 4 rows with opaque red (RGBA).
            // If this red stripe appears on screen, the entire present→blit
            // pipeline is working and the root cause is Flutter rendering blank.
            let alloc_rw = allocation as *mut u8;
            for row in 0..4_usize {
                for col in 0..(row_bytes / 4) {
                    let off = row * row_bytes + col * 4;
                    *alloc_rw.add(off    ) = 0xFF; // R
                    *alloc_rw.add(off + 1) = 0x00; // G
                    *alloc_rw.add(off + 2) = 0x00; // B
                    *alloc_rw.add(off + 3) = 0xFF; // A
                }
            }
            write(b"[embedder] test-red injected into first 4 rows\n");
        }
        let surface_id = SURFACE_ID;
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
    let channel_slice = if msg.channel != 0 {
        unsafe { cstr_to_slice(msg.channel as *const u8) }
    } else {
        b"unknown"
    };

    if channel_slice == APPS_REQUEST_CHANNEL {
        publish_subsystem_apps_catalog();
        return;
    }

    if msg.message == 0 || msg.message_size == 0 {
        return;
    }
    let payload = unsafe { core::slice::from_raw_parts(msg.message as *const u8, msg.message_size) };
    platform_msg_post(channel_slice, payload);
}

fn publish_subsystem_apps_catalog() {
    let mut buf = [0u8; 8192];
    let len = read_small_file(APPS_REGISTRY_PATH_Z, &mut buf);
    let payload = if len > 0 { &buf[..len] } else { EMPTY_APPS_JSON };
    let _ = send_platform_message_direct(APPS_CATALOG_CHANNEL_Z, payload);
}

fn read_small_file(path_z: &[u8], dst: &mut [u8]) -> usize {
    let fd = openat(AT_FDCWD, path_z, O_RDONLY, 0);
    if fd < 0 {
        return 0;
    }

    let mut used = 0usize;
    while used < dst.len() {
        let n = read(fd, &mut dst[used..]);
        if n <= 0 {
            break;
        }
        used = used.saturating_add(n as usize);
    }
    let _ = close(fd);
    used
}

fn send_platform_message_direct(channel_z: &[u8], payload: &[u8]) -> bool {
    let engine = ENGINE.load(Ordering::SeqCst);
    let send_fn_va = SEND_PLATFORM_MESSAGE_FN.load(Ordering::SeqCst);
    if engine == 0 || send_fn_va == 0 {
        return false;
    }

    let msg = FlutterPlatformMessage {
        struct_size: core::mem::size_of::<FlutterPlatformMessage>(),
        channel: channel_z.as_ptr() as u64,
        message: payload.as_ptr() as u64,
        message_size: payload.len(),
        response_handle: 0,
    };
    let send_platform_message: SendPlatformMessageFn = unsafe { core::mem::transmute(send_fn_va) };
    unsafe { send_platform_message(engine, &msg as *const _) == 0 }
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

// Strict engine mode: do not fall back to standalone UI paths.
const STRICT_ENGINE_MODE: bool = true;

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

    if STRICT_ENGINE_MODE {
        write(b"[embedder] strict engine mode enabled\n");
    }

    // 2. Open the engine library.
    write(b"[embedder] calling dlopen...\n");
    let handle = dlopen(ENGINE_LIB_PATH, 0);
    if handle <= 0 {
        write(b"[embedder] dlopen failed for /system/lib/libflutter_engine.so\n");
        exit(-1);
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

    write(b"[embedder] resolving symbols...\n");

    // 3. Resolve FlutterEngineGetProcAddresses.
    let get_procs_va = dlsym(handle, b"FlutterEngineGetProcAddresses");
    write(b"[embedder] dlsym done\n");

    // 4. Build and fill the proc table.
    let mut proctable = FlutterEngineProcTable::default();
    let mut initialize_va = 0u64;
    let mut run_initialized_va = 0u64;
    let mut notify_display_update_va = 0u64;
    let mut is_aot = false;

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
        SEND_PLATFORM_MESSAGE_FN.store(api_table.send_platform_message, Ordering::SeqCst);

        initialize_va                   = api_table.initialize;
        run_initialized_va              = api_table.run_initialized;
        notify_display_update_va        = api_table.notify_display_update;
        RUN_TASK_FN.store(api_table.run_task, Ordering::SeqCst);

        let runs_aot_fn_va = api_table.runs_aot_compiled_dart_code;
        if runs_aot_fn_va != 0 {
            let runs_aot: unsafe extern "C" fn() -> bool = unsafe { core::mem::transmute(runs_aot_fn_va) };
            is_aot = unsafe { runs_aot() };
            if is_aot {
                write(b"[embedder] RunsAOTCompiledDartCode returns TRUE (AOT mode)\n");
            } else {
                write(b"[embedder] RunsAOTCompiledDartCode returns FALSE (JIT mode)\n");
            }
        } else {
            write(b"[embedder] RunsAOTCompiledDartCode is NULL\n");
        }
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
        SEND_PLATFORM_MESSAGE_FN.store(proctable.send_platform_message, Ordering::SeqCst);

        initialize_va                   = dlsym(handle, b"FlutterEngineInitialize");
        run_initialized_va              = dlsym(handle, b"FlutterEngineRunInitialized");
        notify_display_update_va        = dlsym(handle, b"FlutterEngineNotifyDisplayUpdate");
        RUN_TASK_FN.store(dlsym(handle, b"FlutterEngineRunTask"), Ordering::SeqCst);
    }

    if initialize_va == 0 || run_initialized_va == 0 || notify_display_update_va == 0 || RUN_TASK_FN.load(Ordering::SeqCst) == 0 {
        write(b"[embedder] ERROR: FlutterEngineInitialize, RunInitialized, NotifyDisplayUpdate or RunTask not found!\n");
        exit(-1);
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
    let (w, h) = if fb_w > 0 && fb_h > 0 && fb_w <= 16_384 && fb_h <= 16_384 {
        (fb_w, fb_h)
    } else {
        (1280, 720)
    };

    {
        let mut msg = *b"[embedder] viewport fb=________x________ chosen=________x________\n";
        let d = b"0123456789abcdef";
        for i in 0..8 {
            let s0 = ((fb_w >> ((7 - i) * 4)) & 0xF) as usize;
            let s1 = ((fb_h >> ((7 - i) * 4)) & 0xF) as usize;
            let s2 = ((w >> ((7 - i) * 4)) & 0xF) as usize;
            let s3 = ((h >> ((7 - i) * 4)) & 0xF) as usize;
            msg[22 + i] = d[s0];
            msg[31 + i] = d[s1];
            msg[47 + i] = d[s2];
            msg[56 + i] = d[s3];
        }
        write(&msg);
    }

    // Create the surface at full framebuffer resolution so Flutter renders
    // to the entire screen. The kernel compositor handles memory allocation.
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
    // IMPORTANT: FlutterRendererConfig is 120 bytes (C ABI union). Use the
    // new_software() constructor which zero-initialises all 120 bytes and
    // writes the software config fields at the correct offsets.
    let renderer_config = FlutterRendererConfig::new_software(
        present_callback as *const () as u64,
    );

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

    if is_aot {
        // AOT mode: load libapp.so via kernel syscall, parse ELF symbol table,
        // write the 4 Dart snapshot raw pointers into FlutterProjectArgs.
        write(b"[embedder] AOT mode: loading libapp.so via aot_loader\n");
        static LIBAPP_PATH: &[u8] = b"/system/flutter/libapp.so\0";
        match aot_loader::load_dart_snapshot(LIBAPP_PATH) {
            Some(snaps) => {
                aot_loader::log_manifest(&snaps);
                write_u64_at(&mut project_args.bytes, OFF_PA_VM_SNAPSHOT_DATA,              snaps.vm_data);
                write_u64_at(&mut project_args.bytes, OFF_PA_VM_SNAPSHOT_DATA_SIZE,         snaps.vm_data_size);
                write_u64_at(&mut project_args.bytes, OFF_PA_VM_SNAPSHOT_INSTRUCTIONS,      snaps.vm_instr);
                write_u64_at(&mut project_args.bytes, OFF_PA_VM_SNAPSHOT_INSTRUCTIONS_SIZE, snaps.vm_instr_size);
                write_u64_at(&mut project_args.bytes, OFF_PA_ISO_SNAPSHOT_DATA,             snaps.iso_data);
                write_u64_at(&mut project_args.bytes, OFF_PA_ISO_SNAPSHOT_DATA_SIZE,        snaps.iso_data_size);
                write_u64_at(&mut project_args.bytes, OFF_PA_ISO_SNAPSHOT_INSTRUCTIONS,     snaps.iso_instr);
                write_u64_at(&mut project_args.bytes, OFF_PA_ISO_SNAPSHOT_INSTRUCTIONS_SIZE,snaps.iso_instr_size);
                write(b"[embedder] AOT snapshot pointers installed\n");
            }
            None => {
                write(b"[embedder] ERROR: aot_loader::load_dart_snapshot FAILED\n");
                exit(-1);
            }
        }
    } else {
        // JIT mode: pass snapshot asset PATHS to the legacy snapshot pointer
        // fields. This engine build reads those fields as C strings and maps
        // files from disk.
        write(b"[embedder] JIT mode: passing snapshot PATHS to engine (it will open+mmap)\n");

        static VM_PATH:  &[u8] = b"/system/flutter/flutter_assets/vm_snapshot_data\0";
        static ISO_PATH: &[u8] = b"/system/flutter/flutter_assets/isolate_snapshot_data\0";

        write_u64_at(&mut project_args.bytes, OFF_PA_VM_SNAPSHOT_DATA, VM_PATH.as_ptr() as u64);
        write_u64_at(&mut project_args.bytes, OFF_PA_VM_SNAPSHOT_DATA_SIZE, (VM_PATH.len() - 1) as u64);
        write_u64_at(&mut project_args.bytes, OFF_PA_ISO_SNAPSHOT_DATA, ISO_PATH.as_ptr() as u64);
        write_u64_at(&mut project_args.bytes, OFF_PA_ISO_SNAPSHOT_DATA_SIZE, (ISO_PATH.len() - 1) as u64);

        write(b"[embedder] JIT snapshot paths installed; engine will open them via file mmap\n");
    }

    // Save main thread RSP so our task runner callback can detect if we are on the platform thread.
    let mut rsp: u64;
    unsafe {
        asm!("mov {}, rsp", out(reg) rsp);
    }
    PLATFORM_THREAD_RSP.store(rsp, Ordering::SeqCst);

    // Install Custom Task Runners in project args.
    const OFF_PROJECT_ARGS_CUSTOM_TASK_RUNNERS: usize = 184;
    write_u64_at(
        &mut project_args.bytes,
        OFF_PROJECT_ARGS_CUSTOM_TASK_RUNNERS,
        &CUSTOM_TASK_RUNNERS as *const _ as u64,
    );

    // Initialize the MessageLoop for the main thread so that task observers can query it without assertions.
    // NOTE: ensure_initialized_va call removed — hardcoded offset into
    // libflutter_engine.so for fml::MessageLoop::EnsureInitializedForCurrentThread().
    // With render_task_runner=null Flutter manages its own threads' MessageLoops.
    // Calling a stale/wrong offset would silently corrupt engine state.

    // 8. Initialize the engine synchronously on the main thread
    write(b"[embedder] calling FlutterEngineInitialize...\n");
    let mut engine_out: u64 = 0;
    let rc_init = unsafe {
        let initialize_fn: InitializeFn = unsafe { core::mem::transmute(initialize_va) };
        initialize_fn(
            1,
            &renderer_config as *const _,
            &project_args as *const _,
            core::ptr::null_mut(),
            &mut engine_out as *mut u64,
        )
    };

    if rc_init != 0 {
        let mut hex = *b"[embedder] FlutterEngineInitialize FAILED rc=0x________\n";
        let d = b"0123456789abcdef";
        let r = rc_init as u32;
        for i in 0..8 { hex[41 + i] = d[((r >> ((7 - i) * 4)) & 0xF) as usize]; }
        write(&hex);
        exit(-1);
    }
    write(b"[embedder] FlutterEngineInitialize OK\n");
    ENGINE.store(engine_out, Ordering::SeqCst);

    // 9. Run the engine on the main thread (starts the shell)
    write(b"[embedder] calling FlutterEngineRunInitialized...\n");
    let rc_run = unsafe {
        let run_initialized_fn: RunInitializedFn = unsafe { core::mem::transmute(run_initialized_va) };
        run_initialized_fn(engine_out)
    };
    if rc_run != 0 {
        let mut hex = *b"[embedder] FlutterEngineRunInitialized FAILED rc=0x________\n";
        let d = b"0123456789abcdef";
        let r = rc_run as u32;
        for i in 0..8 { hex[45 + i] = d[((r >> ((7 - i) * 4)) & 0xF) as usize]; }
        write(&hex);
        exit(-1);
    }
    write(b"[embedder] FlutterEngineRunInitialized OK\n");

    // 10. Immediately notify display topology (now that shell exists)
    {
        let display = FlutterEngineDisplay {
            struct_size:        core::mem::size_of::<FlutterEngineDisplay>(),
            display_id:         0,
            single_display:     true,
            _pad0:              [0; 7],
            refresh_rate:       60.0,
            width:              w as usize,
            height:             h as usize,
            device_pixel_ratio: 1.0,
        };
        write(b"[embedder] notifying display update...\n");
        let notify_display: NotifyDisplayUpdateFn = unsafe { core::mem::transmute(notify_display_update_va) };
        let rc_disp = unsafe { notify_display(engine_out, 0, &display as *const _, 1) };
        if rc_disp == 0 {
            write(b"[embedder] FlutterEngineNotifyDisplayUpdate OK!\n");
        } else {
            let mut hex = *b"[embedder] FlutterEngineNotifyDisplayUpdate FAILED rc=0x________\n";
            let d = b"0123456789abcdef";
            let r = rc_disp as u32;
            for i in 0..8 { hex[54 + i] = d[((r >> ((7 - i) * 4)) & 0xF) as usize]; }
            write(&hex);
        }
    }

    // 11. Immediately send initial window metrics
    {
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
        write(b"[embedder] sending initial window metrics...\n");
        let send_metrics: SendWindowMetricsFn = unsafe { core::mem::transmute(proctable.send_window_metrics) };
        let rc = unsafe { send_metrics(engine_out, &metrics as *const _) };
        if rc == 0 {
            write(b"[embedder] initial window metrics sent successfully!\n");
            if proctable.schedule_frame != 0 {
                let schedule_frame: ScheduleFrameFn = unsafe { core::mem::transmute(proctable.schedule_frame) };
                let rc_sf = unsafe { schedule_frame(engine_out) };
                if rc_sf == 0 {
                    write(b"[embedder] FlutterEngineScheduleFrame OK\n");
                } else {
                    let mut hex = *b"[embedder] FlutterEngineScheduleFrame FAILED rc=0x________\n";
                    let digits = b"0123456789abcdef";
                    let r = rc_sf as u32;
                    for i in 0..8 {
                        let nyb = ((r >> ((7 - i) * 4)) & 0xF) as usize;
                        hex[51 + i] = digits[nyb];
                    }
                    write(&hex);
                }
            }

            if proctable.send_platform_message != 0 {
                static LIFECYCLE_CH: &[u8] = b"flutter/lifecycle\0";
                static LIFECYCLE_MSG: &[u8] = b"AppLifecycleState.resumed";
                let msg = FlutterPlatformMessage {
                    struct_size: core::mem::size_of::<FlutterPlatformMessage>(),
                    channel: LIFECYCLE_CH.as_ptr() as u64,
                    message: LIFECYCLE_MSG.as_ptr() as u64,
                    message_size: LIFECYCLE_MSG.len(),
                    response_handle: 0,
                };
                let send_platform_message: SendPlatformMessageFn =
                    unsafe { core::mem::transmute(proctable.send_platform_message) };
                let rc_pm = unsafe { send_platform_message(engine_out, &msg as *const _) };
                if rc_pm == 0 {
                    write(b"[embedder] lifecycle resumed message sent\n");
                } else {
                    let mut hex = *b"[embedder] lifecycle message FAILED rc=0x________\n";
                    let digits = b"0123456789abcdef";
                    let r = rc_pm as u32;
                    for i in 0..8 {
                        let nyb = ((r >> ((7 - i) * 4)) & 0xF) as usize;
                        hex[44 + i] = digits[nyb];
                    }
                    write(&hex);
                }
            }

            // Push the subsystem app registry to Flutter shell once at startup.
            publish_subsystem_apps_catalog();
        } else {
            let mut hex = *b"[embedder] send metrics failed, rc = 0x________\n";
            let digits = b"0123456789abcdef";
            let r = rc as u32;
            for i in 0..8 {
                let nyb = ((r >> ((7 - i) * 4)) & 0xF) as usize;
                hex[38 + i] = digits[nyb];
            }
            write(&hex);
        }
    }

    write(b"[embedder] entering event loop\n");

    // 12. Event loop: dispatch vsync / pointer / key / platform-channel events,
    // and run custom task runner platform tasks.
    let mut ev = WmEvent::default();
    let mut platform_buf = [0u8; 512];
    let mut startup_watchdog_stage: u32 = 0;
    let mut startup_watchdog_next_ns: u64 = rdtsc_ns() + 500_000_000;
    // Frame pump: call FlutterEngineScheduleFrame at ~60 fps so Flutter keeps
    // rendering even after it goes idle (static UI or before Dart init completes).
    let mut frame_pump_next_ns: u64 = rdtsc_ns() + 16_666_666;

    loop {
        // Run pending platform tasks
        let now = rdtsc_ns();
        loop {
            let mut task_to_run = None;
            {
                let _guard = PLATFORM_TASKS_LOCK.lock();
                for slot in unsafe { &mut PLATFORM_TASKS } {
                    if slot.active && now >= slot.target_time_ns {
                        task_to_run = Some(slot.task);
                        slot.active = false;
                        break;
                    }
                }
            }
            if let Some(task) = task_to_run {
                static RUN_TASK_LOG: AtomicU32 = AtomicU32::new(0);
                let rn = RUN_TASK_LOG.fetch_add(1, Ordering::Relaxed);
                if rn < 10 || rn % 100 == 0 {
                    write(b"[embedder] run_platform_task #");
                    write_hex_u64(b"n=", rn as u64);
                }
                let run_task_fn: unsafe extern "C" fn(u64, *const FlutterTask) -> i32 =
                    unsafe { core::mem::transmute(RUN_TASK_FN.load(Ordering::SeqCst)) };
                unsafe {
                    run_task_fn(engine_out, &task as *const _);
                }
            } else {
                break;
            }
        }

        // Calculate timeout for next task (wait up to 16ms)
        let now = rdtsc_ns();
        let mut next_task_due = u64::MAX;
        {
            let _guard = PLATFORM_TASKS_LOCK.lock();
            for slot in unsafe { &PLATFORM_TASKS } {
                if slot.active {
                    if slot.target_time_ns < next_task_due {
                        next_task_due = slot.target_time_ns;
                    }
                }
            }
        }
        let timeout_ms = if next_task_due == u64::MAX {
            16
        } else if next_task_due <= now {
            0
        } else {
            let diff_ns = next_task_due - now;
            let diff_ms = diff_ns / 1_000_000;
            diff_ms.min(16).max(1)
        };

        if engine_out != 0
            && now >= startup_watchdog_next_ns
            && startup_watchdog_stage < 6
        {
            if proctable.schedule_frame != 0 {
                let schedule_frame: ScheduleFrameFn =
                    unsafe { core::mem::transmute(proctable.schedule_frame) };
                let rc_sf = unsafe { schedule_frame(engine_out) };
                if rc_sf == 0 {
                    write_hex_u64(
                        b"[embedder] startup watchdog schedule_frame stage=",
                        startup_watchdog_stage as u64,
                    );
                }
            }

            if startup_watchdog_stage >= 2 && proctable.send_platform_message != 0 {
                static LIFECYCLE_CH: &[u8] = b"flutter/lifecycle\0";
                static LIFECYCLE_MSG: &[u8] = b"AppLifecycleState.resumed";
                let msg = FlutterPlatformMessage {
                    struct_size: core::mem::size_of::<FlutterPlatformMessage>(),
                    channel: LIFECYCLE_CH.as_ptr() as u64,
                    message: LIFECYCLE_MSG.as_ptr() as u64,
                    message_size: LIFECYCLE_MSG.len(),
                    response_handle: 0,
                };
                let send_platform_message: SendPlatformMessageFn =
                    unsafe { core::mem::transmute(proctable.send_platform_message) };
                let rc_pm = unsafe { send_platform_message(engine_out, &msg as *const _) };
                if rc_pm == 0 {
                    write(b"[embedder] startup watchdog resent lifecycle resumed\n");
                }
            }

            startup_watchdog_stage += 1;
            startup_watchdog_next_ns = now + 500_000_000;
        }

        let r = wm_event_wait(&mut ev, timeout_ms);
        if r <= 0 {
            // No event or error — check for pending platform messages.
            let n = platform_msg_recv(&mut platform_buf);
            if n > 0 {
                if (n as usize) >= 8 + 2 + 4 {
                    let seq = u64::from_le_bytes(platform_buf[0..8].try_into().unwrap_or([0; 8]));
                    platform_msg_reply(seq, b"ok");
                }
            }
            // Frame pump: keep Flutter rendering at ~60 fps so the Dart UI
            // thread has time to finish init and produce a real frame.
            let now_pump = rdtsc_ns();
            if engine_out != 0
                && proctable.schedule_frame != 0
                && now_pump >= frame_pump_next_ns
            {
                let schedule_frame: ScheduleFrameFn =
                    unsafe { core::mem::transmute(proctable.schedule_frame) };
                let _ = unsafe { schedule_frame(engine_out) };
                frame_pump_next_ns = now_pump + 16_666_666;
            }
            continue;
        }

        match ev.kind {
            EV_VSYNC => {
                let baton = ev.b as usize;
                // Unconditional log for every non-zero baton arrival.
                if baton != 0 {
                    static NZ_BATON_COUNT: AtomicU32 = AtomicU32::new(0);
                    let nzc = NZ_BATON_COUNT.fetch_add(1, Ordering::Relaxed);
                    write_hex_u64(b"[embedder] EV_VSYNC_NZ nzc=", nzc as u64);
                    write_hex_u64(b"  baton=", baton as u64);
                    write_hex_u64(b"  engine_out=", engine_out as u64);
                    write_hex_u64(b"  on_vsync=", proctable.on_vsync as u64);
                }
                // Log every EV_VSYNC receipt for first 80, then every 100th
                {
                    static EV_VSYNC_COUNT: AtomicU32 = AtomicU32::new(0);
                    let evc = EV_VSYNC_COUNT.fetch_add(1, Ordering::Relaxed);
                    if evc < 80 || evc % 100 == 0 {
                        write_hex_u64(b"[embedder] EV_VSYNC #", evc as u64);
                        write_hex_u64(b"  baton=", baton as u64);
                    }
                }
                // When no baton is pending the APIC timer still fires at 60 Hz
                // delivering EV_VSYNC(baton=0).  Use these ticks to pump
                // FlutterEngineScheduleFrame so Flutter posts a real baton and
                // keeps rendering even for static (no-animation) apps.
                if baton == 0
                    && engine_out != 0
                    && proctable.schedule_frame != 0
                {
                    let now_pump = rdtsc_ns();
                    if now_pump >= frame_pump_next_ns {
                        let schedule_frame: ScheduleFrameFn =
                            unsafe { core::mem::transmute(proctable.schedule_frame) };
                        let _ = unsafe { schedule_frame(engine_out) };
                        frame_pump_next_ns = now_pump + 16_666_666;
                    }
                }
                if engine_out != 0 && proctable.on_vsync != 0 && baton != 0 {
                    static VSYNC_SEND_LOG: AtomicU32 = AtomicU32::new(0);
                    let vsync_n = VSYNC_SEND_LOG.fetch_add(1, Ordering::Relaxed);
                    if vsync_n < 20 || vsync_n % 60 == 0 {
                        write(b"[embedder] vsync->engine #");
                        write_hex_u64(b"n=", vsync_n as u64);
                        write_hex_u64(b"  baton=", baton as u64);
                    }
                    let now_ns    = rdtsc_ns();
                    let target_ns = now_ns + 16_666_666;
                    let f: OnVsyncFn =
                        unsafe { core::mem::transmute(proctable.on_vsync) };
                    unsafe { f(engine_out, baton, now_ns, target_ns) };
                    if vsync_n < 20 || vsync_n % 60 == 0 {
                        write_hex_u64(b"[embedder] on_vsync returned vsync_n=", vsync_n as u64);
                    }

                    // Consecutive-no-present watchdog: if 2 vsync→engine calls in a row
                    // produced no new present, force FlutterEngineScheduleFrame to
                    // restart the frame pump.
                    static LAST_PRESENT_AT_VSYNC: AtomicU32 = AtomicU32::new(0);
                    static NO_PRESENT_CONSECUTIVE: AtomicU32 = AtomicU32::new(0);
                    let cur_presents = PRESENT_TRACE_COUNT.load(Ordering::Relaxed);
                    let prev_presents = LAST_PRESENT_AT_VSYNC.swap(cur_presents, Ordering::Relaxed);
                    if cur_presents > prev_presents {
                        NO_PRESENT_CONSECUTIVE.store(0, Ordering::Relaxed);
                    } else if vsync_n >= 2 {
                        let cons = NO_PRESENT_CONSECUTIVE.fetch_add(1, Ordering::Relaxed) + 1;
                        if cons >= 2 && proctable.schedule_frame != 0 {
                            let schedule_frame: ScheduleFrameFn =
                                unsafe { core::mem::transmute(proctable.schedule_frame) };
                            let _ = unsafe { schedule_frame(engine_out) };
                            write_hex_u64(b"[embedder] no-present-kick cons=", cons as u64);
                            NO_PRESENT_CONSECUTIVE.store(0, Ordering::Relaxed);
                        }
                    }

                    let presents = cur_presents;
                    if presents == 0 && (vsync_n == 10 || vsync_n == 30) {
                        if proctable.schedule_frame != 0 {
                            let schedule_frame: ScheduleFrameFn =
                                unsafe { core::mem::transmute(proctable.schedule_frame) };
                            let rc_sf = unsafe { schedule_frame(engine_out) };
                            if rc_sf == 0 {
                                write_hex_u64(b"[embedder] watchdog schedule_frame at vsync=", vsync_n as u64);
                            } else {
                                let mut hex = *b"[embedder] watchdog schedule_frame FAILED rc=0x________\n";
                                let digits = b"0123456789abcdef";
                                let r = rc_sf as u32;
                                for i in 0..8 {
                                    let nyb = ((r >> ((7 - i) * 4)) & 0xF) as usize;
                                    hex[52 + i] = digits[nyb];
                                }
                                write(&hex);
                            }
                        }

                        if vsync_n == 30 && proctable.send_platform_message != 0 {
                            static LIFECYCLE_CH: &[u8] = b"flutter/lifecycle\0";
                            static LIFECYCLE_MSG: &[u8] = b"AppLifecycleState.resumed";
                            let msg = FlutterPlatformMessage {
                                struct_size: core::mem::size_of::<FlutterPlatformMessage>(),
                                channel: LIFECYCLE_CH.as_ptr() as u64,
                                message: LIFECYCLE_MSG.as_ptr() as u64,
                                message_size: LIFECYCLE_MSG.len(),
                                response_handle: 0,
                            };
                            let send_platform_message: SendPlatformMessageFn =
                                unsafe { core::mem::transmute(proctable.send_platform_message) };
                            let rc_pm = unsafe { send_platform_message(engine_out, &msg as *const _) };
                            if rc_pm == 0 {
                                write(b"[embedder] watchdog resent lifecycle resumed\n");
                            }
                        }
                    }

                    // After vsync, check if any platform tasks were queued
                    if vsync_n < 5 {
                        let pending = {
                            let _g = PLATFORM_TASKS_LOCK.lock();
                            let mut n = 0u32;
                            for slot in unsafe { &PLATFORM_TASKS } {
                                if slot.active { n += 1; }
                            }
                            n
                        };
                        write_hex_u64(b"  [vsync] platform_tasks_pending=", pending as u64);
                    }
                } else if baton != 0 {
                    // Baton is non-zero but condition failed — log why.
                    write_hex_u64(b"[embedder] VSYNC_NZ_SKIP engine_out=", engine_out as u64);
                    write_hex_u64(b"  on_vsync=", proctable.on_vsync as u64);
                    write_hex_u64(b"  baton=", baton as u64);
                }
            }
            EV_POINTER => {
                let buttons = ev.flags as i64;
                if engine_out != 0 && proctable.send_pointer_event != 0 {
                    let x       = ((ev.a >> 48) as i16) as f64;
                    let y       = (((ev.a >> 32) & 0xFFFF) as i16) as f64;
                    let phase = if buttons != 0 { 2i32 } else { 1i32 };
                    let evt = FlutterPointerEvent {
                        struct_size:    core::mem::size_of::<FlutterPointerEvent>(),
                        phase,
                        _pad0:          0,
                        timestamp:      rdtsc_ns() / 1000,
                        x,
                        y,
                        device:         0,
                        signal_kind:    0,
                        scroll_delta_x: 0.0,
                        scroll_delta_y: 0.0,
                        device_kind:    1,
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
                    unsafe { f(engine_out, &evt as *const _, 1) };
                }
            }
            EV_KEY => {
                let scancode = ev.a as u32;
                let pressed  = (ev.flags & 1) != 0;
                if engine_out != 0 && proctable.send_key_event != 0 {
                    let evt = FlutterKeyEvent {
                        struct_size:  core::mem::size_of::<FlutterKeyEvent>(),
                        timestamp:    (rdtsc_ns() / 1_000_000) as f64,
                        kind:         if pressed { 0 } else { 1 },
                        _pad:         0,
                        physical:     scancode as u64,
                        logical:      scancode as u64,
                        character:    0,
                        synthesized:  false,
                        _tail:        [0; 7],
                    };
                    let f: SendKeyEventFn =
                        unsafe { core::mem::transmute(proctable.send_key_event) };
                    unsafe { f(engine_out, &evt as *const _, 0, 0) };
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

fn write_hex_u64(label: &[u8], val: u64) {
    write(label);
    let mut buf = *b"0x________________\n";
    let digits = b"0123456789abcdef";
    for i in 0..16 {
        let nyb = ((val >> ((15 - i) * 4)) & 0xF) as usize;
        buf[2 + i] = digits[nyb];
    }
    write(&buf);
}

/// Read the x86 TSC and return an approximate nanosecond timestamp.
/// Uses a nominal 3 GHz frequency (Phase 33-A keeps the kernel calibrated).
/// Must match sys_clock_gettime(CLOCK_MONOTONIC) exactly by adding the Unix epoch
/// offset used by the kernel (1,700,000,000 seconds).
#[inline(always)]
fn rdtsc_ns() -> u64 {
    let lo: u32;
    let hi: u32;
    unsafe {
        core::arch::asm!(
            "rdtsc",
            out("eax") lo,
            out("edx") hi,
            options(nomem, nostack),
        );
    }
    let tsc = ((hi as u64) << 32) | (lo as u64);
    let tsc_ns = tsc / 3;
    tsc_ns.saturating_add(1_700_000_000u64 * 1_000_000_000u64)
}

// ── Panic handler ─────────────────────────────────────────────────────────────

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    write(b"[embedder] PANIC\n");
    exit(1)
}

