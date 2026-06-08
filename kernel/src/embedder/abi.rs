//! Stable userspace embedder ABI contract.
//!
//! Centralizes WM event layout and private syscall numbers used by the
//! compositor/window-manager bridge so external runtimes (Flutter embedder,
//! test apps) can bind to one canonical contract.

pub const EMBEDDER_ABI_VERSION: u64 = 8;

// Private compositor/syscall bridge numbers.
pub const SYS_SURFACE_CREATE: u64 = 0x300;
pub const SYS_SURFACE_MOVE: u64 = 0x301;
pub const SYS_SURFACE_DESTROY: u64 = 0x302;
pub const SYS_SURFACE_UPLOAD_RGBA32: u64 = 0x303;
pub const SYS_SURFACE_PRESENT: u64 = 0x304;
pub const SYS_FB_SIZE_PACKED: u64 = 0x305;
pub const SYS_VSYNC_COUNTER: u64 = 0x306;
pub const SYS_VSYNC_WAIT_NONBLOCK: u64 = 0x307;
pub const SYS_SURFACE_OWNER: u64 = 0x308;
pub const SYS_SURFACE_Z_GET: u64 = 0x309;
pub const SYS_SURFACE_Z_SET: u64 = 0x30A;
pub const SYS_SURFACE_GEOMETRY_GET: u64 = 0x30B;
pub const SYS_SURFACE_GEOMETRY_SET: u64 = 0x30C;
pub const SYS_SURFACE_VISIBILITY_GET: u64 = 0x30D;
pub const SYS_SURFACE_VISIBILITY_SET: u64 = 0x30E;
pub const SYS_SURFACE_CLIP_SET: u64 = 0x30F;
pub const SYS_SURFACE_DAMAGE_SET: u64 = 0x310;
pub const SYS_SURFACE_DAMAGE_GET: u64 = 0x311;
pub const SYS_SURFACE_FLIP: u64 = 0x312;

pub const SYS_WM_EVENT_POLL: u64 = 0x320;
pub const SYS_WM_EVENT_READ: u64 = 0x321;
pub const SYS_WM_EVENT_INJECT: u64 = 0x322;
pub const SYS_WM_EVENT_WAIT: u64 = 0x323;

pub const SYS_EMBEDDER_ABI_VERSION: u64 = 0x330;
pub const SYS_WM_EVENT_SIZE: u64 = 0x331;
pub const SYS_WM_EVENT_STATS: u64 = 0x332;
pub const SYS_WM_FOCUS_PID_GET: u64 = 0x333;
pub const SYS_WM_FOCUS_SURFACE_SET: u64 = 0x334;
pub const SYS_WM_FOCUS_MIRROR_GET: u64 = 0x335;
pub const SYS_WM_FOCUS_MIRROR_SET: u64 = 0x336;

pub const SYS_APP_NOTIFY: u64 = 0x340;
pub const SYS_PROC_SURFACE_COUNT: u64 = 0x341;
pub const SYS_APP_LAUNCH_PATH: u64 = 0x342;
pub const SYS_ENGINE_POLICY_GET: u64 = 0x343;
pub const SYS_ENGINE_VERSION_PACKED: u64 = 0x344;
pub const SYS_ENGINE_HOST_REGISTER: u64 = 0x345;
pub const SYS_ENGINE_HOST_PID_GET: u64 = 0x346;
pub const SYS_ENGINE_LIBRARY_PATH_READ: u64 = 0x347;

// Phase 30 Slice 3 — kernel dynamic linker + anonymous memory mapping.
pub const SYS_DLOPEN:    u64 = 0x350; // dlopen(path_ptr, path_len, flags) → handle
pub const SYS_DLSYM:     u64 = 0x351; // dlsym(handle, name_ptr, name_len) → vaddr
pub const SYS_DLCLOSE:   u64 = 0x352; // dlclose(handle) → 0
pub const SYS_MMAP:      u64 = 0x353; // mmap(hint_va, size, prot) → va
pub const SYS_MUNMAP:    u64 = 0x354; // munmap(va, size) → 0  (stub)
pub const SYS_MPROTECT:  u64 = 0x355; // mprotect(va, size, prot) → 0  (stub)

// Phase 31 Slice A — FlutterEngineProcTable bridge.
pub const SYS_ENGINE_PROCTABLE_SET:     u64 = 0x356; // proctable_set(ptr, size) → 0
pub const SYS_ENGINE_PROCTABLE_PTR_GET: u64 = 0x357; // proctable_ptr_get() → ptr
pub const SYS_ENGINE_VSYNC_BATON_POST:  u64 = 0x358; // vsync_baton_post(baton) → 0

// Phase 31 Slice B — GPU / software rasterizer fast path.
pub const SYS_GPU_SUBMIT: u64 = 0x359; // gpu_submit(surface_id, pixel_ptr, pixel_len) → 0

// Phase 31 Slice C — shared-address-space threads.
pub const SYS_THREAD_CREATE: u64 = 0x35A; // thread_create(entry_fn, arg, stack_size) → tid
pub const SYS_THREAD_EXIT:   u64 = 0x35B; // thread_exit(code) → !
pub const SYS_THREAD_JOIN:   u64 = 0x35C; // thread_join(tid) → exit_code

/// Kernel-side stable definition of the Flutter engine proc-table.
///
/// The embedder allocates this struct in its own address space, fills the
/// function-pointer fields with the addresses returned by
/// `FlutterEngineGetProcAddresses`, then calls `SYS_ENGINE_PROCTABLE_SET`
/// so the kernel (and any other process querying it) can resolve engine
/// entry points without going through dlsym again.
///
/// All pointers are 64-bit VAs valid in the engine-host process's address
/// space.  The kernel stores only the struct VA; it does **not** dereference
/// the function pointers itself.
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct FlutterEngineProcTable {
    /// `FlutterEngineRun` — starts the engine's event loop.
    pub run:                       u64,
    /// `FlutterEngineShutdown` — stops and frees the engine.
    pub shutdown:                  u64,
    /// `FlutterEngineSendWindowMetricsEvent` — resize / DPI change.
    pub send_window_metrics:       u64,
    /// `FlutterEngineSendPointerEvent` — pointer / touch input.
    pub send_pointer_event:        u64,
    /// `FlutterEngineSendKeyEvent` — keyboard input.
    pub send_key_event:            u64,
    /// `FlutterEngineOnVsync` — embedder acks a vsync baton.
    pub on_vsync:                  u64,
    /// `FlutterEngineScheduleFrame` — request an immediate frame.
    pub schedule_frame:            u64,
    /// `FlutterEngineSendPlatformMessage` — send a platform-channel message.
    pub send_platform_message:     u64,
    /// Reserved for future entries; keeps the struct at a round size.
    pub _reserved: [u64; 8],
}

// Embedder engine policy constants.
pub const ENGINE_LOADER_DYNAMIC: u32 = 1;
pub const ENGINE_LOADER_STATIC: u32 = 2;
pub const ENGINE_TARGET_FLUTTER_3_29: u32 = 0x0003_001D;

// WM event kinds.
pub const EV_VSYNC: u32 = 1;
pub const EV_POINTER: u32 = 2;
pub const EV_KEY: u32 = 3;
pub const EV_APP: u32 = 4;
pub const EV_FOCUS: u32 = 5;

// Phase 32-C — platform channel syscalls.
pub const SYS_PLATFORM_MSG_POST:  u64 = 0x360; // platform_msg_post(ch_ptr, ch_len, data_ptr, data_len) → seq:u64
pub const SYS_PLATFORM_MSG_RECV:  u64 = 0x361; // platform_msg_recv(buf_ptr, buf_len) → bytes_written
pub const SYS_PLATFORM_MSG_REPLY: u64 = 0x362; // platform_msg_reply(seq, data_ptr, data_len) → 0
pub const SYS_PLATFORM_MSG_ACK:   u64 = 0x363; // platform_msg_ack(seq, buf_ptr, buf_len) → bytes_written

// Phase 32-D — stride-aware GPU blit.
pub const SYS_GPU_SUBMIT_STRIDED: u64 = 0x364; // gpu_submit_strided(surface_id, px_ptr, row_bytes) → 0

// Phase 33-A — hardware vsync rate control.
pub const SYS_VSYNC_SET_HZ: u64 = 0x365; // vsync_set_hz(hz: u32) → 0  (1–240 Hz)

// Phase 34-C — Dart AOT snapshot loader.
pub const SYS_AOT_SNAPSHOT_LOAD: u64 = 0x366; // aot_snapshot_load(path_ptr, path_len, out_va_ptr, out_size_ptr) → 0

// Phase 35 — Dart isolate lifecycle.
pub const SYS_ISOLATE_SPAWN: u64 = 0x367; // isolate_spawn(aot_va, aot_size, entry_offset, stack_size) → id
pub const SYS_ISOLATE_KILL:  u64 = 0x368; // isolate_kill(id) → 0
pub const SYS_ISOLATE_CTRL:  u64 = 0x369; // isolate_ctrl(id, op) → state  (op: 0=get 1=pause 2=resume)

// Phase 36 — Dart isolate message passing.
pub const SYS_ISOLATE_MSG_SEND:    u64 = 0x36A; // msg_send(dst_id, data_ptr, data_len) → 0
pub const SYS_ISOLATE_MSG_RECV:    u64 = 0x36B; // msg_recv(isolate_id, buf_ptr, buf_len) → bytes
pub const SYS_ISOLATE_MSG_PENDING: u64 = 0x36C; // msg_pending(isolate_id) → count

// Phase 37 — PS/2 input device query.
pub const SYS_INPUT_DEV_COUNT: u64 = 0x36D; // input_dev_count() → n
pub const SYS_INPUT_DEV_INFO:  u64 = 0x36E; // input_dev_info(n) → packed_u32

// Phase 38 — .osx app registry.
pub const SYS_APP_INSTALL:   u64 = 0x36F; // app_install(bundle_ptr, bundle_len, id_out_ptr) → 0
pub const SYS_APP_LIST:      u64 = 0x370; // app_list(buf_ptr, buf_len) → count
pub const SYS_APP_LAUNCH:    u64 = 0x371; // app_launch(app_id, flags) → host_pid
pub const SYS_APP_UNINSTALL: u64 = 0x372; // app_uninstall(app_id) → 0

// Phase 39 — Named port IPC namespace.
pub const SYS_PORT_BIND:   u64 = 0x373; // port_bind(name_ptr, name_len, isolate_id) → 0
pub const SYS_PORT_LOOKUP: u64 = 0x374; // port_lookup(name_ptr, name_len, iso_out, pid_out) → 0
pub const SYS_PORT_UNBIND: u64 = 0x375; // port_unbind(name_ptr, name_len) → 0

// Phase 41 — USB host-controller query.
pub const SYS_USB_CONTROLLER_COUNT: u64 = 0x376; // usb_controller_count() → n

// Phase 42 — userspace framebuffer access + WM event dequeue.
pub const SYS_FB_MAP:        u64 = 0x377; // fb_map(info_out_ptr) → 0 on success
pub const SYS_WM_NEXT_EVENT: u64 = 0x378; // wm_next_event(event_out_ptr) → 0 / -EAGAIN

// Phase 43 — VFS query API (no open-fd needed).
pub const SYS_VFS_LIST:  u64 = 0x379; // vfs_list(path_ptr, path_len, buf_ptr, buf_len) → bytes written
pub const SYS_VFS_READ:  u64 = 0x37A; // vfs_read(path_ptr, path_len, buf_ptr, buf_len) → bytes written

// Phase 44 — writable ramdisk (/tmp/).
pub const SYS_VFS_WRITE: u64 = 0x37B; // vfs_write(path_ptr, path_len, data_ptr, data_len) → 0
pub const SYS_VFS_STAT:  u64 = 0x37C; // vfs_stat(path_ptr, path_len, size_out_ptr) → 0 / -ENOENT

// Phase 45 — virtio-net + lightweight networking.
pub const SYS_NET_INFO:  u64 = 0x37D; // net_info(buf_ptr, buf_len) → bytes (MAC/IP text)
pub const SYS_NET_SEND:  u64 = 0x37E; // net_send(dst_ip_u32, dst_port, data_ptr, data_len) → 0
pub const SYS_NET_RECV:  u64 = 0x37F; // net_recv(buf_ptr, buf_len, src_ip_out, src_port_out) → bytes

// Phase 46 — compositor bypass + full-screen surface.
pub const SYS_FB_RELEASE:  u64 = 0x380; // fb_release() → 0  (hand FB back to compositor)
pub const SYS_SURFACE_FULLSCREEN: u64 = 0x381; // surface_fullscreen() → surface_id

// Phase 65 — expose DT_INIT / DT_INIT_ARRAY after dlopen so the embedder
// can call C++ global constructors from user space.
pub const SYS_DL_GET_INIT_ARRAY: u64 = 0x382;
// dl_get_init_array(handle, out_init_fn: *mut u64, out_array_va: *mut u64, out_count: *mut u64) → 0/-EBADF

// Phase 70 — OS platform capabilities backing the Flutter platform contract.
//
// These are real kernel-provided capabilities the Flutter framework expects of
// its host platform: an active mouse-cursor shape (drawn by the compositor), a
// system-wide clipboard shared across every app/host, a foreground-app close
// path (return focus to the shell), and a PC-speaker beep for SystemSound.play.
pub const SYS_CURSOR_SHAPE_SET: u64 = 0x4B2; // cursor_shape_set(shape: u32) → 0 / -EINVAL
pub const SYS_CLIPBOARD_SET:    u64 = 0x4B3; // clipboard_set(ptr, len) → 0 / -EINVAL
pub const SYS_CLIPBOARD_GET:    u64 = 0x4B4; // clipboard_get(buf_ptr, buf_len) → bytes (full length, may exceed buf)
pub const SYS_APP_CLOSE_FOREGROUND: u64 = 0x4B5; // app_close_foreground() → 0  (refocus shell, terminate fg app)
pub const SYS_BEEP:             u64 = 0x4B6; // beep(freq_hz, duration_ms) → 0  (PC speaker)

/// Mouse-cursor shapes the compositor can draw. Mirrors the subset of Flutter
/// `SystemMouseCursors` kinds OSCortex maps to a distinct on-screen pointer.
pub const CURSOR_SHAPE_BASIC:     u32 = 0; // arrow (default)
pub const CURSOR_SHAPE_TEXT:      u32 = 1; // I-beam (text fields)
pub const CURSOR_SHAPE_CLICK:     u32 = 2; // hand / link
pub const CURSOR_SHAPE_FORBIDDEN: u32 = 3; // no-drop / forbidden
pub const CURSOR_SHAPE_GRAB:      u32 = 4; // open hand / grab
pub const CURSOR_SHAPE_GRABBING:  u32 = 5; // closed hand / grabbing
pub const CURSOR_SHAPE_RESIZE_LR: u32 = 6; // horizontal resize
pub const CURSOR_SHAPE_RESIZE_UD: u32 = 7; // vertical resize
pub const CURSOR_SHAPE_NONE:      u32 = 8; // hidden cursor
pub const CURSOR_SHAPE_MAX:       u32 = 8;

/// Maximum bytes retained in the kernel system clipboard.
pub const CLIPBOARD_MAX: usize = 64 * 1024;

// On-demand package delivery — Fuchsia-inspired ephemeral fetch.
pub const SYS_PKG_RESOLVE:    u64 = 0x390; // pkg_resolve(name_ptr, name_len) → app_id / -ERRNO
pub const SYS_PKG_CATALOG:    u64 = 0x391; // pkg_catalog(buf_ptr, buf_len) → count (writes manifest entries)
pub const SYS_PKG_SET_SERVER: u64 = 0x392; // pkg_set_server(ip_packed_be, port) → 0
pub const SYS_PKG_EVICT:      u64 = 0x393; // pkg_evict(app_id) → 0 / -ERRNO

// WM event kind for incoming platform-channel messages.
pub const EV_PLATFORM_MSG: u32 = 6;

// WM event kind for Dart isolate lifecycle state changes (Phase 35).
// `flags` = IsolateState (1=Running, 2=Paused, 3=Dead), `a` = isolate_id, `b` = owner_pid.
pub const EV_ISOLATE: u32 = 7;
/// Isolate state values carried in `EV_ISOLATE.flags`.
pub const ISOLATE_RUNNING: u32 = 1;
pub const ISOLATE_PAUSED:  u32 = 2;
pub const ISOLATE_DEAD:    u32 = 3;

// WM event kind for incoming isolate messages (Phase 36).
// `flags` = 0, `a` = dst_isolate_id, `b` = message_byte_len.
pub const EV_ISOLATE_MSG: u32 = 8;

// Focus transition flags used with EV_FOCUS events.
pub const FOCUS_LOST:   u32 = 1;
pub const FOCUS_GAINED: u32 = 2;
pub const FOCUS_MIRROR: u32 = 3;

/// Compact description of the project to be run by the Flutter engine.
///
/// The embedder populates this and passes a pointer to `FlutterEngineRun`.
/// All string fields are null-terminated C-string pointers in the embedder's
/// VA space.
///
/// This layout intentionally matches the leading `FlutterProjectArgs` prefix
/// from `flutter_embedder.h` through legacy snapshot pointers so field offsets
/// remain stable for engine-side reads.
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct FlutterProjectArgs {
    /// `sizeof(FlutterProjectArgs)` — used for ABI versioning.
    pub struct_size: usize,
    /// Null-terminated path to the AOT data blob or snapshot directory.
    pub assets_path: u64,
    /// Deprecated Dart source entrypoint path (kept for ABI stability).
    pub main_path_unused: u64,
    /// Deprecated package config path (kept for ABI stability).
    pub packages_path_unused: u64,
    /// Null-terminated path to `icudtl.dat`, or 0 for embedded ICU.
    pub icu_data_path: u64,
    /// Command-line argument count for engine initialization flags.
    pub command_line_argc: i32,
    pub _pad0: u32,
    /// Pointer to `command_line_argc` null-terminated argument strings.
    pub command_line_argv: u64,
    /// Callback invoked for platform-channel messages destined for the host.
    /// Signature: `extern "C" fn(engine: u64, msg: *const FlutterPlatformMessage)`
    pub platform_message_callback: u64,
    /// Legacy VM snapshot data ptr (AOT mode), or 0.
    pub vm_snapshot_data: u64,
    pub vm_snapshot_data_size: usize,
    /// Legacy VM snapshot instructions ptr (AOT mode), or 0.
    pub vm_snapshot_instructions: u64,
    pub vm_snapshot_instructions_size: usize,
    /// Legacy isolate snapshot data ptr (AOT mode), or 0.
    pub isolate_snapshot_data: u64,
    pub isolate_snapshot_data_size: usize,
    /// Legacy isolate snapshot instructions ptr (AOT mode), or 0.
    pub isolate_snapshot_instructions: u64,
    pub isolate_snapshot_instructions_size: usize,
}

/// Software renderer configuration passed to `FlutterEngineRun` inside a
/// `FlutterRendererConfig` union when using the software rasterizer path.
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct FlutterSoftwareRendererConfig {
    /// `sizeof(FlutterSoftwareRendererConfig)`.
    pub struct_size: usize,
    /// Callback invoked when the rasterizer wants to present a frame.
    /// Signature: `extern "C" fn(user_data: *mut (), alloc: *const u8, row_bytes: usize, height: usize) -> bool`
    pub surface_present_callback: u64,
}

/// Top-level renderer configuration passed to `FlutterEngineRun`.
/// `renderer_type` selects which union member is active.
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct FlutterRendererConfig {
    /// Renderer type: 0 = OpenGL, 1 = Software, 2 = Metal, 3 = Vulkan.
    pub renderer_type: u32,
    pub _pad: u32,
    pub software: FlutterSoftwareRendererConfig,
}
// App lifecycle subkinds used with EV_APP events (in flags field).
pub const APP_LAUNCH: u32 = 1;
pub const APP_TERMINATE: u32 = 2;
pub const APP_PAUSE: u32 = 3;
pub const APP_RESUME: u32 = 4;

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct WmEvent {
    pub seq: u64,
    pub kind: u32,
    pub flags: u32,
    pub a: u64,
    pub b: u64,
}

impl WmEvent {
    pub const fn empty() -> Self {
        Self {
            seq: 0,
            kind: 0,
            flags: 0,
            a: 0,
            b: 0,
        }
    }
}

pub const WM_EVENT_SIZE: usize = core::mem::size_of::<WmEvent>();

