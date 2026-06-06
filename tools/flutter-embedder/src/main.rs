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

mod aot_loader;
mod sys;

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
    pub run: u64,
    pub shutdown: u64,
    pub send_window_metrics: u64,
    pub send_pointer_event: u64,
    pub send_key_event: u64,
    pub on_vsync: u64,
    pub schedule_frame: u64,
    pub send_platform_message: u64,
    pub _reserved: [u64; 8],
}

/// Engine-facing proc table used with `FlutterEngineGetProcAddresses`.
/// Field order MUST match `FlutterEngineProcTable` in `embedder.h` exactly —
/// the engine fills entries by offset, not by name.
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct FlutterEngineProcTableApi {
    pub struct_size: usize,
    pub create_aot_data: u64,
    pub collect_aot_data: u64,
    pub run: u64,
    pub shutdown: u64,
    pub initialize: u64,
    pub deinitialize: u64,
    pub run_initialized: u64,
    pub send_window_metrics: u64,
    pub send_pointer_event: u64,
    pub send_key_event: u64,
    pub send_platform_message: u64,
    pub platform_message_create_response_handle: u64,
    pub platform_message_release_response_handle: u64,
    pub send_platform_message_response: u64,
    pub register_external_texture: u64,
    pub unregister_external_texture: u64,
    pub mark_external_texture_frame_available: u64,
    pub update_semantics_enabled: u64,
    pub update_accessibility_features: u64,
    pub dispatch_semantics_action: u64,
    pub on_vsync: u64,
    pub reload_system_fonts: u64,
    pub trace_event_duration_begin: u64,
    pub trace_event_duration_end: u64,
    pub trace_event_instant: u64,
    pub post_render_thread_task: u64,
    pub get_current_time: u64,
    pub run_task: u64,
    pub update_locales: u64,
    pub runs_aot_compiled_dart_code: u64,
    pub post_dart_object: u64,
    pub notify_low_memory_warning: u64,
    pub post_callback_on_all_native_threads: u64,
    pub notify_display_update: u64,
    pub schedule_frame: u64,
    pub set_next_frame_callback: u64,
    pub add_view: u64,
    pub remove_view: u64,
    pub send_view_focus_event: u64,
    pub send_semantics_action: u64,
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
const OFF_PROJECT_ARGS_DART_OLD_GEN_HEAP_SIZE: usize = 208;
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
const OFF_PA_VM_SNAPSHOT_DATA: usize = 64;
const OFF_PA_VM_SNAPSHOT_DATA_SIZE: usize = 72;
const OFF_PA_VM_SNAPSHOT_INSTRUCTIONS: usize = 80;
const OFF_PA_VM_SNAPSHOT_INSTRUCTIONS_SIZE: usize = 88;
const OFF_PA_ISO_SNAPSHOT_DATA: usize = 96;
const OFF_PA_ISO_SNAPSHOT_DATA_SIZE: usize = 104;
const OFF_PA_ISO_SNAPSHOT_INSTRUCTIONS: usize = 112;
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
    struct_size: usize, // = 16
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
    renderer_type: u32, // kSoftware = 1
    _pad_type: u32,     // align type field to 8 bytes
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
            _pad_type: 0,
            union_payload: [0u8; 112],
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
    struct_size: usize,
    width: usize,
    height: usize,
    pixel_ratio: f64,
    left: usize,
    top: usize,
    physical_view_inset_top: f64,
    physical_view_inset_right: f64,
    physical_view_inset_bottom: f64,
    physical_view_inset_left: f64,
    display_id: u64,
    view_id: i64,
}

/// Flutter pointer (mouse / touch / stylus) event.
/// Matches the ABI layout of `FlutterPointerEvent` in `flutter_embedder.h`.
#[repr(C)]
#[derive(Clone, Copy)]
struct FlutterPointerEvent {
    struct_size: usize,  // 8
    phase: i32,          // 4  (kCancel=0 kUp=1 kDown=2 kMove=3 kAdd=4 kRemove=5 kHover=6)
    _pad0: u32,          // 4  (alignment padding)
    timestamp: u64,      // 8  (microseconds)
    x: f64,              // 8
    y: f64,              // 8
    device: i32,         // 4
    signal_kind: i32,    // 4  (kNone=0 kScroll=1)
    scroll_delta_x: f64, // 8
    scroll_delta_y: f64, // 8
    device_kind: i32,    // 4  (kMouse=1 kTouch=2)
    _pad1: u32,          // 4  (alignment padding)
    buttons: i64,        // 8
    pan_x: f64,          // 8
    pan_y: f64,          // 8
    scale: f64,          // 8
    rotation: f64,       // 8
    view_id: i64,        // 8
}

/// Flutter platform-view focus event. Required by newer engine view routing.
#[repr(C)]
#[derive(Clone, Copy)]
struct FlutterViewFocusEvent {
    struct_size: usize,
    view_id: i64,
    state: i32,     // kUnfocused=0, kFocused=1
    direction: i32, // kUndefined=0, kForward=1, kBackward=2
}

/// Flutter keyboard event.
#[repr(C)]
#[derive(Clone, Copy)]
struct FlutterKeyEvent {
    struct_size: usize, // 8
    timestamp: f64,     // 8  (milliseconds)
    kind: u32,          // 4  (kDown=0 kUp=1 kRepeat=2)
    _pad: u32,          // 4  (alignment)
    physical: u64,      // 8  (HID usage page/id)
    logical: u64,       // 8  (unicode codepoint / key code)
    character: u64,     // 8  (*const u8, nullable)
    synthesized: bool,  // 1
    _tail: [u8; 7],     // 7  (padding)
}

/// Incoming platform-channel message from the Flutter engine.
#[repr(C)]
#[derive(Clone, Copy)]
struct FlutterPlatformMessage {
    struct_size: usize,   // 8
    channel: u64,         // 8  *const u8 (NUL-terminated channel name)
    message: u64,         // 8  *const u8 (raw message bytes)
    message_size: usize,  // 8
    response_handle: u64, // 8  *mut FlutterPlatformMessageResponseHandle
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
    pub struct_size: usize,
    pub display_id: u64,
    pub single_display: bool,
    pub _pad0: [u8; 7],
    pub refresh_rate: f64,
    pub width: usize,
    pub height: usize,
    pub device_pixel_ratio: f64,
}

type NotifyDisplayUpdateFn = unsafe extern "C" fn(
    engine: u64,
    update_type: u32,
    displays: *const FlutterEngineDisplay,
    display_count: usize,
) -> i32;

// ── Engine function-pointer types ─────────────────────────────────────────────

type GetProcAddressesFn = unsafe extern "C" fn(table: *mut FlutterEngineProcTableApi) -> i32;
type CreateAotDataFn =
    unsafe extern "C" fn(source: *const FlutterEngineAotDataSource, data_out: *mut u64) -> i32;
type RunFn = unsafe extern "C" fn(
    version: u32,
    config: *const FlutterRendererConfig,
    args: *const FlutterProjectArgsRaw,
    ud: *mut (),
    engine: *mut u64,
) -> i32;
type SendWindowMetricsFn =
    unsafe extern "C" fn(engine: u64, evt: *const FlutterWindowMetricsEvent) -> i32;
type ScheduleFrameFn = unsafe extern "C" fn(engine: u64) -> i32;
type SendPlatformMessageFn =
    unsafe extern "C" fn(engine: u64, msg: *const FlutterPlatformMessage) -> i32;
type SendPointerEventFn =
    unsafe extern "C" fn(engine: u64, evts: *const FlutterPointerEvent, n: usize) -> i32;
type SendKeyEventFn =
    unsafe extern "C" fn(engine: u64, evt: *const FlutterKeyEvent, cb: u64, ud: u64) -> i32;
type OnVsyncFn =
    unsafe extern "C" fn(engine: u64, baton: usize, start_ns: u64, target_ns: u64) -> i32;
type GetCurrentTimeFn = unsafe extern "C" fn() -> u64;
type SendViewFocusEventFn =
    unsafe extern "C" fn(engine: u64, evt: *const FlutterViewFocusEvent) -> i32;

type InitializeFn = unsafe extern "C" fn(
    version: usize,
    config: *const FlutterRendererConfig,
    args: *const FlutterProjectArgsRaw,
    ud: *mut (),
    engine: *mut u64,
) -> i32;

type RunInitializedFn = unsafe extern "C" fn(engine: u64) -> i32;

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
static IS_AOT: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);

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
        Self {
            locked: core::sync::atomic::AtomicBool::new(false),
        }
    }

    fn lock(&self) -> SpinlockGuard {
        while self
            .locked
            .compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_err()
        {
            core::hint::spin_loop();
        }
        SpinlockGuard {
            locked: &self.locked,
        }
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
static SEND_PLATFORM_MESSAGE_RESPONSE_FN: AtomicU64 = AtomicU64::new(0);
static CURRENT_HOST_MODE: AtomicU64 = AtomicU64::new(0);
static CURRENT_APP_ID: AtomicU64 = AtomicU64::new(0);

const SHELL_CHANNEL: &[u8] = b"oscortex/shell";

const APPS_REQUEST_CHANNEL: &[u8] = b"oscortex/apps/request";

/// StandardMethodCodec envelope for a successful `null` response.
const METHOD_SUCCESS_NULL: &[u8] = &[0x00, 0x00];

fn send_platform_response_now(handle: u64, data: &[u8]) {
    if handle == 0 {
        return;
    }
    let engine = ENGINE.load(Ordering::SeqCst);
    let resp_fn_va = SEND_PLATFORM_MESSAGE_RESPONSE_FN.load(Ordering::SeqCst);
    if engine == 0 || resp_fn_va == 0 {
        return;
    }
    type SendResponseFn = unsafe extern "C" fn(u64, u64, *const u8, usize) -> i32;
    let send_response: SendResponseFn = unsafe { core::mem::transmute(resp_fn_va) };
    let _ = unsafe { send_response(engine, handle, data.as_ptr(), data.len()) };
}

fn respond_platform_message(msg: &FlutterPlatformMessage, data: &[u8]) {
    if msg.response_handle == 0 {
        return;
    }
    // Must reply synchronously: Dart may call BasicMessageChannel.send during
    // FlutterEngineRunInitialized before our event loop runs. Deferring to the
    // main thread deadlocks init (spinner forever on shell list).
    send_platform_response_now(msg.response_handle, data);
}

unsafe extern "C" fn runs_task_on_current_thread_callback(_user_data: *mut ()) -> bool {
    let tid = unsafe { syscall0(186) };
    tid == 1
}

unsafe extern "C" fn post_task_callback(
    task: FlutterTask,
    target_time_ns: u64,
    _user_data: *mut (),
) {
    static POST_TASK_LOG: AtomicU32 = AtomicU32::new(0);
    let log_n = POST_TASK_LOG.fetch_add(1, Ordering::Relaxed);
    if log_n < 32 {
        write(b"[embedder] post_task runner=");
        write_hex(task.runner);
        write(b" task=");
        write_hex(task.task);
        write(b" now=");
        write_hex(rdtsc_ns());
        write(b" target=");
        write_hex(target_time_ns);
        write(b"\n");
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
    // Merge the UI task runner onto the platform runner (same identifier=1) so
    // the engine runs Dart/UI on the platform thread instead of spawning a
    // worker thread. With ui_task_runner null, the engine never registers a UI
    // TaskQueueId, so the first Shell::OnPlatformViewScheduleFrame calls
    // MessageLoopTaskQueues::RegisterTask on an unregistered queue and writes a
    // DelayedTask into a null TaskQueueEntry (fault at 0x50) — killing pid 1
    // before any frame is presented. Sharing the platform runner gives the UI
    // queue a valid, registered entry.
    ui_task_runner: &PLATFORM_TASK_RUNNER_DESC,
};

static PRESENT_TRACE_COUNT: AtomicU32 = AtomicU32::new(0);

fn default_platform_reply(msg: &FlutterPlatformMessage) -> &'static [u8] {
    if msg.message == 0 || msg.message_size == 0 {
        return METHOD_SUCCESS_NULL;
    }
    let payload =
        unsafe { core::slice::from_raw_parts(msg.message as *const u8, msg.message_size) };
    let first = payload[0];

    // JSONMessageCodec / JSONMethodCodec messages begin with JSON text.
    // Reply with JSONMethodCodec success envelope `[null]`; plain `null`
    // triggers "Expected envelope List" errors on flutter/platform/navigation.
    if first == b'{' || first == b'[' || first == b'"' || first == b'n' {
        return b"[null]";
    }

    // StandardMethodCodec fallback for framework method channels.
    METHOD_SUCCESS_NULL
}

/// Present a rendered frame.  Called by the engine on the raster thread.
/// `allocation` is a row-major RGBA8 buffer; `row_bytes` may be > width*4.
unsafe extern "C" fn present_callback(
    _user_data: *mut (),
    _allocation: *const u8,
    row_bytes: usize,
    height: usize,
) -> bool {
    unsafe {
        let surface_id = SURFACE_ID;
        let pixel_len = row_bytes * height;
        let pixels = core::slice::from_raw_parts(_allocation, pixel_len);
        let ok = gpu_submit_strided(surface_id, pixels, row_bytes) >= 0;
        if ok {
            let n = PRESENT_TRACE_COUNT.fetch_add(1, Ordering::Relaxed);
            write(b"[embedder] present_callback n=");
            write_dec(n as u64);
            write(b"\n");
        }
        ok
    }
}

/// Vsync callback — the Flutter engine calls this when it needs a vsync
/// notification (passing an opaque `baton`).  We forward the baton to the
/// kernel; the APIC ISR returns it in the next `EV_VSYNC` event so the event
/// loop can call `FlutterEngineOnVsync(engine, baton, start_ns, target_ns)`.
unsafe extern "C" fn vsync_callback(_user_data: *mut (), baton: usize) {
    engine_vsync_baton_post(baton as u64);
}

/// Platform-message callback — the engine calls this when Dart sends a
/// platform-channel message.  ABI: `(const FlutterPlatformMessage* message, void* user_data)`.
unsafe extern "C" fn platform_message_callback(
    msg_ptr:   *const FlutterPlatformMessage,
    _user_data: *mut (),
) {
    if msg_ptr.is_null() {
        return;
    }
    let msg = unsafe { &*msg_ptr };
    let channel_slice = if msg.channel != 0 {
        unsafe { cstr_to_slice(msg.channel as *const u8) }
    } else {
        b"unknown"
    };

    static PFM_LOG: AtomicU32 = AtomicU32::new(0);
    let n = PFM_LOG.fetch_add(1, Ordering::Relaxed);
    if n < 8 {
        write(b"[embedder/pfm] ch=");
        write(channel_slice);
        write(b" sz=");
        write_dec(msg.message_size as u64);
        write(b"\n");
    }

    if channel_slice == APPS_REQUEST_CHANNEL {
        respond_platform_message(msg, METHOD_SUCCESS_NULL);
        return;
    }

    if channel_slice == SHELL_CHANNEL {
        handle_shell_platform_message(msg);
        return;
    }

    // Flutter framework MethodChannels (text input, mouse cursor, etc.) expect
    // a StandardMethodCodec reply; without one BasicMessageChannel/MethodChannel
    // `.send()` / `.invokeMethod()` calls hang forever.
    if msg.response_handle != 0 {
        respond_platform_message(msg, METHOD_SUCCESS_NULL);
    }
}

fn handle_shell_platform_message(msg: &FlutterPlatformMessage) {
    static SHELL_MSG_LOG: AtomicU32 = AtomicU32::new(0);
    if SHELL_MSG_LOG.fetch_add(1, Ordering::Relaxed) < 4 {
        write(b"[embedder/shell] platform message\n");
    }
    if msg.message == 0 || msg.message_size == 0 {
        if msg.response_handle != 0 {
            respond_platform_message(msg, b"{\"apps\":[]}");
        }
        return;
    }
    let payload =
        unsafe { core::slice::from_raw_parts(msg.message as *const u8, msg.message_size) };
    let reply = dispatch_shell_command(payload);
    if SHELL_MSG_LOG.load(Ordering::Relaxed) <= 4 {
        write(b"[embedder/shell] dispatch reply len=");
        write_dec(reply.len() as u64);
        write(b"\n");
    }
    respond_platform_message(msg, reply);
}

fn write_dec(mut v: u64) {
    if v == 0 {
        write(b"0");
        return;
    }
    let mut tmp = [0u8; 20];
    let mut n = 0usize;
    while v > 0 {
        tmp[n] = b'0' + (v % 10) as u8;
        v /= 10;
        n += 1;
    }
    while n > 0 {
        n -= 1;
        write(core::slice::from_ref(&tmp[n]));
    }
}

fn run_due_platform_tasks(engine: u64, now: u64, max_tasks: usize) {
    if engine == 0 || max_tasks == 0 {
        return;
    }
    let run_task_va = RUN_TASK_FN.load(Ordering::SeqCst);
    if run_task_va == 0 {
        return;
    }
    let run_task_fn: unsafe extern "C" fn(u64, *const FlutterTask) -> i32 =
        unsafe { core::mem::transmute(run_task_va) };
    for _ in 0..max_tasks {
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
        let Some(task) = task_to_run else { break; };
        unsafe {
            run_task_fn(engine, &task as *const _);
        }
    }
}

fn dispatch_shell_command(payload: &[u8]) -> &'static [u8] {
    let host_mode = CURRENT_HOST_MODE.load(Ordering::Acquire);
    let app_id = CURRENT_APP_ID.load(Ordering::Acquire);
    let shell_capable = host_mode == HOST_MODE_SHELL;
    let files_capable = host_mode == HOST_MODE_APP && app_id == 2;

    if payload.starts_with(b"list") {
        if !shell_capable {
            return b"{\"ok\":false,\"err\":\"cap\"}";
        }
        return format_app_list_json();
    }
    if payload.starts_with(b"vfs:list:") {
        if !(shell_capable || files_capable) {
            return b"{\"ok\":false,\"err\":\"cap\",\"entries\":[]}";
        }
        let path = trim_line(&payload[9..]);
        return format_vfs_list_json(path);
    }
    if payload.starts_with(b"launch:") {
        if !shell_capable {
            return b"{\"ok\":false,\"err\":\"cap\"}";
        }
        let id = parse_u32_after_colon(payload);
        if id == 0 {
            return b"{\"ok\":false}";
        }
        let pid = sys::app_launch(id);
        if pid > 0 {
            return b"{\"ok\":true}";
        }
        return b"{\"ok\":false}";
    }
    if payload.starts_with(b"uninstall:") {
        if !shell_capable {
            return b"{\"ok\":false,\"err\":\"cap\"}";
        }
        let id = parse_u32_after_colon(payload);
        if id == 0 {
            return b"{\"ok\":false}";
        }
        if sys::app_uninstall(id) == 0 {
            return b"{\"ok\":true}";
        }
        return b"{\"ok\":false}";
    }
    if payload.starts_with(b"install:") {
        if !(shell_capable || files_capable) {
            return b"{\"ok\":false,\"err\":\"cap\"}";
        }
        let path = &payload[8..];
        let path = trim_line(path);
        return install_osx_from_path(path);
    }
    b"{\"ok\":false}"
}

fn parse_u32_after_colon(payload: &[u8]) -> u32 {
    let mut n = 0u32;
    let mut started = false;
    for &b in payload {
        if b == b':' {
            started = true;
            continue;
        }
        if !started {
            continue;
        }
        if b < b'0' || b > b'9' {
            break;
        }
        n = n.saturating_mul(10).saturating_add((b - b'0') as u32);
    }
    n
}

fn trim_line(s: &[u8]) -> &[u8] {
    let end = s
        .iter()
        .position(|&b| b == 0 || b == b'\n')
        .unwrap_or(s.len());
    &s[..end]
}

static mut APP_LIST_JSON: [u8; 4096] = [0; 4096];
static APP_LIST_JSON_LEN: AtomicU32 = AtomicU32::new(0);

fn format_app_list_json() -> &'static [u8] {
    let mut records = [0u8; 4096];
    let count = sys::app_list(&mut records) as usize;
    let mut out = unsafe { &mut APP_LIST_JSON };
    let mut pos = 0usize;
    out[pos..pos + 9].copy_from_slice(b"{\"apps\":[");
    pos += 9;
    let mut i = 0usize;
    while i < count {
        let off = i * 88;
        if off + 88 > records.len() {
            break;
        }
        let id = u32::from_le_bytes(records[off..off + 4].try_into().unwrap_or([0; 4]));
        let name_end = records[off + 4..off + 68]
            .iter()
            .position(|&b| b == 0)
            .unwrap_or(64);
        let name = &records[off + 4..off + 4 + name_end];
        let version_end = records[off + 68..off + 84]
            .iter()
            .position(|&b| b == 0)
            .unwrap_or(16);
        let version = &records[off + 68..off + 68 + version_end];
        let flags = u32::from_le_bytes(records[off + 84..off + 88].try_into().unwrap_or([0; 4]));
        let system = (flags & 1) != 0;
        if i > 0 {
            out[pos] = b',';
            pos += 1;
        }
        out[pos] = b'{';
        pos += 1;
        const ID_PREFIX: &[u8] = b"\"id\":";
        out[pos..pos + ID_PREFIX.len()].copy_from_slice(ID_PREFIX);
        pos += ID_PREFIX.len();
        pos += write_json_u32(&mut out[pos..], id);
        const NAME_PREFIX: &[u8] = b",\"name\":\"";
        out[pos..pos + NAME_PREFIX.len()].copy_from_slice(NAME_PREFIX);
        pos += NAME_PREFIX.len();
        let copy = name
            .len()
            .min(out.len().saturating_sub(pos).saturating_sub(64));
        out[pos..pos + copy].copy_from_slice(&name[..copy]);
        pos += copy;
        const VERSION_PREFIX: &[u8] = b"\",\"version\":\"";
        out[pos..pos + VERSION_PREFIX.len()].copy_from_slice(VERSION_PREFIX);
        pos += VERSION_PREFIX.len();
        let copy = version
            .len()
            .min(out.len().saturating_sub(pos).saturating_sub(40));
        out[pos..pos + copy].copy_from_slice(&version[..copy]);
        pos += copy;
        const SYSTEM_TRUE: &[u8] = b"\",\"system\":true";
        const SYSTEM_FALSE: &[u8] = b"\",\"system\":false";
        let system_slice = if system { SYSTEM_TRUE } else { SYSTEM_FALSE };
        out[pos..pos + system_slice.len()].copy_from_slice(system_slice);
        pos += system_slice.len();
        if name == b"Canvas" {
            const ROLE: &[u8] = b",\"coreRole\":\"canvas\"";
            out[pos..pos + ROLE.len()].copy_from_slice(ROLE);
            pos += ROLE.len();
        } else if name == b"Files" {
            const ROLE: &[u8] = b",\"coreRole\":\"files\"";
            out[pos..pos + ROLE.len()].copy_from_slice(ROLE);
            pos += ROLE.len();
        } else if name == b"Web Link" {
            const ROLE: &[u8] = b",\"coreRole\":\"web\"";
            out[pos..pos + ROLE.len()].copy_from_slice(ROLE);
            pos += ROLE.len();
        }
        out[pos] = b'}';
        pos += 1;
        i += 1;
    }
    out[pos..pos + 2].copy_from_slice(b"]}");
    pos += 2;
    APP_LIST_JSON_LEN.store(pos as u32, Ordering::Release);
    unsafe { core::slice::from_raw_parts(APP_LIST_JSON.as_ptr(), pos) }
}

static mut VFS_LIST_JSON: [u8; 8192] = [0; 8192];
static VFS_LIST_JSON_LEN: AtomicU32 = AtomicU32::new(0);

fn format_vfs_list_json(path: &[u8]) -> &'static [u8] {
    let mut records = [0u8; 4096];
    let n = sys::vfs_list(path, &mut records);
    if n < 0 {
        return b"{\"ok\":false,\"entries\":[]}";
    }

    let mut out = unsafe { &mut VFS_LIST_JSON };
    let mut pos = 0usize;
    const PREFIX: &[u8] = b"{\"ok\":true,\"entries\":[";
    out[pos..pos + PREFIX.len()].copy_from_slice(PREFIX);
    pos += PREFIX.len();

    let mut first = true;
    let valid = (n as usize).min(records.len());
    for line in records[..valid].split(|&b| b == b'\n') {
        if line.is_empty() {
            continue;
        }
        if !first {
            out[pos] = b',';
            pos += 1;
        }
        first = false;

        const ITEM_PREFIX: &[u8] = b"{\"name\":\"";
        out[pos..pos + ITEM_PREFIX.len()].copy_from_slice(ITEM_PREFIX);
        pos += ITEM_PREFIX.len();
        for &b in line {
            if pos + 8 >= out.len() {
                break;
            }
            if b == b'"' || b == b'\\' {
                out[pos] = b'\\';
                out[pos + 1] = b;
                pos += 2;
            } else {
                out[pos] = b;
                pos += 1;
            }
        }
        let installable = line.ends_with(b".osx");
        const MID_TRUE: &[u8] = b"\",\"installable\":true}";
        const MID_FALSE: &[u8] = b"\",\"installable\":false}";
        let suffix = if installable { MID_TRUE } else { MID_FALSE };
        if pos + suffix.len() >= out.len() {
            break;
        }
        out[pos..pos + suffix.len()].copy_from_slice(suffix);
        pos += suffix.len();
    }

    out[pos..pos + 2].copy_from_slice(b"]}");
    pos += 2;
    VFS_LIST_JSON_LEN.store(pos as u32, Ordering::Release);
    unsafe { core::slice::from_raw_parts(VFS_LIST_JSON.as_ptr(), pos) }
}

fn write_json_u32(out: &mut [u8], mut v: u32) -> usize {
    if out.is_empty() {
        return 0;
    }
    if v == 0 {
        out[0] = b'0';
        return 1;
    }
    let mut tmp = [0u8; 10];
    let mut n = 0usize;
    while v > 0 {
        tmp[n] = b'0' + (v % 10) as u8;
        v /= 10;
        n += 1;
    }
    for i in 0..n {
        out[i] = tmp[n - 1 - i];
    }
    n
}

static mut INSTALL_REPLY: [u8; 48] = [0; 48];
static INSTALL_REPLY_LEN: AtomicU32 = AtomicU32::new(0);

fn install_osx_from_path(path: &[u8]) -> &'static [u8] {
    let file_sz = sys::vfs_stat(path);
    if file_sz <= 0 {
        write(b"[embedder/shell] install stat failed\n");
        return b"{\"ok\":false,\"err\":\"stat\"}";
    }
    let file_sz = file_sz as usize;
    if file_sz > 16 * 1024 * 1024 {
        write(b"[embedder/shell] install file too large\n");
        return b"{\"ok\":false,\"err\":\"size\"}";
    }
    let va = sys::mmap_anon(file_sz);
    if va == 0 || va == u64::MAX {
        write(b"[embedder/shell] install mmap failed\n");
        return b"{\"ok\":false,\"err\":\"mmap\"}";
    }
    let bundle = unsafe { core::slice::from_raw_parts_mut(va as *mut u8, file_sz) };
    let n = sys::vfs_read(path, bundle);
    if n as usize != file_sz {
        write(b"[embedder/shell] install read short\n");
        return b"{\"ok\":false,\"err\":\"read\"}";
    }
    let mut id = 0u32;
    if sys::app_install(bundle, &mut id) != 0 {
        write(b"[embedder/shell] install app_install failed\n");
        return b"{\"ok\":false,\"err\":\"install\"}";
    }
    write(b"[embedder/shell] installed id=");
    write_dec(id as u64);
    write(b"\n");
    let mut out = unsafe { &mut INSTALL_REPLY };
    out[..18].copy_from_slice(b"{\"ok\":true,\"id\":");
    let mut pos = 18usize;
    pos += write_json_u32(&mut out[pos..], id);
    out[pos] = b'}';
    pos += 1;
    INSTALL_REPLY_LEN.store(pos as u32, Ordering::Release);
    unsafe { core::slice::from_raw_parts(INSTALL_REPLY.as_ptr(), pos) }
}

/// Log callback — writes to the kernel's serial debug output.
unsafe extern "C" fn log_message_callback(tag: *const u8, msg: *const u8, _ud: *mut ()) {
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
        while *ptr.add(len) != 0 && len < 4096 {
            len += 1;
        }
        core::slice::from_raw_parts(ptr, len)
    }
}

// ── Engine library path constant ──────────────────────────────────────────────

const ENGINE_LIB_PATH: &[u8] = b"/system/lib/libflutter_engine.so";

const HOST_MODE_SHELL: u64 = 1;
const HOST_MODE_APP: u64 = 2;

fn read_host_bootstrap() -> (u64, u64, u64) {
    let rdi: u64;
    let rsi: u64;
    let rdx: u64;
    unsafe {
        asm!(
            "mov {0}, rdi",
            "mov {1}, rsi",
            "mov {2}, rdx",
            out(reg) rdi,
            out(reg) rsi,
            out(reg) rdx,
        );
    }
    (rdi, rsi, rdx)
}

// ── Main embedder logic ───────────────────────────────────────────────────────

extern "C" fn main_embedder() {
    let (host_mode, app_id, aot_va) = read_host_bootstrap();
    CURRENT_HOST_MODE.store(host_mode, Ordering::Release);
    CURRENT_APP_ID.store(app_id, Ordering::Release);
    write(b"[host] starting\n");

    // 1. Register as the engine host.
    let host_pid = engine_host_register();
    if host_pid < 0 {
        write(b"[embedder] engine_host_register failed\n");
        exit(-1);
    }

    // 1b. Phase 33-A: request high refresh cadence for smoother input/render.
    vsync_set_hz(120);

    // 2. Open the engine library.
    dlopen(b"/system/lib/liboscortex_libc.so", 0);
    let handle = dlopen(ENGINE_LIB_PATH, 0);
    if handle <= 0 {
        write(b"[embedder] dlopen failed for /system/lib/libflutter_engine.so\n");
        exit(-1);
    }
    let handle = handle as u32;

    // 2b. Call DT_INIT and DT_INIT_ARRAY constructors.
    //     Flutter's global heap/mutex state is uninitialized until these run.
    {
        let mut init_fn: u64 = 0;
        let mut array_va: u64 = 0;
        let mut count: u64 = 0;
        dl_get_init_array(handle, &mut init_fn, &mut array_va, &mut count);
        if init_fn != 0 {
            let f: unsafe extern "C" fn() = unsafe { core::mem::transmute(init_fn) };
            unsafe {
                f();
            }
        }
        for i in 0..count as usize {
            let fn_ptr_addr = array_va + (i * 8) as u64;
            let fn_va = unsafe { core::ptr::read_unaligned(fn_ptr_addr as *const u64) };
            if fn_va != 0 && fn_va != u64::MAX {
                let f: unsafe extern "C" fn() = unsafe { core::mem::transmute(fn_va) };
                unsafe {
                    f();
                }
            }
        }
    }

    // 3. Resolve FlutterEngineGetProcAddresses.
    let get_procs_va = dlsym(handle, b"FlutterEngineGetProcAddresses");

    // 4. Build and fill the proc table.
    let mut proctable = FlutterEngineProcTable::default();
    let mut initialize_va = 0u64;
    let mut run_initialized_va = 0u64;
    let mut notify_display_update_va = 0u64;
    let mut get_current_time_va = 0u64;
    let mut send_view_focus_event_va = 0u64;

    if get_procs_va != 0 {
        let mut api_table = FlutterEngineProcTableApi::default();
        api_table.struct_size = core::mem::size_of::<FlutterEngineProcTableApi>();
        // SAFETY: we resolved this VA from our own address space via dlsym.
        let get_procs: GetProcAddressesFn = unsafe { core::mem::transmute(get_procs_va) };
        let rc = unsafe { get_procs(&mut api_table as *mut FlutterEngineProcTableApi) };
        if rc != 0 {
            write(b"[embedder] GetProcAddresses returned non-zero\n");
        }
        proctable.run = api_table.run;
        proctable.shutdown = api_table.shutdown;
        proctable.send_window_metrics = api_table.send_window_metrics;
        proctable.send_pointer_event = api_table.send_pointer_event;
        proctable.send_key_event = api_table.send_key_event;
        proctable.on_vsync = api_table.on_vsync;
        proctable.schedule_frame = api_table.schedule_frame;
        proctable.send_platform_message = api_table.send_platform_message;
        SEND_PLATFORM_MESSAGE_FN.store(api_table.send_platform_message, Ordering::SeqCst);
        SEND_PLATFORM_MESSAGE_RESPONSE_FN
            .store(api_table.send_platform_message_response, Ordering::SeqCst);

        initialize_va = api_table.initialize;
        run_initialized_va = api_table.run_initialized;
        notify_display_update_va = api_table.notify_display_update;
        get_current_time_va = api_table.get_current_time;
        send_view_focus_event_va = api_table.send_view_focus_event;
        RUN_TASK_FN.store(api_table.run_task, Ordering::SeqCst);
    } else {
        // Stub path: resolve each symbol manually.
        proctable.run = dlsym(handle, b"FlutterEngineRun");
        proctable.shutdown = dlsym(handle, b"FlutterEngineShutdown");
        proctable.send_window_metrics = dlsym(handle, b"FlutterEngineSendWindowMetricsEvent");
        proctable.send_pointer_event = dlsym(handle, b"FlutterEngineSendPointerEvent");
        proctable.send_key_event = dlsym(handle, b"FlutterEngineSendKeyEvent");
        proctable.on_vsync = dlsym(handle, b"FlutterEngineOnVsync");
        proctable.schedule_frame = dlsym(handle, b"FlutterEngineScheduleFrame");
        proctable.send_platform_message = dlsym(handle, b"FlutterEngineSendPlatformMessage");
        SEND_PLATFORM_MESSAGE_FN.store(proctable.send_platform_message, Ordering::SeqCst);
        SEND_PLATFORM_MESSAGE_RESPONSE_FN.store(
            dlsym(handle, b"FlutterEngineSendPlatformMessageResponse"),
            Ordering::SeqCst,
        );

        initialize_va = dlsym(handle, b"FlutterEngineInitialize");
        run_initialized_va = dlsym(handle, b"FlutterEngineRunInitialized");
        notify_display_update_va = dlsym(handle, b"FlutterEngineNotifyDisplayUpdate");
        get_current_time_va = dlsym(handle, b"FlutterEngineGetCurrentTime");
        send_view_focus_event_va = dlsym(handle, b"FlutterEngineSendViewFocusEvent");
        RUN_TASK_FN.store(dlsym(handle, b"FlutterEngineRunTask"), Ordering::SeqCst);
    }

    if initialize_va == 0
        || run_initialized_va == 0
        || notify_display_update_va == 0
        || RUN_TASK_FN.load(Ordering::SeqCst) == 0
    {
        write(b"[embedder] ERROR: FlutterEngineInitialize, RunInitialized, NotifyDisplayUpdate or RunTask not found!\n");
        exit(-1);
    }

    // Verify engine compilation mode: AOT vs JIT
    let runs_aot_va = dlsym(handle, b"FlutterEngineRunsAOTCompiledDartCode");
    let engine_load_base = if runs_aot_va != 0 {
        runs_aot_va - 0x1960260
    } else {
        0x4_8000_0000
    };
    write(b"[embedder] FlutterEngineRunsAOTCompiledDartCode VA: ");
    write_hex(runs_aot_va);
    write(b"\n");
    let mut is_aot = false;
    if runs_aot_va != 0 {
        let runs_aot: unsafe extern "C" fn() -> bool = unsafe { core::mem::transmute(runs_aot_va) };
        is_aot = unsafe { runs_aot() };
        write(b"[embedder] runs_aot_compiled_dart_code returned: ");
        if is_aot {
            write(b"true\n");
        } else {
            write(b"false\n");
        }
    }
    IS_AOT.store(is_aot, Ordering::SeqCst);

    // 5. Register proc table with the kernel.
    engine_proctable_set(
        &proctable as *const FlutterEngineProcTable as u64,
        core::mem::size_of::<FlutterEngineProcTable>(),
    );

    // 6. Create a compositor surface sized to the framebuffer.
    let fb_packed = fb_size_packed();
    let fb_w = ((fb_packed >> 32) & 0xFFFF_FFFF) as u32;
    let fb_h = (fb_packed & 0xFFFF_FFFF) as u32;
    let (w, h) = if fb_w > 0 && fb_h > 0 && fb_w <= 16_384 && fb_h <= 16_384 {
        (fb_w, fb_h)
    } else {
        (1280, 720)
    };

    write(b"[embedder] FB resolution: w=");
    write_dec(w as u64);
    write(b" h=");
    write_dec(h as u64);
    write(b"\n");

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
        SURFACE_W = w;
        SURFACE_H = h;
    }

    // 7. Build project args + renderer config.
    // IMPORTANT: FlutterRendererConfig is 120 bytes (C ABI union). Use the
    // new_software() constructor which zero-initialises all 120 bytes and
    // writes the software config fields at the correct offsets.
    let renderer_config = FlutterRendererConfig::new_software(present_callback as *const () as u64);

    let assets_path = b"/system/flutter/flutter_assets\0";
    let icu_path = b"/system/flutter/icudtl.dat\0";

    // Engine command-line switches. The first argv item is the executable
    // name (engine skips it). We disable Impeller because the Impeller
    // backend requires GPU contexts we don't provide and otherwise destroys
    // an unfulfilled `std::promise<RuntimeStageBackend>`, throwing
    // `std::future_error` which abort()s under our no-libunwind runtime.
    static ARG0: &[u8] = b"oscortex-flutter\0";
    static ARG1: &[u8] = b"--enable-impeller=false\0";
    static ARG2: &[u8] = b"--enable-software-rendering=true\0";
    static ARG3: &[u8] = b"--disable-vm-service\0";
    static ARG4: &[u8] = b"--precompiled-mode\0";
    static ARG5: &[u8] = b"--old_gen_heap_size=64\0";
    static ARG6: &[u8] = b"--new_gen_heap_size=8\0";
    static ARG7: &[u8] = b"--max_old_gen_heap_size=64\0";
    // SINGLE-THREADED VM/GC: the bare-metal sync layer cannot reliably bring all
    // Dart VM threads to a GC stop-the-world safepoint, so the scavenger livelocks
    // (endless new-space page churn, first frame never completes for the heavy
    // Material app; a trivial tree squeaks through). Eliminate the background
    // threads that must be coordinated: no background JIT compiler thread (compile
    // synchronously on the mutator), no concurrent mark/sweep threads, serial GC
    // marker/scavenger tasks. Fewer threads → safepoint always reachable.
    // Pass Dart VM flags the canonical way: a single --dart-flags= switch that the
    // Flutter engine's SettingsFromCommandLine splits and forwards to Dart_SetVMFlags.
    // (Bare --flag args are NOT forwarded to the VM by the engine.) marker_tasks=0
    // and scavenger_tasks=0 make GC serial on the mutator (no parallel GC tasks to
    // coordinate through a safepoint); no-background_compilation removes the
    // background JIT compiler thread.
    static ARG8: &[u8] =
        b"--dart-flags=--no-background_compilation --no-concurrent_mark --no-concurrent_sweep --marker_tasks=0 --scavenger_tasks=0\0";
    #[repr(transparent)]
    struct ArgvPtrs([*const u8; 9]);
    unsafe impl Sync for ArgvPtrs {}
    static ENGINE_ARGV: ArgvPtrs =
        ArgvPtrs([
            ARG0.as_ptr(),
            ARG1.as_ptr(),
            ARG2.as_ptr(),
            ARG3.as_ptr(),
            ARG4.as_ptr(),
            ARG5.as_ptr(),
            ARG6.as_ptr(),
            ARG7.as_ptr(),
            ARG8.as_ptr(),
        ]);

    let mut project_args = FlutterProjectArgsRaw {
        bytes: [0; FLUTTER_PROJECT_ARGS_SIZE],
    };
    write_u64_at(
        &mut project_args.bytes,
        OFF_PROJECT_ARGS_STRUCT_SIZE,
        FLUTTER_PROJECT_ARGS_SIZE as u64,
    );
    write_u64_at(
        &mut project_args.bytes,
        OFF_PROJECT_ARGS_ASSETS_PATH,
        assets_path.as_ptr() as u64,
    );
    write_u64_at(
        &mut project_args.bytes,
        OFF_PROJECT_ARGS_ICU_DATA_PATH,
        icu_path.as_ptr() as u64,
    );
    write_i32_at(
        &mut project_args.bytes,
        OFF_PROJECT_ARGS_COMMAND_LINE_ARGC,
        ENGINE_ARGV.0.len() as i32,
    );
    write_u64_at(
        &mut project_args.bytes,
        OFF_PROJECT_ARGS_COMMAND_LINE_ARGV,
        ENGINE_ARGV.0.as_ptr() as u64,
    );
    write_u64_at(
        &mut project_args.bytes,
        OFF_PROJECT_ARGS_PLATFORM_MESSAGE_CALLBACK,
        platform_message_callback as *const () as u64,
    );
    write_u64_at(
        &mut project_args.bytes,
        OFF_PROJECT_ARGS_VSYNC_CALLBACK,
        // RELIABILITY TEST: engine-internal vsync (NULL). Still on-demand (the
        // engine's own VsyncWaiter only produces frames when needed) but avoids the
        // cross-process baton round-trip (engine->kernel->pid1->OnVsync) which is
        // one more thing to stall under the heavy real-app sync load. This was the
        // render-milestone config. Toggle back to embedder vsync once the sync
        // layer is reliable.
        0u64,
    );
    write_i32_at(
        &mut project_args.bytes,
        OFF_PROJECT_ARGS_DART_ENTRYPOINT_ARGC,
        0,
    );
    write_u64_at(
        &mut project_args.bytes,
        OFF_PROJECT_ARGS_DART_ENTRYPOINT_ARGV,
        0,
    );
    write_u64_at(
        &mut project_args.bytes,
        OFF_PROJECT_ARGS_LOG_MESSAGE_CALLBACK,
        log_message_callback as *const () as u64,
    );
    write_u64_at(
        &mut project_args.bytes,
        OFF_PROJECT_ARGS_DART_OLD_GEN_HEAP_SIZE,
        64, // 64 MB heap limit
    );

    if host_mode == HOST_MODE_APP {
        write(b"[host] APP mode pid bootstrap app_id=");
        write_dec(app_id as u64);
        write(b" aot_va=");
        write_hex(aot_va);
        write(b"\n");
        configure_app_assets(&mut project_args, app_id);
        if is_aot {
            configure_aot_snapshots(&mut project_args, aot_va);
        }
    } else {
        write(b"[host] SHELL mode\n");
        configure_project_assets(&mut project_args, b"/system/flutter/flutter_assets");
        if is_aot {
            write(b"[embedder] registering shell libapp.so globally...\n");
            let aot_handle = sys::dlopen(b"/system/flutter/libapp.so", 0);
            if aot_handle <= 0 {
                write(b"[embedder] WARNING: dlopen /system/flutter/libapp.so failed\n");
            } else {
                write(b"[embedder] shell libapp.so registered globally\n");
            }
            configure_aot_snapshots(&mut project_args, 0);
        }
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
        // EXPERIMENT 1 (vsync isolation): engine-default task runners (NULL) so the
        // engine spawns and manages its own platform/UI/raster/IO threads — the
        // proven-rendering threading. This isolates the ONE variable under test:
        // embedder-driven vsync (vsync_callback is SET above). With this config the
        // UI thread's Animator should call our vsync_callback on each frame request,
        // posting a baton the main loop acks via FlutterEngineOnVsync. If the serial
        // log shows on_vsync called with NON-ZERO batons, embedder-driven on-demand
        // vsync works and we can delete the frame-pump hacks (experiment 2).
        0u64,
    );
    let _ = &CUSTOM_TASK_RUNNERS; // silence unused while custom runners are disabled

    // Initialize the MessageLoop for the main thread before FlutterEngineInitialize.
    // Without this, task runners stall in epoll_wait and RunInitialized blocks forever.
    write(b"[embedder] initializing main thread message loop...\n");
    let ensure_initialized_va = dlsym(handle, b"_ZN3fml11MessageLoop33EnsureInitializedForCurrentThreadEv");
    if ensure_initialized_va != 0 {
        let ensure_initialized: unsafe extern "C" fn() = unsafe { core::mem::transmute(ensure_initialized_va) };
        unsafe {
            ensure_initialized();
        }
        write(b"[embedder] message loop initialized!\n");
    } else {
        write(b"[embedder] ERROR: fml::MessageLoop::EnsureInitializedForCurrentThread not found!\n");
        exit(-1);
    }

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
        for i in 0..8 {
            hex[41 + i] = d[((r >> ((7 - i) * 4)) & 0xF) as usize];
        }
        write(&hex);
        exit(-1);
    }
    write(b"[embedder] FlutterEngineInitialize OK\n");
    ENGINE.store(engine_out, Ordering::SeqCst);

    // 9. Run the engine on the main thread (starts the shell)
    write(b"[embedder] calling FlutterEngineRunInitialized...\n");
    let rc_run = unsafe {
        let run_initialized_fn: RunInitializedFn =
            unsafe { core::mem::transmute(run_initialized_va) };
        run_initialized_fn(engine_out)
    };
    if rc_run != 0 {
        let mut hex = *b"[embedder] FlutterEngineRunInitialized FAILED rc=0x________\n";
        let d = b"0123456789abcdef";
        let r = rc_run as u32;
        for i in 0..8 {
            hex[45 + i] = d[((r >> ((7 - i) * 4)) & 0xF) as usize];
        }
        write(&hex);
        exit(-1);
    }
    write(b"[embedder] FlutterEngineRunInitialized OK\n");

    // 10. Immediately notify display topology (now that shell exists)
    {
        let display = FlutterEngineDisplay {
            struct_size: core::mem::size_of::<FlutterEngineDisplay>(),
            display_id: 0,
            single_display: true,
            _pad0: [0; 7],
            refresh_rate: 60.0,
            width: w as usize,
            height: h as usize,
            device_pixel_ratio: 1.0,
        };
        let notify_display: NotifyDisplayUpdateFn =
            unsafe { core::mem::transmute(notify_display_update_va) };
        let rc_disp = unsafe { notify_display(engine_out, 0, &display as *const _, 1) };
        if rc_disp == 0 {
        } else {
            let mut hex = *b"[embedder] FlutterEngineNotifyDisplayUpdate FAILED rc=0x________\n";
            let d = b"0123456789abcdef";
            let r = rc_disp as u32;
            for i in 0..8 {
                hex[54 + i] = d[((r >> ((7 - i) * 4)) & 0xF) as usize];
            }
            write(&hex);
        }
    }

    // 11. Immediately send initial window metrics
    {
        let metrics = FlutterWindowMetricsEvent {
            struct_size: core::mem::size_of::<FlutterWindowMetricsEvent>(),
            width: w as usize,
            height: h as usize,
            pixel_ratio: 1.0,
            left: 0,
            top: 0,
            physical_view_inset_top: 0.0,
            physical_view_inset_right: 0.0,
            physical_view_inset_bottom: 0.0,
            physical_view_inset_left: 0.0,
            display_id: 0,
            view_id: 0,
        };
        let send_metrics: SendWindowMetricsFn =
            unsafe { core::mem::transmute(proctable.send_window_metrics) };
        let rc = unsafe { send_metrics(engine_out, &metrics as *const _) };
        if rc == 0 {
            if send_view_focus_event_va != 0 {
                let focus = FlutterViewFocusEvent {
                    struct_size: core::mem::size_of::<FlutterViewFocusEvent>(),
                    view_id: 0,
                    state: 1,
                    direction: 0,
                };
                let send_focus: SendViewFocusEventFn =
                    unsafe { core::mem::transmute(send_view_focus_event_va) };
                let rc_focus = unsafe { send_focus(engine_out, &focus as *const _) };
                write(b"[embedder] send view focus rc=");
                write_dec(rc_focus as u64);
                write(b"\n");
            } else {
                write(b"[embedder] send view focus unavailable\n");
            }

            if proctable.schedule_frame != 0 {
                let rc_sf =
                    schedule_frame_with_log(engine_out, proctable.schedule_frame, b"initial");
                if rc_sf == 0 {
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

    // 12. Event loop: dispatch vsync / pointer / key / platform-channel events,
    // and run custom task runner platform tasks.
    let mut ev = WmEvent::default();
    let mut platform_buf = [0u8; 512];
    let mut startup_watchdog_stage: u32 = 0;
    let mut startup_watchdog_next_ns: u64 = rdtsc_ns() + 100_000_000;
    // Frame pump: call FlutterEngineScheduleFrame at ~60 fps so Flutter keeps
    // rendering even after it goes idle (static UI or before Dart init completes).
    let mut frame_pump_next_ns: u64 = rdtsc_ns() + 1_000_000;

    write(b"[embedder] entering event loop\n");
    loop {
        let now = rdtsc_ns();
        run_due_platform_tasks(engine_out, now, 64);

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

        if engine_out != 0 && now >= startup_watchdog_next_ns && startup_watchdog_stage < 6 {
            if proctable.schedule_frame != 0 {
                let _ = schedule_frame_with_log(engine_out, proctable.schedule_frame, b"startup");
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
                if rc_pm == 0 {}
            }

            startup_watchdog_stage += 1;
            startup_watchdog_next_ns = now + 100_000_000;
        }

        let r = wm_event_wait(&mut ev, timeout_ms);
        // Always drain the platform-channel queue (run-visible.log had 16 early
        // platform-recv calls; waiting for wm_event_wait EAGAIN starves init).
        let n = platform_msg_recv(&mut platform_buf);
        if n > 0 {
            if (n as usize) >= 8 + 2 + 4 {
                let seq = u64::from_le_bytes(platform_buf[0..8].try_into().unwrap_or([0; 8]));
                platform_msg_reply(seq, b"ok");
            }
        }
        if r <= 0 {
            // Frame pump: keep Flutter rendering at ~60 fps so the Dart UI
            // thread has time to finish init and produce a real frame.
            let now_pump = rdtsc_ns();
            if engine_out != 0 && proctable.schedule_frame != 0 && now_pump >= frame_pump_next_ns {
                let _ = schedule_frame_with_log(engine_out, proctable.schedule_frame, b"idle");
                let presents = PRESENT_TRACE_COUNT.load(Ordering::Relaxed);
                let interval = if presents < 10 {
                    1_000_000u64
                } else {
                    16_666_666u64
                };
                frame_pump_next_ns = now_pump + interval;
            }
            continue;
        }

        match ev.kind {
            EV_VSYNC => {
                let baton = ev.b as usize;
                // When no baton is pending the APIC timer still fires at 60 Hz
                // delivering EV_VSYNC(baton=0).  Use these ticks to pump
                // FlutterEngineScheduleFrame so Flutter posts a real baton and
                // keeps rendering even for static (no-animation) apps.
                if baton == 0 && engine_out != 0 && proctable.schedule_frame != 0 {
                    let now_pump = rdtsc_ns();
                    if now_pump >= frame_pump_next_ns {
                        let _ = schedule_frame_with_log(
                            engine_out,
                            proctable.schedule_frame,
                            b"vsync0",
                        );
                        let presents = PRESENT_TRACE_COUNT.load(Ordering::Relaxed);
                        let interval = if presents < 10 {
                            1_000_000u64
                        } else {
                            16_666_666u64
                        };
                        frame_pump_next_ns = now_pump + interval;
                    }
                }
                if engine_out != 0 && proctable.on_vsync != 0 && baton != 0 {
                    static VSYNC_SEND_LOG: AtomicU32 = AtomicU32::new(0);
                    let vsync_n = VSYNC_SEND_LOG.fetch_add(1, Ordering::Relaxed);
                    // CRITICAL: frame_start/target MUST be on the engine's own monotonic
                    // clock (FlutterEngineGetCurrentTime), NOT rdtsc_ns(). rdtsc_ns adds a
                    // ~1.7e18 ns epoch offset and assumes 3GHz, so it lands ~17 years in
                    // the engine's future — the Animator then schedules BeginFrame
                    // effectively never, so no frame is built and (because the pending-frame
                    // semaphore is never released) no further vsync is requested. Using the
                    // engine clock here is what every reference embedder does.
                    let now_ns = if get_current_time_va != 0 {
                        let get_time: GetCurrentTimeFn =
                            unsafe { core::mem::transmute(get_current_time_va) };
                        unsafe { get_time() }
                    } else {
                        rdtsc_ns()
                    };
                    let target_ns = now_ns + 16_666_666;
                    let f: OnVsyncFn = unsafe { core::mem::transmute(proctable.on_vsync) };
                    write(b"[embedder/vsync] calling on_vsync n=");
                    write_dec(vsync_n as u64);
                    write(b" baton=");
                    write_hex(baton as u64);
                    write(b"\n");
                    // A baton is a one-shot token: call OnVsync EXACTLY once.
                    // Calling it twice corrupts the engine's vsync accounting
                    // and stops it from scheduling further frames.
                    unsafe { f(engine_out, baton, now_ns, target_ns) };

                    // OnVsync posts raster/UI work to the engine's runner threads
                    // (separate PIDs). We deliberately do NOT spin on sched_yield
                    // here: sys_sched_yield switches to a busy-spinning runner via
                    // enter_user_by_pid_noreturn and never regains the CPU, which
                    // stalls this loop so it stops draining WM pointer/key events
                    // (clicks never reach Flutter). Instead we fall through to
                    // wm_event_wait below, which cooperatively yields CPU to the
                    // runner threads while remaining wakeable on the next vsync or
                    // input event.

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
                            let _ = schedule_frame_with_log(
                                engine_out,
                                proctable.schedule_frame,
                                b"watchdog",
                            );
                            NO_PRESENT_CONSECUTIVE.store(0, Ordering::Relaxed);
                        }
                    }

                    let presents = cur_presents;
                    if presents == 0 && (vsync_n == 10 || vsync_n == 30) {
                        if proctable.schedule_frame != 0 {
                            let _ = schedule_frame_with_log(
                                engine_out,
                                proctable.schedule_frame,
                                b"late-vsync",
                            );
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
                            let rc_pm =
                                unsafe { send_platform_message(engine_out, &msg as *const _) };
                            if rc_pm == 0 {}
                        }
                    }
                }
            }
            EV_POINTER => {
                let buttons = ev.flags as i64;
                if engine_out != 0 && proctable.send_pointer_event != 0 {
                    // push_pointer packs: a = (x as u32 as u64) << 32 | (y as u32)
                    let x = ((ev.a >> 32) as i32) as f64;
                    let y = (ev.a as u32) as f64;
                    let f: SendPointerEventFn =
                        unsafe { core::mem::transmute(proctable.send_pointer_event) };

                    static POINTER_ADDED: AtomicU32 = AtomicU32::new(0);
                    static LAST_POINTER_BUTTONS: AtomicU64 = AtomicU64::new(0);
                    static LAST_TIMESTAMP: AtomicU64 = AtomicU64::new(0);

                    let engine_now_us = || -> u64 {
                        if get_current_time_va != 0 {
                            let get_time: GetCurrentTimeFn =
                                unsafe { core::mem::transmute(get_current_time_va) };
                            unsafe { get_time() / 1000 }
                        } else {
                            rdtsc_ns() / 1000
                        }
                    };

                    let send = |phase: i32, btns: i64| -> i32 {
                        let mut timestamp = engine_now_us();
                        loop {
                            let last = LAST_TIMESTAMP.load(Ordering::SeqCst);
                            let next = if timestamp <= last {
                                last + 1
                            } else {
                                timestamp
                            };
                            if LAST_TIMESTAMP
                                .compare_exchange_weak(
                                    last,
                                    next,
                                    Ordering::SeqCst,
                                    Ordering::SeqCst,
                                )
                                .is_ok()
                            {
                                timestamp = next;
                                break;
                            }
                        }
                        let evt = FlutterPointerEvent {
                            struct_size: core::mem::size_of::<FlutterPointerEvent>(),
                            phase,
                            _pad0: 0,
                            timestamp,
                            x,
                            y,
                            device: 0,
                            signal_kind: 0,
                            scroll_delta_x: 0.0,
                            scroll_delta_y: 0.0,
                            device_kind: 1,
                            _pad1: 0,
                            buttons: btns,
                            pan_x: 0.0,
                            pan_y: 0.0,
                            scale: 1.0,
                            rotation: 0.0,
                            view_id: 0,
                        };
                        unsafe { f(engine_out, &evt as *const _, 1) }
                    };

                    if POINTER_ADDED.swap(1, Ordering::Relaxed) == 0 {
                        let rc = send(4, 0); // kAdd
                        write(b"[embedder/ptr] kAdd (4) rc=");
                        write_dec(rc as u64);
                        write(b"\n");
                    }

                    let prev_buttons =
                        LAST_POINTER_BUTTONS.swap(buttons as u64, Ordering::Relaxed) as i64;
                    let phase = if prev_buttons == 0 && buttons != 0 {
                        2i32 // kDown
                    } else if prev_buttons != 0 && buttons == 0 {
                        1i32 // kUp
                    } else if buttons == 0 {
                        6i32 // kHover
                    } else {
                        3i32 // kMove (drag)
                    };
                    let rc = send(phase, buttons);
                    write(b"[embedder/ptr] send phase=");
                    write_dec(phase as u64);
                    write(b" x=");
                    write_dec(x as u64);
                    write(b" y=");
                    write_dec(y as u64);
                    write(b" buttons=");
                    write_dec(buttons as u64);
                    write(b" rc=");
                    write_dec(rc as u64);
                    write(b"\n");
                }
            }
            EV_KEY => {
                let scancode = ev.a as u32;
                let pressed = (ev.flags & 1) != 0;
                if engine_out != 0 && proctable.send_key_event != 0 {
                    let evt = FlutterKeyEvent {
                        struct_size: core::mem::size_of::<FlutterKeyEvent>(),
                        timestamp: (rdtsc_ns() / 1_000_000) as f64,
                        kind: if pressed { 0 } else { 1 },
                        _pad: 0,
                        physical: scancode as u64,
                        logical: scancode as u64,
                        character: 0,
                        synthesized: false,
                        _tail: [0; 7],
                    };
                    let f: SendKeyEventFn =
                        unsafe { core::mem::transmute(proctable.send_key_event) };
                    unsafe { f(engine_out, &evt as *const _, 0, 0) };
                }
            }
            EV_PLATFORM_MSG => {
                // A native kernel module sent us a platform-channel message.
                let _seq = ev.a;
                let _channel_hash = ev.flags;
                let n = platform_msg_recv(&mut platform_buf);
                if n > 8 + 2 + 4 {
                    let seq = u64::from_le_bytes(platform_buf[0..8].try_into().unwrap_or([0; 8]));
                    // Echo-reply OK.
                    platform_msg_reply(seq, b"ok");
                }
            }
            _ => {}
        }
    }
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

fn write_hex(mut v: u64) {
    let mut buf = [0u8; 18];
    buf[0] = b'0';
    buf[1] = b'x';
    let digits = b"0123456789abcdef";
    for i in 0..16 {
        let nyb = ((v >> ((15 - i) * 4)) & 0xF) as usize;
        buf[2 + i] = digits[nyb];
    }
    write(&buf);
}

fn configure_project_assets(project_args: &mut FlutterProjectArgsRaw, path: &[u8]) {
    static mut ASSETS_PATH_BUF: [u8; 256] = [0; 256];
    unsafe {
        let len = path.len().min(255);
        ASSETS_PATH_BUF[..len].copy_from_slice(&path[..len]);
        ASSETS_PATH_BUF[len] = 0;
        write_u64_at(&mut project_args.bytes, OFF_PROJECT_ARGS_ASSETS_PATH, ASSETS_PATH_BUF.as_ptr() as u64);
    }
}

fn configure_app_assets(project_args: &mut FlutterProjectArgsRaw, app_id: u64) {
    let mut records = [0u8; 4096];
    let count = sys::app_list(&mut records) as usize;
    static mut APP_ASSETS_PATH_BUF: [u8; 256] = [0; 256];
    
    let mut found = false;
    for i in 0..count {
        let off = i * 88;
        if off + 88 > records.len() {
            break;
        }
        let id = u32::from_le_bytes(records[off..off + 4].try_into().unwrap_or([0; 4]));
        if id as u64 == app_id {
            let name_end = records[off + 4..off + 68]
                .iter()
                .position(|&b| b == 0)
                .unwrap_or(64);
            let name = &records[off + 4..off + 4 + name_end];
            
            unsafe {
                let prefix = b"/Applications/";
                let suffix = b".app/flutter_assets";
                let mut pos = 0usize;
                
                APP_ASSETS_PATH_BUF[pos..pos + prefix.len()].copy_from_slice(prefix);
                pos += prefix.len();
                
                let copy_len = name.len().min(APP_ASSETS_PATH_BUF.len() - pos - suffix.len() - 1);
                APP_ASSETS_PATH_BUF[pos..pos + copy_len].copy_from_slice(&name[..copy_len]);
                pos += copy_len;
                
                APP_ASSETS_PATH_BUF[pos..pos + suffix.len()].copy_from_slice(suffix);
                pos += suffix.len();
                
                APP_ASSETS_PATH_BUF[pos] = 0;
                
                write_u64_at(&mut project_args.bytes, OFF_PROJECT_ARGS_ASSETS_PATH, APP_ASSETS_PATH_BUF.as_ptr() as u64);
            }
            found = true;
            break;
        }
    }
    if !found {
        write(b"[embedder] WARNING: app_id ");
        write_dec(app_id);
        write(b" not found in registry\n");
    }
}

fn configure_aot_snapshots(project_args: &mut FlutterProjectArgsRaw, aot_va: u64) {
    let opt = if aot_va == 0 {
        aot_loader::load_dart_snapshot(b"/system/flutter/libapp.so")
    } else {
        aot_loader::load_dart_snapshot_from_mapping(aot_va)
    };
    if let Some((ptrs, _loaded_va)) = opt {
        write_u64_at(&mut project_args.bytes, OFF_PA_VM_SNAPSHOT_DATA, ptrs.vm_data);
        write_u64_at(&mut project_args.bytes, OFF_PA_VM_SNAPSHOT_DATA_SIZE, ptrs.vm_data_size);
        write_u64_at(&mut project_args.bytes, OFF_PA_VM_SNAPSHOT_INSTRUCTIONS, ptrs.vm_instr);
        write_u64_at(&mut project_args.bytes, OFF_PA_VM_SNAPSHOT_INSTRUCTIONS_SIZE, ptrs.vm_instr_size);
        write_u64_at(&mut project_args.bytes, OFF_PA_ISO_SNAPSHOT_DATA, ptrs.iso_data);
        write_u64_at(&mut project_args.bytes, OFF_PA_ISO_SNAPSHOT_DATA_SIZE, ptrs.iso_data_size);
        write_u64_at(&mut project_args.bytes, OFF_PA_ISO_SNAPSHOT_INSTRUCTIONS, ptrs.iso_instr);
        write_u64_at(&mut project_args.bytes, OFF_PA_ISO_SNAPSHOT_INSTRUCTIONS_SIZE, ptrs.iso_instr_size);
        write(b"[embedder/aot] SNAPSHOT PTRS:\n");
        write_hex_u64(b"[embedder/aot]   vm_data  =", ptrs.vm_data);
        write_hex_u64(b"[embedder/aot]   vm_instr =", ptrs.vm_instr);
        write_hex_u64(b"[embedder/aot]   vm_instr_size =", ptrs.vm_instr_size);
        write_hex_u64(b"[embedder/aot]   iso_data =", ptrs.iso_data);
        write_hex_u64(b"[embedder/aot]   iso_instr=", ptrs.iso_instr);
        write_hex_u64(b"[embedder/aot]   iso_instr_size=", ptrs.iso_instr_size);
    } else {
        write(b"[embedder] ERROR: failed to load Dart AOT snapshot\n");
    }
}

fn schedule_frame_with_log(engine: u64, schedule_frame_va: u64, label: &[u8]) -> i32 {
    write(b"[embedder/sched] schedule_frame entry label=");
    write(label);
    write(b"\n");
    
    let func: unsafe extern "C" fn(u64) -> i32 = unsafe { core::mem::transmute(schedule_frame_va) };
    let rc = unsafe { func(engine) };
    
    write(b"[embedder/sched] schedule_frame exit rc=");
    write_dec(rc as u64);
    write(b"\n");
    rc
}

// ── Panic handler ─────────────────────────────────────────────────────────────

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    write(b"[embedder] PANIC\n");
    exit(1)
}
