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
#[cfg(target_arch = "x86_64")]
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
            // Preserve the kernel's bootstrap registers (rdi=host_mode,
            // rsi=app_id, rdx=aot_va) across the breadcrumb syscall below.
            // SYS_WRITE clobbers rdi/rsi/rdx, which made read_host_bootstrap()
            // see rdi=1 (the write fd) for EVERY process — so a launched app
            // (kernel sets rdi=HOST_MODE_APP=2) booted as a second SHELL and
            // loaded the shell's blob instead of the app's. r12-r14 are
            // callee-saved and survive the syscall.
            "mov r12, rdi",
            "mov r13, rsi",
            "mov r14, rdx",
            // Earliest possible userspace breadcrumb: write one byte to serial
            // before calling into Rust code.
            "mov rax, 1",
            "mov rdi, 1",
            "lea rsi, [rip + 2f]",
            "mov rdx, 1",
            "syscall",
            // Restore the bootstrap registers for main_embedder.
            "mov rdi, r12",
            "mov rsi, r13",
            "mov rdx, r14",
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

/// aarch64 userspace entry. The kernel enters EL0 with x0=host_mode, x1=app_id,
/// x2=aot_va (the SpawnBootstrap rdi/rsi/rdx mapped to x0/x1/x2 by
/// `enter_user.rs::build_image`) and SP pointing at a fresh 16-byte-aligned user
/// stack. We preserve x0..x2 across the breadcrumb write (callee-saved x19..x21
/// survive the `svc`), then call `main_embedder(x0, x1, x2)`.
#[cfg(target_arch = "aarch64")]
#[unsafe(no_mangle)]
#[unsafe(naked)]
unsafe extern "C" fn _start() -> ! {
    unsafe {
        core::arch::naked_asm!(
            // Zero the frame pointer (x29) — terminates backtraces cleanly.
            "mov x29, #0",
            "mov x30, #0",
            // AAPCS64 requires SP to be 16-byte aligned. SP cannot be a direct
            // operand of AND, so round it down via a scratch GPR (x9).
            "mov x9, sp",
            "and x9, x9, #-16",
            "mov sp, x9",
            // Preserve the bootstrap args (x0=host_mode, x1=app_id, x2=aot_va)
            // across the breadcrumb write syscall, which clobbers x0..x2.
            "mov x19, x0",
            "mov x20, x1",
            "mov x21, x2",
            // Earliest userspace breadcrumb: write one '!' byte to fd 1.
            "mov x8, #1",            // SYS_WRITE
            "mov x0, #1",            // fd = 1
            "adr x1, 2f",            // buf = label 2
            "mov x2, #1",            // len = 1
            "svc #0",
            // Restore the bootstrap args for main_embedder(x0, x1, x2).
            "mov x0, x19",
            "mov x1, x20",
            "mov x2, x21",
            "bl {main}",
            // If main returns, exit(0).
            "mov x8, #60",           // SYS_EXIT
            "mov x0, #0",
            "svc #0",
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

// ════════════════════════════════════════════════════════════════════════════
// Semantics / accessibility tree.
//
// The engine produces semantics updates only after the embedder enables them
// (FlutterEngineUpdateSemanticsEnabled) AND provides update_semantics_callback2.
// We keep the LIVE tree here so an accessibility / UI-automation consumer (a
// screen reader, a test harness) can read the current node set: each node's id,
// label, screen rect, flags and actions. This makes the semantics pipeline real
// rather than dead — Flutter computes it, we receive and retain it.
// ════════════════════════════════════════════════════════════════════════════

const MAX_SEMANTICS_NODES: usize = 256;
const SEMANTICS_LABEL_CAP: usize = 96;

#[derive(Clone, Copy)]
struct SemanticsNode {
    id: i32,
    flags: u32,
    actions: u32,
    // Bounding rect in logical pixels (left, top, right, bottom).
    rect: [f64; 4],
    label_len: u16,
    label: [u8; SEMANTICS_LABEL_CAP],
}

impl SemanticsNode {
    const fn empty() -> Self {
        Self {
            id: -1,
            flags: 0,
            actions: 0,
            rect: [0.0; 4],
            label_len: 0,
            label: [0u8; SEMANTICS_LABEL_CAP],
        }
    }
}

struct SemanticsTree {
    count: usize,
    nodes: [SemanticsNode; MAX_SEMANTICS_NODES],
}

static mut SEMANTICS_TREE: SemanticsTree = SemanticsTree {
    count: 0,
    nodes: [SemanticsNode::empty(); MAX_SEMANTICS_NODES],
};
static SEMANTICS_ENABLED: core::sync::atomic::AtomicBool =
    core::sync::atomic::AtomicBool::new(false);
static SEMANTICS_UPDATES: AtomicU32 = AtomicU32::new(0);

// Field offsets within FlutterSemanticsNode2 (verified against
// tools/flutter-engine/flutter_embedder.h, x86_64 layout).
const OFF_SN2_ID: usize = 8;
const OFF_SN2_FLAGS: usize = 12;
const OFF_SN2_ACTIONS: usize = 16;
const OFF_SN2_LABEL: usize = 80; // const char*
const OFF_SN2_RECT: usize = 128; // FlutterRect { double left, top, right, bottom }

// Field offsets within FlutterSemanticsUpdate2.
const OFF_SU2_NODE_COUNT: usize = 8;
const OFF_SU2_NODES: usize = 16; // FlutterSemanticsNode2**

#[inline]
fn read_u64_ptr(base: u64, off: usize) -> u64 {
    unsafe { core::ptr::read_unaligned((base as usize + off) as *const u64) }
}
#[inline]
fn read_i32_ptr(base: u64, off: usize) -> i32 {
    unsafe { core::ptr::read_unaligned((base as usize + off) as *const i32) }
}
#[inline]
fn read_u32_ptr(base: u64, off: usize) -> u32 {
    unsafe { core::ptr::read_unaligned((base as usize + off) as *const u32) }
}
#[inline]
fn read_f64_ptr(base: u64, off: usize) -> f64 {
    unsafe { core::ptr::read_unaligned((base as usize + off) as *const f64) }
}

/// `update_semantics_callback2`: receives a `FlutterSemanticsUpdate2*` and
/// stores the node set into the embedder's live semantics tree.
unsafe extern "C" fn update_semantics_callback2(update: *const u8, _user_data: *mut ()) {
    if update.is_null() {
        return;
    }
    let upd = update as u64;
    let node_count = read_u64_ptr(upd, OFF_SU2_NODE_COUNT) as usize;
    let nodes_arr = read_u64_ptr(upd, OFF_SU2_NODES); // FlutterSemanticsNode2**
    if nodes_arr == 0 {
        return;
    }

    let n = node_count.min(MAX_SEMANTICS_NODES);
    let tree = unsafe { &mut *core::ptr::addr_of_mut!(SEMANTICS_TREE) };
    for i in 0..n {
        // The update carries an array of POINTERS to nodes.
        let node_ptr = unsafe {
            core::ptr::read_unaligned((nodes_arr as usize + i * 8) as *const u64)
        };
        if node_ptr == 0 {
            continue;
        }
        let dst = &mut tree.nodes[i];
        dst.id = read_i32_ptr(node_ptr, OFF_SN2_ID);
        dst.flags = read_u32_ptr(node_ptr, OFF_SN2_FLAGS);
        dst.actions = read_u32_ptr(node_ptr, OFF_SN2_ACTIONS);
        dst.rect[0] = read_f64_ptr(node_ptr, OFF_SN2_RECT);
        dst.rect[1] = read_f64_ptr(node_ptr, OFF_SN2_RECT + 8);
        dst.rect[2] = read_f64_ptr(node_ptr, OFF_SN2_RECT + 16);
        dst.rect[3] = read_f64_ptr(node_ptr, OFF_SN2_RECT + 24);
        // Copy the label string (NUL-terminated C string) up to our cap.
        let label_ptr = read_u64_ptr(node_ptr, OFF_SN2_LABEL);
        let mut llen = 0usize;
        if label_ptr != 0 {
            while llen < SEMANTICS_LABEL_CAP {
                let c = unsafe { *((label_ptr as usize + llen) as *const u8) };
                if c == 0 {
                    break;
                }
                dst.label[llen] = c;
                llen += 1;
            }
        }
        dst.label_len = llen as u16;
    }
    tree.count = n;

    let seq = SEMANTICS_UPDATES.fetch_add(1, Ordering::Relaxed);
    if seq < 8 {
        write(b"[embedder/semantics] update nodes=");
        write_dec(node_count as u64);
        write(b" stored=");
        write_dec(n as u64);
        write(b"\n");
    }
}

const OFF_PROJECT_ARGS_UPDATE_SEMANTICS_CALLBACK2: usize = 280;

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
/// `__FlutterEngineFlushPendingTasksNow` VA — drains queued platform tasks
/// (including pending message replies) on the current thread.
static FLUSH_PENDING_FN: AtomicU64 = AtomicU64::new(0);
/// `FlutterEngineUpdateSemanticsEnabled` VA — toggles semantics production.
static UPDATE_SEMANTICS_ENABLED_FN: AtomicU64 = AtomicU64::new(0);
static CURRENT_HOST_MODE: AtomicU64 = AtomicU64::new(0);
static CURRENT_APP_ID: AtomicU64 = AtomicU64::new(0);

// ── User-configurable input settings (driven by the Settings UI over the
// oscortex/shell channel: "config:scroll_invert:0|1", "config:scroll_speed:NN").
// These live in the embedder because it owns the FlutterPointerEvent it builds,
// so no kernel round-trip is needed for scroll feel. Defaults match the
// previously hard-coded behaviour (reverse/natural off, 100% speed).
static SCROLL_INVERT: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);
static SCROLL_SPEED_PCT: AtomicU32 = AtomicU32::new(100);

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

/// Drain queued engine platform tasks (incl. pending message replies) on the
/// current thread. Used before exiting an app on SystemNavigator.pop so the
/// final reply is actually delivered.
fn flush_engine_tasks() {
    let va = FLUSH_PENDING_FN.load(Ordering::SeqCst);
    if va != 0 {
        let f: unsafe extern "C" fn() = unsafe { core::mem::transmute(va) };
        unsafe { f() };
    }
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

    dispatch_platform_channel(channel_slice, msg);
}

/// JSONMethodCodec success envelope carrying a `null` result: a one-element
/// JSON list `[null]`. Distinct from `METHOD_SUCCESS_NULL` (StandardMethodCodec,
/// the `[0x00, 0x00]` two-byte form). Sending the wrong codec's null makes the
/// framework throw "Expected envelope List" on JSON channels.
const JSON_SUCCESS_NULL: &[u8] = b"[null]";

/// Real channel dispatcher. Every channel Flutter reaches for is routed to a
/// concrete handler that either does the platform work or returns a correctly
/// typed ack. The final catch-all logs the channel name so nothing stays an
/// invisible stub — `[embedder/chan] unbound: <name>` on serial.
fn dispatch_platform_channel(channel: &[u8], msg: &FlutterPlatformMessage) {
    match channel {
        // ── OSCortex app channels (custom string codec) ──────────────────
        b"oscortex/apps/request" => {
            respond_platform_message(msg, METHOD_SUCCESS_NULL);
        }
        b"oscortex/shell" => {
            handle_shell_platform_message(msg);
        }

        // ── Text input (JSONMethodCodec) — THE critical channel ──────────
        b"flutter/textinput" => {
            handle_textinput_channel(msg);
        }

        // ── Mouse cursor (JSONMethodCodec) ───────────────────────────────
        b"flutter/mousecursor" => {
            handle_mousecursor_channel(msg);
        }

        // ── Platform services (JSONMethodCodec): clipboard, nav, sound… ──
        b"flutter/platform" => {
            handle_platform_channel(msg);
        }

        // ── Graceful-ack JSON channels: the framework only needs a valid
        //    `[null]` envelope back. We still route them explicitly so the
        //    catch-all log only ever fires for something genuinely new.
        // flutter/accessibility is a BasicMessageChannel using StandardMessageCodec
        // (announce/tap/longPress/tooltip events). A StandardMessageCodec null
        // reply is a single 0x00 byte — NOT the JSON `[null]`. Reply correctly.
        b"flutter/accessibility" => {
            respond_platform_message(msg, &[0x00u8]);
        }

        b"flutter/navigation"
        | b"flutter/system"
        | b"flutter/spellcheck"
        | b"flutter/processtext"
        | b"flutter/menu"
        | b"flutter/contextmenu"
        | b"flutter/scribe"
        | b"flutter/restoration"
        | b"flutter/keyevent"
        | b"flutter/platform_views"
        | b"flutter/isolate"
        | b"flutter/lifecycle" => {
            respond_platform_message(msg, JSON_SUCCESS_NULL);
        }

        // ── Anything else: reply with the codec-correct null AND log it so
        //    an unbound channel is never silent. ─────────────────────────
        _ => {
            write(b"[embedder/chan] unbound: ");
            write(channel);
            write(b"\n");
            if msg.response_handle != 0 {
                respond_platform_message(msg, default_platform_reply(msg));
            }
        }
    }
}

// ════════════════════════════════════════════════════════════════════════════
// Flutter framework platform channels (text input, cursor, clipboard, …)
//
// These all use JSONMethodCodec: a message is the JSON text of a 2-element list
// `["MethodName", arguments]`; a success reply is `[result]` and an error reply
// is `[code, message, details]`. We implement just enough of a JSON
// reader/writer to decode method calls and the editing-state map, and to encode
// the `TextInputClient.updateEditingState` call we push back to the framework.
// ════════════════════════════════════════════════════════════════════════════

// ── Minimal JSON scanning helpers (no allocation, byte-slice based) ──────────

/// Skip ASCII JSON whitespace, returning the index of the next significant byte.
fn json_skip_ws(buf: &[u8], mut i: usize) -> usize {
    while i < buf.len() {
        match buf[i] {
            b' ' | b'\t' | b'\n' | b'\r' => i += 1,
            _ => break,
        }
    }
    i
}

/// Parse a JSON string starting at `buf[start]` (which must be the opening `"`).
/// Decodes into `out`, returning (bytes_written, index_after_closing_quote).
/// Handles the escape sequences Flutter's JSON codec emits: \" \\ \/ \b \f \n
/// \r \t and \uXXXX (BMP). Returns None on malformed input.
fn json_parse_string(buf: &[u8], start: usize, out: &mut [u8]) -> Option<(usize, usize)> {
    if start >= buf.len() || buf[start] != b'"' {
        return None;
    }
    let mut i = start + 1;
    let mut w = 0usize;
    while i < buf.len() {
        let c = buf[i];
        if c == b'"' {
            return Some((w, i + 1));
        }
        if c == b'\\' {
            i += 1;
            if i >= buf.len() {
                return None;
            }
            let e = buf[i];
            let decoded: u32 = match e {
                b'"' => b'"' as u32,
                b'\\' => b'\\' as u32,
                b'/' => b'/' as u32,
                b'b' => 0x08,
                b'f' => 0x0c,
                b'n' => b'\n' as u32,
                b'r' => b'\r' as u32,
                b't' => b'\t' as u32,
                b'u' => {
                    if i + 4 >= buf.len() {
                        return None;
                    }
                    let mut cp = 0u32;
                    for k in 1..=4 {
                        cp = (cp << 4) | hex_nyb(buf[i + k])? as u32;
                    }
                    i += 4;
                    cp
                }
                _ => return None,
            };
            i += 1;
            w += encode_utf8(decoded, &mut out[w..]);
        } else {
            if w < out.len() {
                out[w] = c;
                w += 1;
            }
            i += 1;
        }
    }
    None
}

fn hex_nyb(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

/// Encode a Unicode scalar value as UTF-8 into `out`, returning byte count.
fn encode_utf8(cp: u32, out: &mut [u8]) -> usize {
    if cp < 0x80 {
        if !out.is_empty() {
            out[0] = cp as u8;
            return 1;
        }
        0
    } else if cp < 0x800 {
        if out.len() >= 2 {
            out[0] = 0xC0 | (cp >> 6) as u8;
            out[1] = 0x80 | (cp & 0x3F) as u8;
            return 2;
        }
        0
    } else if cp < 0x10000 {
        if out.len() >= 3 {
            out[0] = 0xE0 | (cp >> 12) as u8;
            out[1] = 0x80 | ((cp >> 6) & 0x3F) as u8;
            out[2] = 0x80 | (cp & 0x3F) as u8;
            return 3;
        }
        0
    } else {
        if out.len() >= 4 {
            out[0] = 0xF0 | (cp >> 18) as u8;
            out[1] = 0x80 | ((cp >> 12) & 0x3F) as u8;
            out[2] = 0x80 | ((cp >> 6) & 0x3F) as u8;
            out[3] = 0x80 | (cp & 0x3F) as u8;
            return 4;
        }
        0
    }
}

/// Extract the method name from a JSONMethodCodec message `["Method", args]`.
/// Returns (method_bytes_in_scratch_len, index_just_past_the_method_string).
/// `scratch` receives the decoded method name.
fn json_method_name(buf: &[u8], scratch: &mut [u8]) -> Option<(usize, usize)> {
    let mut i = json_skip_ws(buf, 0);
    if i >= buf.len() || buf[i] != b'[' {
        return None;
    }
    i = json_skip_ws(buf, i + 1);
    let (len, next) = json_parse_string(buf, i, scratch)?;
    Some((len, next))
}

/// Locate the value following the first occurrence of an object key `"key":`
/// within `buf`. Returns the index of the first byte of the value, or None.
/// This is a flat scan — adequate for the small, well-formed maps the framework
/// sends (editing state, cursor args, clipboard payload).
fn json_find_value(buf: &[u8], key: &[u8]) -> Option<usize> {
    // Build the search pattern: "key"
    let mut i = 0usize;
    while i + key.len() + 2 <= buf.len() {
        if buf[i] == b'"'
            && buf[i + 1..].starts_with(key)
            && buf.get(i + 1 + key.len()) == Some(&b'"')
        {
            // Found the key string; skip to ':' then to the value.
            let mut j = i + 2 + key.len();
            j = json_skip_ws(buf, j);
            if j < buf.len() && buf[j] == b':' {
                return Some(json_skip_ws(buf, j + 1));
            }
        }
        i += 1;
    }
    None
}

/// Parse a (possibly negative) JSON integer at `buf[start]`. Returns the value.
fn json_parse_int(buf: &[u8], start: usize) -> Option<i64> {
    let mut i = start;
    let mut neg = false;
    if i < buf.len() && buf[i] == b'-' {
        neg = true;
        i += 1;
    }
    let mut v: i64 = 0;
    let mut any = false;
    while i < buf.len() && buf[i].is_ascii_digit() {
        v = v.saturating_mul(10).saturating_add((buf[i] - b'0') as i64);
        i += 1;
        any = true;
    }
    if !any {
        return None;
    }
    Some(if neg { -v } else { v })
}

/// Append `s` to `out` at `*pos`, advancing `*pos`. Truncates on overflow.
fn json_push(out: &mut [u8], pos: &mut usize, s: &[u8]) {
    let n = s.len().min(out.len().saturating_sub(*pos));
    out[*pos..*pos + n].copy_from_slice(&s[..n]);
    *pos += n;
}

/// Append `v` formatted as decimal to `out` at `*pos`.
fn json_push_int(out: &mut [u8], pos: &mut usize, v: i64) {
    let mut tmp = [0u8; 20];
    let mut neg = false;
    let mut uv = if v < 0 {
        neg = true;
        (v as i128).unsigned_abs() as u64
    } else {
        v as u64
    };
    let mut n = 0usize;
    if uv == 0 {
        tmp[0] = b'0';
        n = 1;
    } else {
        while uv > 0 {
            tmp[n] = b'0' + (uv % 10) as u8;
            uv /= 10;
            n += 1;
        }
    }
    if neg {
        json_push(out, pos, b"-");
    }
    while n > 0 {
        n -= 1;
        json_push(out, pos, core::slice::from_ref(&tmp[n]));
    }
}

/// Append `s` as a JSON string literal (with quotes + escaping) to `out`.
fn json_push_str_escaped(out: &mut [u8], pos: &mut usize, s: &[u8]) {
    json_push(out, pos, b"\"");
    for &b in s {
        if *pos + 6 >= out.len() {
            break;
        }
        match b {
            b'"' => json_push(out, pos, b"\\\""),
            b'\\' => json_push(out, pos, b"\\\\"),
            b'\n' => json_push(out, pos, b"\\n"),
            b'\r' => json_push(out, pos, b"\\r"),
            b'\t' => json_push(out, pos, b"\\t"),
            0x00..=0x1f => {
                // Control char → \u00XX
                json_push(out, pos, b"\\u00");
                let hi = b >> 4;
                let lo = b & 0xf;
                let digits = b"0123456789abcdef";
                json_push(out, pos, core::slice::from_ref(&digits[hi as usize]));
                json_push(out, pos, core::slice::from_ref(&digits[lo as usize]));
            }
            _ => json_push(out, pos, core::slice::from_ref(&b)),
        }
    }
    json_push(out, pos, b"\"");
}

// ── Text input state ─────────────────────────────────────────────────────────

const TEXT_BUF_CAP: usize = 1024;

/// The framework's active text-input client and its editing state, maintained
/// entirely in the embedder. `selection_base`/`selection_extent` are byte
/// offsets into `text[..text_len]` (we keep text as UTF-8, no multi-byte input
/// from a US PS/2 keyboard so offsets stay byte == codepoint here).
struct TextInputState {
    client_id: i64,
    active: bool,
    focused: bool,
    text: [u8; TEXT_BUF_CAP],
    text_len: usize,
    selection_base: i64,
    selection_extent: i64,
}

static mut TEXT_INPUT: TextInputState = TextInputState {
    client_id: -1,
    active: false,
    focused: false,
    text: [0; TEXT_BUF_CAP],
    text_len: 0,
    selection_base: 0,
    selection_extent: 0,
};
static TEXT_INPUT_LOCK: Spinlock = Spinlock::new();

fn handle_textinput_channel(msg: &FlutterPlatformMessage) {
    let payload = if msg.message != 0 && msg.message_size != 0 {
        unsafe { core::slice::from_raw_parts(msg.message as *const u8, msg.message_size) }
    } else {
        respond_platform_message(msg, JSON_SUCCESS_NULL);
        return;
    };

    let mut method = [0u8; 64];
    let (mlen, args_start) = match json_method_name(payload, &mut method) {
        Some(v) => v,
        None => {
            respond_platform_message(msg, JSON_SUCCESS_NULL);
            return;
        }
    };
    let method = &method[..mlen];

    let _guard = TEXT_INPUT_LOCK.lock();
    let st = unsafe { &mut *core::ptr::addr_of_mut!(TEXT_INPUT) };

    static TI_LOG: AtomicU32 = AtomicU32::new(0);
    if TI_LOG.fetch_add(1, Ordering::Relaxed) < 24 {
        write(b"[embedder/textinput] ");
        write(method);
        write(b"\n");
    }

    match method {
        b"TextInput.setClient" => {
            // args = [clientId, config]
            if let Some(cid) = json_parse_int(payload, json_skip_ws(payload, args_start + 1)) {
                st.client_id = cid;
                st.active = true;
            }
        }
        b"TextInput.setEditingState" => {
            // args = { text, selectionBase, selectionExtent, ... }
            apply_editing_state(st, &payload[args_start..]);
        }
        b"TextInput.show" => {
            st.focused = true;
        }
        b"TextInput.hide" => {
            st.focused = false;
        }
        b"TextInput.clearClient" => {
            st.active = false;
            st.focused = false;
            st.client_id = -1;
            st.text_len = 0;
            st.selection_base = 0;
            st.selection_extent = 0;
        }
        // setEditableSizeAndTransform, setStyle, requestAutofill,
        // setCaretRect, finishAutofillContext, etc. — no platform work needed.
        _ => {}
    }

    respond_platform_message(msg, JSON_SUCCESS_NULL);
}

/// Load `text`/`selectionBase`/`selectionExtent` from a JSON editing-state map
/// into the embedder's state.
fn apply_editing_state(st: &mut TextInputState, map: &[u8]) {
    if let Some(vi) = json_find_value(map, b"text") {
        if map.get(vi) == Some(&b'"') {
            let mut tmp = [0u8; TEXT_BUF_CAP];
            if let Some((len, _)) = json_parse_string(map, vi, &mut tmp) {
                let len = len.min(TEXT_BUF_CAP);
                st.text[..len].copy_from_slice(&tmp[..len]);
                st.text_len = len;
            }
        }
    }
    if let Some(vi) = json_find_value(map, b"selectionBase") {
        if let Some(v) = json_parse_int(map, vi) {
            st.selection_base = v;
        }
    }
    if let Some(vi) = json_find_value(map, b"selectionExtent") {
        if let Some(v) = json_parse_int(map, vi) {
            st.selection_extent = v;
        }
    }
    clamp_selection(st);
}

fn clamp_selection(st: &mut TextInputState) {
    let max = st.text_len as i64;
    if st.selection_base < 0 || st.selection_base > max {
        st.selection_base = max;
    }
    if st.selection_extent < 0 || st.selection_extent > max {
        st.selection_extent = max;
    }
}

static mut UPDATE_EDITING_BUF: [u8; 2048] = [0; 2048];
static UPDATE_EDITING_CH: &[u8] = b"flutter/textinput\0";

/// Encode and send `TextInputClient.updateEditingState` back to the framework
/// over flutter/textinput. Must be called with the engine running and the
/// TEXT_INPUT_LOCK held by the caller (it reads `st`).
fn send_update_editing_state(st: &TextInputState) {
    let send_fn_va = SEND_PLATFORM_MESSAGE_FN.load(Ordering::SeqCst);
    let engine = ENGINE.load(Ordering::SeqCst);
    if send_fn_va == 0 || engine == 0 || !st.active {
        return;
    }

    let out = unsafe { &mut *core::ptr::addr_of_mut!(UPDATE_EDITING_BUF) };
    let mut pos = 0usize;
    // ["TextInputClient.updateEditingState", [clientId, {state}]]
    json_push(out, &mut pos, b"[\"TextInputClient.updateEditingState\",[");
    json_push_int(out, &mut pos, st.client_id);
    json_push(out, &mut pos, b",{\"text\":");
    json_push_str_escaped(out, &mut pos, &st.text[..st.text_len.min(TEXT_BUF_CAP)]);
    json_push(out, &mut pos, b",\"selectionBase\":");
    json_push_int(out, &mut pos, st.selection_base);
    json_push(out, &mut pos, b",\"selectionExtent\":");
    json_push_int(out, &mut pos, st.selection_extent);
    json_push(
        out,
        &mut pos,
        b",\"selectionAffinity\":\"TextAffinity.downstream\",\"selectionIsDirectional\":false,\"composingBase\":-1,\"composingExtent\":-1}]]",
    );

    let msg = FlutterPlatformMessage {
        struct_size: core::mem::size_of::<FlutterPlatformMessage>(),
        channel: UPDATE_EDITING_CH.as_ptr() as u64,
        message: out.as_ptr() as u64,
        message_size: pos,
        response_handle: 0,
    };
    let send: SendPlatformMessageFn = unsafe { core::mem::transmute(send_fn_va) };
    let _ = unsafe { send(engine, &msg as *const _) };
}

// ── PS/2 set-1 scancode → editing action ─────────────────────────────────────

/// What a key should do to the editing state.
enum KeyAction {
    Insert(u32), // unicode codepoint
    Backspace,
    Delete,
    Left,
    Right,
    Home,
    End,
    None,
}

/// Modifier latch state for the keyboard, tracked across EV_KEY events.
struct KbdMods {
    shift: bool,
    caps: bool,
    ctrl: bool,
}

static mut KBD_MODS: KbdMods = KbdMods {
    shift: false,
    caps: false,
    ctrl: false,
};

/// Map a PS/2 set-1 scancode (as delivered by the kernel: low 7 bits of the
/// make code, extended keys carrying the 0xE000 prefix) plus current modifiers
/// to an editing action. Only handles a make (press); the caller gates on
/// `pressed`.
fn scancode_to_action(scancode: u32, mods: &KbdMods) -> KeyAction {
    // Extended (E0) navigation keys.
    match scancode {
        0xE04B => return KeyAction::Left,  // arrow left
        0xE04D => return KeyAction::Right, // arrow right
        0xE047 => return KeyAction::Home,  // Home
        0xE04F => return KeyAction::End,   // End
        0xE053 => return KeyAction::Delete, // Delete
        _ => {}
    }
    match scancode {
        0x0E => return KeyAction::Backspace, // Backspace
        0x1C => return KeyAction::Insert(b'\n' as u32), // Enter
        0x0F => return KeyAction::Insert(b'\t' as u32), // Tab
        0x39 => return KeyAction::Insert(b' ' as u32),  // Space
        0x4B => return KeyAction::Left,  // keypad-translated left (some sets)
        0x4D => return KeyAction::Right,
        0x47 => return KeyAction::Home,
        0x4F => return KeyAction::End,
        0x53 => return KeyAction::Delete,
        _ => {}
    }
    // Ctrl chords don't produce text here.
    if mods.ctrl {
        return KeyAction::None;
    }
    if let Some(ch) = scancode_to_char(scancode, mods.shift, mods.caps) {
        return KeyAction::Insert(ch as u32);
    }
    KeyAction::None
}

/// Map a PS/2 set-1 scancode to an ASCII character given shift/caps. Returns
/// None for non-printable / unmapped keys (US QWERTY layout).
fn scancode_to_char(sc: u32, shift: bool, caps: bool) -> Option<u8> {
    // (unshifted, shifted)
    let pair: (u8, u8) = match sc {
        0x02 => (b'1', b'!'),
        0x03 => (b'2', b'@'),
        0x04 => (b'3', b'#'),
        0x05 => (b'4', b'$'),
        0x06 => (b'5', b'%'),
        0x07 => (b'6', b'^'),
        0x08 => (b'7', b'&'),
        0x09 => (b'8', b'*'),
        0x0A => (b'9', b'('),
        0x0B => (b'0', b')'),
        0x0C => (b'-', b'_'),
        0x0D => (b'=', b'+'),
        0x10 => (b'q', b'Q'),
        0x11 => (b'w', b'W'),
        0x12 => (b'e', b'E'),
        0x13 => (b'r', b'R'),
        0x14 => (b't', b'T'),
        0x15 => (b'y', b'Y'),
        0x16 => (b'u', b'U'),
        0x17 => (b'i', b'I'),
        0x18 => (b'o', b'O'),
        0x19 => (b'p', b'P'),
        0x1A => (b'[', b'{'),
        0x1B => (b']', b'}'),
        0x1E => (b'a', b'A'),
        0x1F => (b's', b'S'),
        0x20 => (b'd', b'D'),
        0x21 => (b'f', b'F'),
        0x22 => (b'g', b'G'),
        0x23 => (b'h', b'H'),
        0x24 => (b'j', b'J'),
        0x25 => (b'k', b'K'),
        0x26 => (b'l', b'L'),
        0x27 => (b';', b':'),
        0x28 => (b'\'', b'"'),
        0x29 => (b'`', b'~'),
        0x2B => (b'\\', b'|'),
        0x2C => (b'z', b'Z'),
        0x2D => (b'x', b'X'),
        0x2E => (b'c', b'C'),
        0x2F => (b'v', b'V'),
        0x30 => (b'b', b'B'),
        0x31 => (b'n', b'N'),
        0x32 => (b'm', b'M'),
        0x33 => (b',', b'<'),
        0x34 => (b'.', b'>'),
        0x35 => (b'/', b'?'),
        _ => return None,
    };
    let is_alpha = pair.0.is_ascii_lowercase();
    // Letters: caps XOR shift selects upper. Symbols/digits: shift only.
    let upper = if is_alpha { shift ^ caps } else { shift };
    Some(if upper { pair.1 } else { pair.0 })
}

/// Update the modifier latches from a key event. Returns true if the scancode
/// was a pure modifier key (no further text handling required).
fn update_modifiers(scancode: u32, pressed: bool, mods: &mut KbdMods) -> bool {
    match scancode {
        0x2A | 0x36 => {
            // Left / right shift
            mods.shift = pressed;
            true
        }
        0x1D | 0xE01D => {
            // Left / right ctrl
            mods.ctrl = pressed;
            true
        }
        0x3A => {
            // Caps lock toggles on press only
            if pressed {
                mods.caps = !mods.caps;
            }
            true
        }
        _ => false,
    }
}

/// Apply a key action to the editing state. Returns true if the state changed
/// (so the caller should push updateEditingState).
fn apply_key_to_editing(st: &mut TextInputState, action: KeyAction) -> bool {
    if !st.active {
        return false;
    }
    clamp_selection(st);
    let base = st.selection_base.max(0) as usize;
    let extent = st.selection_extent.max(0) as usize;
    let (sel_lo, sel_hi) = if base <= extent { (base, extent) } else { (extent, base) };
    let has_sel = sel_lo != sel_hi;

    match action {
        KeyAction::Insert(cp) => {
            let mut enc = [0u8; 4];
            let n = encode_utf8(cp, &mut enc);
            if n == 0 {
                return false;
            }
            // Replace any selection, then insert.
            delete_range(st, sel_lo, sel_hi);
            if st.text_len + n > TEXT_BUF_CAP {
                return false;
            }
            // Shift tail right by n.
            let tail = st.text_len - sel_lo;
            for k in (0..tail).rev() {
                st.text[sel_lo + n + k] = st.text[sel_lo + k];
            }
            st.text[sel_lo..sel_lo + n].copy_from_slice(&enc[..n]);
            st.text_len += n;
            let caret = (sel_lo + n) as i64;
            st.selection_base = caret;
            st.selection_extent = caret;
            true
        }
        KeyAction::Backspace => {
            if has_sel {
                delete_range(st, sel_lo, sel_hi);
                st.selection_base = sel_lo as i64;
                st.selection_extent = sel_lo as i64;
                true
            } else if sel_lo > 0 {
                // Delete one byte before caret (ASCII == 1 codepoint here).
                delete_range(st, sel_lo - 1, sel_lo);
                st.selection_base = (sel_lo - 1) as i64;
                st.selection_extent = (sel_lo - 1) as i64;
                true
            } else {
                false
            }
        }
        KeyAction::Delete => {
            if has_sel {
                delete_range(st, sel_lo, sel_hi);
                st.selection_base = sel_lo as i64;
                st.selection_extent = sel_lo as i64;
                true
            } else if sel_lo < st.text_len {
                delete_range(st, sel_lo, sel_lo + 1);
                st.selection_base = sel_lo as i64;
                st.selection_extent = sel_lo as i64;
                true
            } else {
                false
            }
        }
        KeyAction::Left => {
            let caret = if has_sel { sel_lo } else { sel_lo.saturating_sub(1) };
            st.selection_base = caret as i64;
            st.selection_extent = caret as i64;
            true
        }
        KeyAction::Right => {
            let caret = if has_sel {
                sel_hi
            } else {
                (sel_hi + 1).min(st.text_len)
            };
            st.selection_base = caret as i64;
            st.selection_extent = caret as i64;
            true
        }
        KeyAction::Home => {
            st.selection_base = 0;
            st.selection_extent = 0;
            true
        }
        KeyAction::End => {
            st.selection_base = st.text_len as i64;
            st.selection_extent = st.text_len as i64;
            true
        }
        KeyAction::None => false,
    }
}

/// Remove `text[lo..hi]`, compacting the tail. Caller ensures lo<=hi<=text_len.
fn delete_range(st: &mut TextInputState, lo: usize, hi: usize) {
    if lo >= hi || hi > st.text_len {
        return;
    }
    let n = hi - lo;
    let tail = st.text_len - hi;
    for k in 0..tail {
        st.text[lo + k] = st.text[hi + k];
    }
    st.text_len -= n;
}

/// Entry point called from the EV_KEY handler: feed one raw PS/2 key event into
/// the text-input pipeline. Returns true if it consumed the key as text (so the
/// caller still sends the hard key event, but knows text was handled).
fn textinput_feed_key(scancode: u32, pressed: bool) {
    let _guard = TEXT_INPUT_LOCK.lock();
    let mods = unsafe { &mut *core::ptr::addr_of_mut!(KBD_MODS) };
    if update_modifiers(scancode, pressed, mods) {
        return;
    }
    if !pressed {
        return;
    }
    let st = unsafe { &mut *core::ptr::addr_of_mut!(TEXT_INPUT) };
    if !st.active {
        return;
    }
    let action = scancode_to_action(scancode, mods);
    if apply_key_to_editing(st, action) {
        send_update_editing_state(st);
    }
}

// ── Mouse cursor channel ─────────────────────────────────────────────────────

fn handle_mousecursor_channel(msg: &FlutterPlatformMessage) {
    let payload = if msg.message != 0 && msg.message_size != 0 {
        unsafe { core::slice::from_raw_parts(msg.message as *const u8, msg.message_size) }
    } else {
        respond_platform_message(msg, JSON_SUCCESS_NULL);
        return;
    };
    let mut method = [0u8; 64];
    if let Some((mlen, _)) = json_method_name(payload, &mut method) {
        if &method[..mlen] == b"MouseCursor.activateSystemCursor" {
            // args = { device, kind }. Map the Flutter cursor `kind` to a kernel
            // cursor shape and apply it via SYS_CURSOR_SHAPE_SET so the compositor
            // actually draws the matching pointer (hand on links, I-beam on text…).
            if let Some(vi) = json_find_value(payload, b"kind") {
                if payload.get(vi) == Some(&b'"') {
                    let mut kind = [0u8; 32];
                    if let Some((klen, _)) = json_parse_string(payload, vi, &mut kind) {
                        let shape = cursor_kind_to_shape(&kind[..klen]);
                        // De-dupe: Flutter spams activateSystemCursor on every
                        // hover move. The kernel syscall redraws the cursor
                        // (invalidate → composite), so calling it per-event is a
                        // redraw STORM that freezes the cooperative core. Only hit
                        // the kernel when the shape actually CHANGES.
                        static LAST_CURSOR_SHAPE: AtomicU32 = AtomicU32::new(u32::MAX);
                        let changed = LAST_CURSOR_SHAPE.swap(shape, Ordering::Relaxed) != shape;
                        if changed {
                            sys::cursor_shape_set(shape);
                        }
                        static CUR_LOG: AtomicU32 = AtomicU32::new(0);
                        if CUR_LOG.fetch_add(1, Ordering::Relaxed) < 16 {
                            write(b"[embedder/cursor] kind=");
                            write(&kind[..klen]);
                            write(b" -> shape=");
                            write_dec(shape as u64);
                            write(b"\n");
                        }
                    }
                }
            }
        }
    }
    respond_platform_message(msg, JSON_SUCCESS_NULL);
}

/// Map a Flutter `SystemMouseCursors` kind string to a kernel `CURSOR_SHAPE_*`.
/// Unknown kinds fall back to the basic arrow.
fn cursor_kind_to_shape(kind: &[u8]) -> u32 {
    match kind {
        b"text" => sys::CURSOR_SHAPE_TEXT,
        b"click" => sys::CURSOR_SHAPE_CLICK,
        b"forbidden" | b"noDrop" | b"notAllowed" => sys::CURSOR_SHAPE_FORBIDDEN,
        b"grab" => sys::CURSOR_SHAPE_GRAB,
        b"grabbing" | b"move" | b"allScroll" => sys::CURSOR_SHAPE_GRABBING,
        b"resizeLeftRight" | b"resizeColumn" | b"resizeLeft" | b"resizeRight" => {
            sys::CURSOR_SHAPE_RESIZE_LR
        }
        b"resizeUpDown" | b"resizeRow" | b"resizeUp" | b"resizeDown" => {
            sys::CURSOR_SHAPE_RESIZE_UD
        }
        b"none" => sys::CURSOR_SHAPE_NONE,
        // basic, precise, copy, contextMenu, help, progress, wait, cell, zoomIn,
        // zoomOut, alias, disappearing, and any unknown → arrow.
        _ => sys::CURSOR_SHAPE_BASIC,
    }
}

// ── flutter/platform channel: clipboard, navigation, sound, chrome ───────────

// The clipboard is backed by the KERNEL (SYS_CLIPBOARD_*) so copied text is
// system-wide: it survives across apps/hosts, not just within one embedder.
// These buffers are just scratch space for marshalling to/from the syscalls.
const CLIPBOARD_CAP: usize = 8192;
static mut CLIPBOARD_BUF: [u8; CLIPBOARD_CAP] = [0; CLIPBOARD_CAP];
static mut CLIPBOARD_REPLY: [u8; CLIPBOARD_CAP + 64] = [0; CLIPBOARD_CAP + 64];

fn handle_platform_channel(msg: &FlutterPlatformMessage) {
    let payload = if msg.message != 0 && msg.message_size != 0 {
        unsafe { core::slice::from_raw_parts(msg.message as *const u8, msg.message_size) }
    } else {
        respond_platform_message(msg, JSON_SUCCESS_NULL);
        return;
    };
    let mut method = [0u8; 64];
    let mlen = match json_method_name(payload, &mut method) {
        Some((l, _)) => l,
        None => {
            respond_platform_message(msg, JSON_SUCCESS_NULL);
            return;
        }
    };
    let method = &method[..mlen];

    match method {
        b"Clipboard.setData" => {
            // args = { text: "..." } → push into the kernel system clipboard.
            if let Some(vi) = json_find_value(payload, b"text") {
                if payload.get(vi) == Some(&b'"') {
                    let tmp = unsafe { &mut *core::ptr::addr_of_mut!(CLIPBOARD_BUF) };
                    if let Some((len, _)) = json_parse_string(payload, vi, tmp) {
                        let len = len.min(CLIPBOARD_CAP);
                        sys::clipboard_set(&tmp[..len]);
                    }
                }
            }
            respond_platform_message(msg, JSON_SUCCESS_NULL);
        }
        b"Clipboard.getData" => {
            // Pull from the kernel system clipboard and reply [{"text": "..."}].
            let buf = unsafe { &mut *core::ptr::addr_of_mut!(CLIPBOARD_BUF) };
            let rc = sys::clipboard_get(buf);
            let len = if rc < 0 { 0 } else { (rc as usize).min(CLIPBOARD_CAP) };
            let out = unsafe { &mut *core::ptr::addr_of_mut!(CLIPBOARD_REPLY) };
            let mut pos = 0usize;
            json_push(out, &mut pos, b"[{\"text\":");
            json_push_str_escaped(out, &mut pos, &buf[..len]);
            json_push(out, &mut pos, b"}]");
            let reply =
                unsafe { core::slice::from_raw_parts(CLIPBOARD_REPLY.as_ptr(), pos) };
            respond_platform_message(msg, reply);
        }
        b"Clipboard.hasStrings" => {
            let has = sys::clipboard_len() > 0;
            respond_platform_message(
                msg,
                if has {
                    b"[{\"value\":true}]" as &[u8]
                } else {
                    b"[{\"value\":false}]" as &[u8]
                },
            );
        }
        b"SystemNavigator.pop" => {
            // The foreground app asked to close. If this host is a launched app
            // (not the shell), return focus to the shell via the kernel and exit
            // this process so the shell resumes. The framework has already popped
            // any in-app routes before reaching here.
            write(b"[embedder/platform] SystemNavigator.pop\n");
            let host_mode = CURRENT_HOST_MODE.load(Ordering::Acquire);
            respond_platform_message(msg, JSON_SUCCESS_NULL);
            if host_mode == HOST_MODE_APP {
                write(b"[embedder/platform] closing app -> refocus shell\n");
                sys::app_close_foreground();
                // Flush the response we just queued, then terminate this host.
                flush_engine_tasks();
                sys::exit(0);
            }
        }
        // SystemSound.play → a real PC-speaker beep (the OS provides the device).
        b"SystemSound.play" => {
            // The argument is a sound-type string (e.g. "SystemSoundType.alert"
            // or ".click"). Alert is lower/longer so the two are distinguishable.
            let is_alert = payload.windows(5).any(|w| w == b"alert");
            if is_alert {
                sys::beep(660, 90);
            } else {
                sys::beep(1000, 25);
            }
            respond_platform_message(msg, JSON_SUCCESS_NULL);
        }
        // No-op acknowledgements — correct typed null keeps the framework happy.
        // HapticFeedback: no vibration motor on this hardware (deliberate no-op).
        // SystemChrome: no system UI overlays/orientation on a single full-screen
        // compositor surface (deliberate no-op, acked correctly).
        b"HapticFeedback.vibrate"
        | b"SystemChrome.setApplicationSwitcherDescription"
        | b"SystemChrome.setSystemUIOverlayStyle"
        | b"SystemChrome.setPreferredOrientations"
        | b"SystemChrome.setEnabledSystemUIMode"
        | b"SystemChrome.setEnabledSystemUIOverlays"
        | b"SystemChrome.restoreSystemUIOverlays"
        | b"SystemChrome.setSystemUIChangeListener" => {
            respond_platform_message(msg, JSON_SUCCESS_NULL);
        }
        _ => {
            static PLAT_LOG: AtomicU32 = AtomicU32::new(0);
            if PLAT_LOG.fetch_add(1, Ordering::Relaxed) < 16 {
                write(b"[embedder/platform] unhandled method: ");
                write(method);
                write(b"\n");
            }
            respond_platform_message(msg, JSON_SUCCESS_NULL);
        }
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
    // ── On-demand package delivery. The kernel pipeline (HTTP → SHA-256 verify →
    // LRU cache → install) is reachable via syscalls 0x4C0-0x4C2; route the shell's
    // pkg_* messages to it. "Apps stream from your server on first tap." ─────────
    if payload == b"pkg_catalog" || payload.starts_with(b"pkg_catalog") {
        if !shell_capable {
            return b"{\"ok\":false,\"err\":\"cap\",\"packages\":[]}";
        }
        return format_pkg_catalog_json();
    }
    if payload.starts_with(b"pkg_resolve:") {
        if !shell_capable {
            return b"{\"ok\":false,\"err\":\"cap\",\"app_id\":-1}";
        }
        let name = trim_line(&payload[12..]); // "pkg_resolve:".len() == 12
        return format_app_id_json(sys::pkg_resolve(name));
    }
    if payload.starts_with(b"pkg_set_server:") {
        if !shell_capable {
            return b"{\"ok\":false,\"err\":\"cap\"}";
        }
        // "pkg_set_server:<dotted-ip>:<port>"
        let rest = trim_line(&payload[15..]); // "pkg_set_server:".len() == 15
        match parse_ip_port(rest) {
            Some((ip_be, port)) => {
                let _ = sys::pkg_set_server(ip_be, port);
                return b"{\"ok\":true}";
            }
            None => return b"{\"ok\":false}",
        }
    }
    // ── Settings. Driver/input preferences, settable from any host (the Settings
    // UI may run in the shell or its own app). "config:get" returns current state.
    if payload.starts_with(b"config:scroll_invert:") {
        let v = parse_u32_after_colon(payload);
        SCROLL_INVERT.store(v != 0, Ordering::Release);
        return b"{\"ok\":true}";
    }
    if payload.starts_with(b"config:scroll_speed:") {
        let v = parse_u32_after_colon(payload).clamp(10, 500);
        SCROLL_SPEED_PCT.store(v, Ordering::Release);
        return b"{\"ok\":true}";
    }
    if payload.starts_with(b"config:get") {
        return format_config_json();
    }
    b"{\"ok\":false}"
}

/// Serialise the current input settings as JSON for the Settings UI to load.
fn format_config_json() -> &'static [u8] {
    static mut CONFIG_JSON: [u8; 128] = [0; 128];
    let invert = SCROLL_INVERT.load(Ordering::Acquire);
    let speed = SCROLL_SPEED_PCT.load(Ordering::Acquire);
    let buf = unsafe { &mut *core::ptr::addr_of_mut!(CONFIG_JSON) };
    let mut pos = 0usize;

    fn append(buf: &mut [u8], pos: &mut usize, s: &[u8]) {
        for &b in s {
            if *pos < buf.len() {
                buf[*pos] = b;
                *pos += 1;
            }
        }
    }

    append(buf, &mut pos, b"{\"ok\":true,\"scroll_invert\":");
    append(buf, &mut pos, if invert { b"true" } else { b"false" });
    append(buf, &mut pos, b",\"scroll_speed\":");
    // speed as decimal
    let mut digits = [0u8; 10];
    let mut di = 0;
    let mut n = speed;
    if n == 0 {
        append(buf, &mut pos, b"0");
    } else {
        while n > 0 {
            digits[di] = b'0' + (n % 10) as u8;
            di += 1;
            n /= 10;
        }
        while di > 0 {
            di -= 1;
            let d = digits[di];
            append(buf, &mut pos, &[d]);
        }
    }
    append(buf, &mut pos, b"}");
    unsafe { core::slice::from_raw_parts(buf.as_ptr(), pos) }
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

// ── On-demand package pipeline JSON helpers ──────────────────────────────────

static mut PKG_APPID_JSON: [u8; 32] = [0; 32];
/// `{"app_id":N}` for a pkg_resolve result (N>=0 ok, anything <0 → -1).
fn format_app_id_json(id: i64) -> &'static [u8] {
    let out = unsafe { &mut PKG_APPID_JSON };
    const PREFIX: &[u8] = b"{\"app_id\":";
    out[..PREFIX.len()].copy_from_slice(PREFIX);
    let mut pos = PREFIX.len();
    if id < 0 {
        out[pos] = b'-';
        pos += 1;
        out[pos] = b'1';
        pos += 1;
    } else {
        pos += write_json_u32(&mut out[pos..], id as u32);
    }
    out[pos] = b'}';
    pos += 1;
    unsafe { core::slice::from_raw_parts(PKG_APPID_JSON.as_ptr(), pos) }
}

static mut PKG_CATALOG_JSON: [u8; 4096] = [0; 4096];
/// Serialise the kernel package catalog (128-byte PkgManifest entries) into the
/// `{"packages":[{"name","version","size"}]}` shape the Dart shell expects.
fn format_pkg_catalog_json() -> &'static [u8] {
    let mut records = [0u8; 4096];
    let cnt = sys::pkg_catalog(&mut records);
    let count = if cnt < 0 { 0 } else { cnt as usize };
    let out = unsafe { &mut PKG_CATALOG_JSON };
    const HEAD: &[u8] = b"{\"packages\":[";
    out[..HEAD.len()].copy_from_slice(HEAD);
    let mut pos = HEAD.len();
    let max = (records.len() / 128).min(count);
    let mut i = 0usize;
    while i < max {
        let off = i * 128;
        let name_end = records[off..off + 64].iter().position(|&b| b == 0).unwrap_or(64);
        let name = &records[off..off + name_end];
        let ver_end = records[off + 64..off + 80].iter().position(|&b| b == 0).unwrap_or(16);
        let version = &records[off + 64..off + 64 + ver_end];
        let size = u32::from_le_bytes(records[off + 112..off + 116].try_into().unwrap_or([0; 4]));
        if pos + name.len() + version.len() + 80 > out.len() {
            break;
        }
        if i > 0 {
            out[pos] = b',';
            pos += 1;
        }
        const NP: &[u8] = b"{\"name\":\"";
        out[pos..pos + NP.len()].copy_from_slice(NP);
        pos += NP.len();
        out[pos..pos + name.len()].copy_from_slice(name);
        pos += name.len();
        const VP: &[u8] = b"\",\"version\":\"";
        out[pos..pos + VP.len()].copy_from_slice(VP);
        pos += VP.len();
        out[pos..pos + version.len()].copy_from_slice(version);
        pos += version.len();
        const SP: &[u8] = b"\",\"size\":";
        out[pos..pos + SP.len()].copy_from_slice(SP);
        pos += SP.len();
        pos += write_json_u32(&mut out[pos..], size);
        out[pos] = b'}';
        pos += 1;
        i += 1;
    }
    out[pos..pos + 2].copy_from_slice(b"]}");
    pos += 2;
    unsafe { core::slice::from_raw_parts(PKG_CATALOG_JSON.as_ptr(), pos) }
}

/// Parse "a.b.c.d:port" → (ip as big-endian-packed u32, port). None on malformed.
fn parse_ip_port(s: &[u8]) -> Option<(u32, u16)> {
    let colon = s.iter().rposition(|&b| b == b':')?;
    let (ip_str, port_str) = (&s[..colon], &s[colon + 1..]);
    let mut octets = [0u32; 4];
    let mut oi = 0usize;
    let mut cur = 0u32;
    let mut have = false;
    for &b in ip_str {
        if b == b'.' {
            if oi >= 3 || !have {
                return None;
            }
            octets[oi] = cur;
            oi += 1;
            cur = 0;
            have = false;
        } else if b.is_ascii_digit() {
            cur = cur * 10 + (b - b'0') as u32;
            if cur > 255 {
                return None;
            }
            have = true;
        } else {
            return None;
        }
    }
    if oi != 3 || !have {
        return None;
    }
    octets[3] = cur;
    let ip = (octets[0] << 24) | (octets[1] << 16) | (octets[2] << 8) | octets[3];
    let mut port = 0u32;
    let mut phave = false;
    for &b in port_str {
        if b.is_ascii_digit() {
            port = port * 10 + (b - b'0') as u32;
            if port > 65535 {
                return None;
            }
            phave = true;
        } else {
            return None;
        }
    }
    if !phave {
        return None;
    }
    Some((ip, port as u16))
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

// ── Main embedder logic ───────────────────────────────────────────────────────

/// Entry called from `_start` with the kernel's bootstrap registers passed as
/// the first three C arguments (x86: rdi/rsi/rdx, aarch64: x0/x1/x2):
///   `host_mode` (1 = shell, 2 = launched app), `app_id`, `aot_va`.
extern "C" fn main_embedder(host_mode: u64, app_id: u64, aot_va: u64) {
    // DIAGNOSTIC (aarch64 bring-up): emit a raw breadcrumb via the syscall
    // wrapper so we can confirm main_embedder is entered and `write()` works
    // against a .rodata buffer before the heavier init runs.
    write(b"[host] main_embedder entered\n");
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
        UPDATE_SEMANTICS_ENABLED_FN.store(api_table.update_semantics_enabled, Ordering::SeqCst);
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
        UPDATE_SEMANTICS_ENABLED_FN.store(
            dlsym(handle, b"FlutterEngineUpdateSemanticsEnabled"),
            Ordering::SeqCst,
        );
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
    // Dart VM flags: UNDERSCORE names, COMMA-separated (Flutter splits --dart-flags
    // by comma; a space list is one invalid flag, ignored — delivery confirmed via
    // --print_flags). The real app's first frame triggers an old-gen GC whose
    // multi-thread STOP-THE-WORLD safepoint never converges on this bare-metal sync
    // layer (livelock). Strategy: make the heap large enough that NO GC fires during
    // the first frame — no safepoint, no livelock. (Do NOT use --marker_tasks=0 /
    // --scavenger_tasks=0: the debug VM asserts num_tasks>0 and SIGABRTs.)
    // NOTE: we deliberately do NOT pass a `--dart-flags=...` switch. The engine's
    // SettingsFromCommandLine (shell/common/switches.cc) runs every comma-split
    // entry through IsAllowedDartVMFlag against a tiny allowlist and FML_LOG(FATAL)s
    // on anything else — `--old_gen_heap_size` / `--new_gen_semi_max_size` /
    // `--no_background_compilation` are NOT on that list, so passing them via
    // --dart-flags aborts the engine (switches.cc:478). The original GC-heap sizing
    // was a workaround for an x86-TCG/JIT livelock that the later diagnosis traced
    // to font fallback + slow JIT; under AOT on ARM we let the VM use its defaults.
    #[repr(transparent)]
    struct ArgvPtrs([*const u8; 8]);
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
    // ARG0-ARG4 are AOT-safe (name, impeller=false, software-rendering,
    // disable-vm-service, precompiled-mode). ARG5-ARG8 are JIT-era GC/heap
    // workarounds that the RELEASE/AOT engine rejects as disallowed Dart VM flags
    // (switches.cc:478 FATAL). So pass only the first 5 args under AOT.
    let engine_argc: i32 = if is_aot { 5 } else { ENGINE_ARGV.0.len() as i32 };
    write_i32_at(
        &mut project_args.bytes,
        OFF_PROJECT_ARGS_COMMAND_LINE_ARGC,
        engine_argc,
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
    // Wire the semantics update callback so that once we enable semantics the
    // engine actually delivers the accessibility tree to us (stored in
    // SEMANTICS_TREE for a11y / automation consumers).
    write_u64_at(
        &mut project_args.bytes,
        OFF_PROJECT_ARGS_UPDATE_SEMANTICS_CALLBACK2,
        update_semantics_callback2 as *const () as u64,
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
            // Each app has its OWN AOT blob at /Applications/<name>.app/libapp.so.
            // (The kernel hands aot_va=0 for the JIT-era path, so load by file like
            // the shell does — NOT the shell's libapp.so, which is the bug that
            // froze the app host.)
            let libapp = build_app_libapp_path(app_id);
            if !libapp.is_empty() {
                write(b"[host] APP libapp.so: ");
                write(&libapp[..libapp.len().saturating_sub(1)]);
                write(b"\n");
                // dlopen uses the byte length (not NUL-termination), so pass the
                // path WITHOUT the trailing NUL or the kernel treats it as part of
                // the filename and the open fails.
                sys::dlopen(&libapp[..libapp.len().saturating_sub(1)], 0);
                configure_aot_snapshots(&mut project_args, 0, libapp);
            } else {
                write(b"[host] WARNING: no libapp.so path for app\n");
            }
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
            configure_aot_snapshots(&mut project_args, 0, b"/system/flutter/libapp.so\0");
        }
    }

    // Save the platform-thread stack pointer so the task-runner callback can
    // detect whether it is running on the platform thread.
    let rsp: u64;
    #[cfg(target_arch = "x86_64")]
    unsafe {
        asm!("mov {}, rsp", out(reg) rsp);
    }
    #[cfg(target_arch = "aarch64")]
    unsafe {
        asm!("mov {}, sp", out(reg) rsp);
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

    // 9b. Enable the semantics pipeline. With the callback wired (above) this
    // makes the engine compute and deliver the accessibility tree to
    // update_semantics_callback2, which we store for a11y / automation use.
    {
        let sem_va = UPDATE_SEMANTICS_ENABLED_FN.load(Ordering::SeqCst);
        if sem_va != 0 {
            let update_semantics_enabled: unsafe extern "C" fn(u64, bool) -> i32 =
                unsafe { core::mem::transmute(sem_va) };
            // DISABLED for now (pass `false`): enabling semantics makes the engine
            // rebuild the FULL accessibility tree on every frame/hover-change, which
            // is far too expensive on the cooperative single core — it froze the
            // shell on hover. The callback + channel stay wired, so re-enabling is a
            // one-flag change once it's throttled / moved off the hot path.
            let rc_sem = unsafe { update_semantics_enabled(engine_out, false) };
            if rc_sem == 0 {
                SEMANTICS_ENABLED.store(true, Ordering::Release);
                write(b"[embedder] semantics enabled\n");
            } else {
                write(b"[embedder] WARN: UpdateSemanticsEnabled non-zero\n");
            }
        } else {
            write(b"[embedder] WARN: FlutterEngineUpdateSemanticsEnabled not found\n");
        }
    }

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
    // Timestamp of the last user input (pointer/key/scroll). The pump runs fast
    // (60 fps) for a short window after input so reactions are snappy, then backs
    // right off so the single cooperative core isn't drowned in no-op frames.
    let mut last_input_ns: u64 = rdtsc_ns();

    // Pump the engine's platform task runner. With engine-default task runners
    // (custom_task_runners=NULL — our proven render config) the engine posts
    // platform-thread work, INCLUDING platform-message DELIVERY (the oscortex/shell
    // channel, flutter/textinput, flutter/mousecursor, every MethodChannel), to the
    // fml MessageLoop we initialized on this thread (pid 1). Nothing was draining it,
    // so platform_message_callback was NEVER invoked: the startup `list` and the
    // Install-demo `install:` round-trips were silently dropped (the UI sat on "No
    // apps installed"). __FlutterEngineFlushPendingTasksNow runs the expired tasks
    // on the current thread, delivering those messages.
    let flush_pending_va = dlsym(handle, b"__FlutterEngineFlushPendingTasksNow");
    if flush_pending_va == 0 {
        write(b"[embedder] WARN: __FlutterEngineFlushPendingTasksNow not found\n");
    }
    FLUSH_PENDING_FN.store(flush_pending_va, Ordering::SeqCst);
    let flush_pending: Option<unsafe extern "C" fn()> = if flush_pending_va != 0 {
        Some(unsafe { core::mem::transmute(flush_pending_va) })
    } else {
        None
    };

    // Foreground/background lifecycle. A host that loses focus (the shell when an
    // app is launched over it, or an app when the user returns to the shell) must
    // STOP driving frames: on a single core the backgrounded host's frame pump
    // otherwise hogs the CPU and starves the foreground host so it can never even
    // JIT-warm up. Start focused; the EV_FOCUS arm toggles this and all frame-pump
    // / schedule_frame work is gated on it.
    let mut focused = true;

    write(b"[embedder] entering event loop\n");
    loop {
        let now = rdtsc_ns();
        run_due_platform_tasks(engine_out, now, 64);
        // Deliver any pending platform messages (shell channel, text input, etc.).
        if let Some(f) = flush_pending {
            unsafe { f() };
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

        if focused && engine_out != 0 && now >= startup_watchdog_next_ns && startup_watchdog_stage < 6 {
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
            // Skipped entirely while backgrounded (focused=false) so we don't
            // starve the foreground host on a single core.
            let now_pump = rdtsc_ns();
            if focused && engine_out != 0 && proctable.schedule_frame != 0 && now_pump >= frame_pump_next_ns {
                let _ = schedule_frame_with_log(engine_out, proctable.schedule_frame, b"idle");
                frame_pump_next_ns = now_pump + pump_interval_ns(now_pump, last_input_ns);
            }
            continue;
        }

        // Any real user input enters the "active" window: pump at 60 fps for a
        // short while so the reaction is snappy, and fire the next pump right away.
        if ev.kind == EV_POINTER || ev.kind == EV_KEY || ev.kind == EV_SCROLL {
            last_input_ns = rdtsc_ns();
            frame_pump_next_ns = last_input_ns;
            // Request a frame RIGHT NOW so the reaction (hover highlight, button
            // press, scroll) paints this same iteration instead of waiting up to
            // ~16ms for the idle pump in the no-event branch to notice. The engine
            // coalesces redundant requests, and Flutter batches the pointer packet
            // (sent just below in the match) + flushes it at BeginFrame, so the
            // scheduled frame still reflects this event. This is the input latency
            // the user sees as a "few ms, very noticeable" feedback delay.
            if focused && engine_out != 0 && proctable.schedule_frame != 0 {
                let _ = schedule_frame_with_log(engine_out, proctable.schedule_frame, b"input");
            }
        }

        match ev.kind {
            EV_VSYNC => {
                let baton = ev.b as usize;
                // When no baton is pending the APIC timer still fires at 60 Hz
                // delivering EV_VSYNC(baton=0).  Use these ticks to pump
                // FlutterEngineScheduleFrame so Flutter posts a real baton and
                // keeps rendering even for static (no-animation) apps.
                if focused && baton == 0 && engine_out != 0 && proctable.schedule_frame != 0 {
                    let now_pump = rdtsc_ns();
                    if now_pump >= frame_pump_next_ns {
                        let _ = schedule_frame_with_log(
                            engine_out,
                            proctable.schedule_frame,
                            b"vsync0",
                        );
                        frame_pump_next_ns = now_pump + pump_interval_ns(now_pump, last_input_ns);
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
            EV_SCROLL => {
                // Mouse wheel. a = packed (x<<32 | y), b = signed dz (low 32 bits).
                if engine_out != 0 && proctable.send_pointer_event != 0 {
                    let x = ((ev.a >> 32) as i32) as f64;
                    let y = (ev.a as u32) as f64;
                    let dz = (ev.b as u32) as i32; // sign-preserved through u32

                    // One wheel notch → a chunk of logical pixels. Wheel up (+dz)
                    // scrolls content up → negative scroll_delta_y in Flutter's
                    // convention; so the base sign is negative. Both the direction
                    // (natural/reverse) and the pixels-per-notch speed are user
                    // configurable via the Settings UI (config:scroll_* commands).
                    let base = 50.0 * (SCROLL_SPEED_PCT.load(Ordering::Acquire) as f64) / 100.0;
                    let sign = if SCROLL_INVERT.load(Ordering::Acquire) { 1.0 } else { -1.0 };
                    let scroll_dy = sign * (dz as f64) * base;

                    let engine_now_us = || -> u64 {
                        if get_current_time_va != 0 {
                            let get_time: GetCurrentTimeFn =
                                unsafe { core::mem::transmute(get_current_time_va) };
                            unsafe { get_time() / 1000 }
                        } else {
                            rdtsc_ns() / 1000
                        }
                    };

                    static SCROLL_TS: AtomicU64 = AtomicU64::new(0);
                    let mut timestamp = engine_now_us();
                    loop {
                        let last = SCROLL_TS.load(Ordering::SeqCst);
                        let next = if timestamp <= last { last + 1 } else { timestamp };
                        if SCROLL_TS
                            .compare_exchange_weak(last, next, Ordering::SeqCst, Ordering::SeqCst)
                            .is_ok()
                        {
                            timestamp = next;
                            break;
                        }
                    }

                    let f: SendPointerEventFn =
                        unsafe { core::mem::transmute(proctable.send_pointer_event) };
                    let evt = FlutterPointerEvent {
                        struct_size: core::mem::size_of::<FlutterPointerEvent>(),
                        phase: 6, // kHover — scroll while not dragging
                        _pad0: 0,
                        timestamp,
                        x,
                        y,
                        device: 0,
                        signal_kind: 1, // kFlutterPointerSignalKindScroll
                        scroll_delta_x: 0.0,
                        scroll_delta_y: scroll_dy,
                        device_kind: 1,
                        _pad1: 0,
                        buttons: 0,
                        pan_x: 0.0,
                        pan_y: 0.0,
                        scale: 1.0,
                        rotation: 0.0,
                        view_id: 0,
                    };
                    let rc = unsafe { f(engine_out, &evt as *const _, 1) };
                    write(b"[embedder/scroll] dz=");
                    write_dec(dz as u64);
                    write(b" dy=");
                    write_dec(scroll_dy as i64 as u64);
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
                // Route the same key into the text-input pipeline: when a text
                // client is active this mutates the editing state and pushes
                // TextInputClient.updateEditingState back to the framework. It
                // tracks modifier latches even when no client is focused.
                textinput_feed_key(scancode, pressed);
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
            EV_FOCUS => {
                // Foreground/background transition (ev.flags: FOCUS_LOST=1,
                // FOCUS_GAINED=2). Backgrounded hosts stop driving frames so the
                // foreground host gets the single core; foregrounded hosts resume.
                let gained = ev.flags == 2;
                focused = gained;
                write(if gained {
                    b"[embedder] FOCUS_GAINED -> resume rendering\n" as &[u8]
                } else {
                    b"[embedder] FOCUS_LOST -> pause rendering\n" as &[u8]
                });
                if engine_out != 0 && proctable.send_platform_message != 0 {
                    static LIFECYCLE_CH: &[u8] = b"flutter/lifecycle\0";
                    let body: &[u8] = if gained {
                        b"AppLifecycleState.resumed"
                    } else {
                        b"AppLifecycleState.paused"
                    };
                    let msg = FlutterPlatformMessage {
                        struct_size: core::mem::size_of::<FlutterPlatformMessage>(),
                        channel: LIFECYCLE_CH.as_ptr() as u64,
                        message: body.as_ptr() as u64,
                        message_size: body.len(),
                        response_handle: 0,
                    };
                    let send_platform_message: SendPlatformMessageFn =
                        unsafe { core::mem::transmute(proctable.send_platform_message) };
                    let _ = unsafe { send_platform_message(engine_out, &msg as *const _) };
                }
                if gained && engine_out != 0 && proctable.schedule_frame != 0 {
                    // Re-kick the pipeline on resume.
                    frame_pump_next_ns = rdtsc_ns();
                    let _ = schedule_frame_with_log(
                        engine_out,
                        proctable.schedule_frame,
                        b"focus-resume",
                    );
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

/// Read a monotonic nanosecond timestamp from the CPU cycle/tick counter.
///
/// x86: the TSC at a nominal 3 GHz. aarch64: the architected generic timer
/// virtual count (`CNTVCT_EL0`) scaled by its frequency (`CNTFRQ_EL0`, typically
/// 62.5 MHz under QEMU). Both add the same Unix-epoch offset the kernel's
/// `sys_clock_gettime(CLOCK_MONOTONIC)` uses (1,700,000,000 s) so the engine's
/// timeline matches the kernel's.
#[cfg(target_arch = "x86_64")]
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

#[cfg(target_arch = "aarch64")]
#[inline(always)]
fn rdtsc_ns() -> u64 {
    let cnt: u64;
    let frq: u64;
    unsafe {
        core::arch::asm!("mrs {}, cntvct_el0", out(reg) cnt, options(nomem, nostack));
        core::arch::asm!("mrs {}, cntfrq_el0", out(reg) frq, options(nomem, nostack));
    }
    // ns = count * 1e9 / freq. Guard against a zero frequency (shouldn't happen
    // on QEMU virt, where CNTFRQ_EL0 is programmed). Use u128 to avoid overflow.
    let freq = if frq == 0 { 62_500_000u64 } else { frq };
    let ns = (cnt as u128 * 1_000_000_000u128 / freq as u128) as u64;
    ns.saturating_add(1_700_000_000u64 * 1_000_000_000u64)
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

/// Build "/Applications/<name>.app/libapp.so\0" for `app_id` (registry lookup).
/// Returns the NUL-terminated path slice, or empty if the app isn't found.
fn build_app_libapp_path(app_id: u64) -> &'static [u8] {
    let mut records = [0u8; 4096];
    let count = sys::app_list(&mut records) as usize;
    static mut LIBAPP_PATH_BUF: [u8; 256] = [0; 256];
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
                let buf = &mut *core::ptr::addr_of_mut!(LIBAPP_PATH_BUF);
                let prefix = b"/Applications/";
                let suffix = b".app/libapp.so";
                let mut pos = 0usize;
                buf[pos..pos + prefix.len()].copy_from_slice(prefix);
                pos += prefix.len();
                let copy_len = name.len().min(buf.len() - pos - suffix.len() - 1);
                buf[pos..pos + copy_len].copy_from_slice(&name[..copy_len]);
                pos += copy_len;
                buf[pos..pos + suffix.len()].copy_from_slice(suffix);
                pos += suffix.len();
                buf[pos] = 0;
                pos += 1;
                return core::slice::from_raw_parts(buf.as_ptr(), pos);
            }
        }
    }
    b""
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

fn configure_aot_snapshots(
    project_args: &mut FlutterProjectArgsRaw,
    aot_va: u64,
    libapp_path: &[u8],
) {
    let opt = if aot_va == 0 {
        aot_loader::load_dart_snapshot(libapp_path)
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

/// Adaptive frame-pump interval (ns). The single cooperative core is the scarce
/// resource, so we request frames only as often as actually useful:
///   • Warm-up (engine still JIT-compiling, <10 presents): 50 ms. One pending
///     schedule_frame is enough to mark "frame wanted"; pumping faster (the old
///     code used 1 ms = 1000/s) just floods the UI thread with tasks and steals
///     the very CPU the warm-up needs, so the FIRST frame arrives LATER.
///   • Active (input within the last 400 ms): 16 ms (~60 fps) for snappy reactions.
///   • Idle (warm, no recent input): 120 ms (~8 fps) — enough to keep on-demand
///     vsync alive and pick up late async state changes, without burning the core.
fn pump_interval_ns(now_ns: u64, last_input_ns: u64) -> u64 {
    let presents = PRESENT_TRACE_COUNT.load(Ordering::Relaxed);
    if presents < 10 {
        50_000_000 // warm-up: gentle nudge, leave the core free to compile
    } else if now_ns.saturating_sub(last_input_ns) < 400_000_000 {
        16_666_666 // recently interacted: full 60 fps
    } else {
        120_000_000 // idle: back off hard
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
