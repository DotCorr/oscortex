//! Syscall wrappers for OSCortex userspace — Phase 32-B.
//!
//! Implements all syscalls needed by the Flutter embedder using inline asm.
//!
//! ABI (x86_64 Linux-compatible):
//!   rax = syscall number
//!   rdi = arg0, rsi = arg1, rdx = arg2, r10 = arg3
//!   Return value in rax (negative = error).
//!
//! ABI (aarch64 Linux-compatible):
//!   x8 = syscall number
//!   x0 = arg0, x1 = arg1, x2 = arg2, x3 = arg3
//!   `svc #0`; return value in x0 (negative = error).
//!
//! Both backends expose the same `syscallN(nr, …)` surface so the typed wrappers
//! below are architecture-neutral. The kernel's aarch64 SVC handler
//! (`arch::aarch64::syscall::production_svc_handler`) reads x8 as the number and
//! x0..x4 as the dispatcher args, mirroring the x86 SYSCALL fast path.

use core::arch::asm;

// ── Syscall number constants (mirrors kernel/src/embedder/abi.rs) ─────────────

pub const SYS_WRITE: u64 = 1;
pub const SYS_READ: u64 = 0;
pub const SYS_CLOSE: u64 = 3;
pub const SYS_OPENAT: u64 = 257;
pub const SYS_GETPID: u64 = 39;
pub const SYS_EXIT: u64 = 60;

pub const SYS_SURFACE_CREATE: u64 = 0x300;
pub const SYS_SURFACE_UPLOAD_RGBA32: u64 = 0x303;
pub const SYS_SURFACE_DESTROY: u64 = 0x302;
pub const SYS_SURFACE_FLIP: u64 = 0x312;
pub const SYS_FB_SIZE_PACKED: u64 = 0x305;
pub const SYS_VSYNC_WAIT_NONBLOCK: u64 = 0x307;

pub const SYS_MMAP: u64 = 0x353;
pub const SYS_MPROTECT: u64 = 0x355;

pub const SYS_WM_EVENT_POLL: u64 = 0x320;
pub const SYS_WM_EVENT_READ: u64 = 0x321;
pub const SYS_WM_EVENT_INJECT: u64 = 0x322;
pub const SYS_WM_EVENT_WAIT: u64 = 0x323;
pub const SYS_SCHED_YIELD: u64 = 0x390;
pub const SYS_FB_MAP: u64 = 0x377;
pub const SYS_WM_NEXT_EVENT: u64 = 0x378;
pub const SYS_SURFACE_FULLSCREEN: u64 = 0x381;

pub const SYS_ENGINE_HOST_REGISTER: u64 = 0x345;
pub const SYS_ENGINE_LIBRARY_PATH_READ: u64 = 0x347;

pub const SYS_DLOPEN: u64 = 0x350;
pub const SYS_DLSYM: u64 = 0x351;
pub const SYS_DLCLOSE: u64 = 0x352;

pub const SYS_ENGINE_PROCTABLE_SET: u64 = 0x356;
pub const SYS_ENGINE_PROCTABLE_PTR_GET: u64 = 0x357;
pub const SYS_ENGINE_VSYNC_BATON_POST: u64 = 0x358;
pub const SYS_GPU_SUBMIT: u64 = 0x359;
pub const SYS_THREAD_CREATE: u64 = 0x35A;
pub const SYS_THREAD_EXIT: u64 = 0x35B;
pub const SYS_THREAD_JOIN: u64 = 0x35C;

pub const SYS_PLATFORM_MSG_POST: u64 = 0x360;
pub const SYS_PLATFORM_MSG_RECV: u64 = 0x361;
pub const SYS_PLATFORM_MSG_REPLY: u64 = 0x362;
pub const SYS_PLATFORM_MSG_ACK: u64 = 0x363;
pub const SYS_GPU_SUBMIT_STRIDED: u64 = 0x364;
pub const SYS_VSYNC_SET_HZ: u64 = 0x365; // Phase 33-A
pub const SYS_AOT_SNAPSHOT_LOAD: u64 = 0x366; // Phase 34-C
                                              // Phase 35 — Dart isolate lifecycle
pub const SYS_ISOLATE_SPAWN: u64 = 0x367;
pub const SYS_ISOLATE_KILL: u64 = 0x368;
pub const SYS_ISOLATE_CTRL: u64 = 0x369;
// Phase 36 — Dart isolate message passing
pub const SYS_ISOLATE_MSG_SEND: u64 = 0x36A;
pub const SYS_ISOLATE_MSG_RECV: u64 = 0x36B;
pub const SYS_ISOLATE_MSG_PENDING: u64 = 0x36C;

// Phase 70 — Flutter platform-contract OS capabilities (mirrors abi.rs).
pub const SYS_CURSOR_SHAPE_SET: u64 = 0x4B2;
pub const SYS_CLIPBOARD_SET: u64 = 0x4B3;
pub const SYS_CLIPBOARD_GET: u64 = 0x4B4;
pub const SYS_APP_CLOSE_FOREGROUND: u64 = 0x4B5;
pub const SYS_BEEP: u64 = 0x4B6;

// Mouse-cursor shapes accepted by SYS_CURSOR_SHAPE_SET (mirrors abi.rs).
pub const CURSOR_SHAPE_BASIC: u32 = 0;
pub const CURSOR_SHAPE_TEXT: u32 = 1;
pub const CURSOR_SHAPE_CLICK: u32 = 2;
pub const CURSOR_SHAPE_FORBIDDEN: u32 = 3;
pub const CURSOR_SHAPE_GRAB: u32 = 4;
pub const CURSOR_SHAPE_GRABBING: u32 = 5;
pub const CURSOR_SHAPE_RESIZE_LR: u32 = 6;
pub const CURSOR_SHAPE_RESIZE_UD: u32 = 7;
pub const CURSOR_SHAPE_NONE: u32 = 8;

// ── WM event kinds ────────────────────────────────────────────────────────────

pub const EV_VSYNC: u32 = 1;
pub const EV_POINTER: u32 = 2;
pub const EV_KEY: u32 = 3;
pub const EV_APP: u32 = 4;
pub const EV_FOCUS: u32 = 5;
pub const EV_PLATFORM_MSG: u32 = 6;
pub const EV_ISOLATE: u32 = 7; // Phase 35
pub const EV_ISOLATE_MSG: u32 = 8; // Phase 36
pub const EV_SCROLL: u32 = 9; // mouse scroll wheel; a = packed (x<<32|y), b = signed dz
                                   // Isolate state values (carried in WmEvent.flags on EV_ISOLATE events).
pub const ISOLATE_RUNNING: u32 = 1;
pub const ISOLATE_PAUSED: u32 = 2;
pub const ISOLATE_DEAD: u32 = 3;

// ── WM event structure (matches kernel's WmEvent repr(C)) ────────────────────

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct WmEvent {
    pub seq: u64,
    pub kind: u32,
    pub flags: u32,
    pub a: u64,
    pub b: u64,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct FbInfo {
    pub addr: u64,
    pub width: u32,
    pub height: u32,
    pub pitch: u32,
    pub bpp: u32,
}

pub const WM_EVENT_SIZE: usize = core::mem::size_of::<WmEvent>();

// ── Raw syscall helpers ───────────────────────────────────────────────────────

// ── x86_64 backend (SYSCALL: nr=rax, args rdi/rsi/rdx/r10) ────────────────────

#[cfg(target_arch = "x86_64")]
#[inline(always)]
pub unsafe fn syscall0(nr: u64) -> i64 {
    let ret: i64;
    unsafe {
        asm!("syscall",
            in("rax") nr,
            out("rcx") _,
            out("r11") _,
            lateout("rax") ret,
            options(nostack),
        );
    }
    ret
}

#[cfg(target_arch = "x86_64")]
#[inline(always)]
pub unsafe fn syscall1(nr: u64, a0: u64) -> i64 {
    let ret: i64;
    unsafe {
        asm!("syscall",
            in("rax") nr,
            in("rdi") a0,
            out("rcx") _,
            out("r11") _,
            lateout("rax") ret,
            options(nostack),
        );
    }
    ret
}

#[cfg(target_arch = "x86_64")]
#[inline(always)]
pub unsafe fn syscall2(nr: u64, a0: u64, a1: u64) -> i64 {
    let ret: i64;
    unsafe {
        asm!("syscall",
            in("rax") nr,
            in("rdi") a0,
            in("rsi") a1,
            out("rcx") _,
            out("r11") _,
            lateout("rax") ret,
            options(nostack),
        );
    }
    ret
}

#[cfg(target_arch = "x86_64")]
#[inline(always)]
pub unsafe fn syscall3(nr: u64, a0: u64, a1: u64, a2: u64) -> i64 {
    let ret: i64;
    unsafe {
        asm!("syscall",
            in("rax") nr,
            in("rdi") a0,
            in("rsi") a1,
            in("rdx") a2,
            out("rcx") _,
            out("r11") _,
            lateout("rax") ret,
            options(nostack),
        );
    }
    ret
}

#[cfg(target_arch = "x86_64")]
#[inline(always)]
pub unsafe fn syscall4(nr: u64, a0: u64, a1: u64, a2: u64, a3: u64) -> i64 {
    let ret: i64;
    unsafe {
        asm!("syscall",
            in("rax") nr,
            in("rdi") a0,
            in("rsi") a1,
            in("rdx") a2,
            in("r10") a3,
            out("rcx") _,
            out("r11") _,
            lateout("rax") ret,
            options(nostack),
        );
    }
    ret
}

// ── aarch64 backend (SVC #0: nr=x8, args x0..x5) ──────────────────────────────
//
// The kernel SVC handler reads x8 as the syscall number and x0..x4 as the five
// dispatcher arguments, then writes the return value back into x0. We mark x8 as
// `in` (the kernel does not clobber it) and x0 as `inlateout` since it carries
// arg0 in and the return out. We do NOT use `options(nostack)` blanket here
// because the SVC trap saves/restores the full frame; `nomem` is unsafe to
// assume (the syscall may read/write user buffers passed by pointer), so we omit
// it to keep the compiler from reordering memory ops across the trap.

#[cfg(target_arch = "aarch64")]
#[inline(always)]
pub unsafe fn syscall0(nr: u64) -> i64 {
    let ret: i64;
    unsafe {
        asm!("svc #0",
            in("x8") nr,
            lateout("x0") ret,
            options(nostack),
        );
    }
    ret
}

#[cfg(target_arch = "aarch64")]
#[inline(always)]
pub unsafe fn syscall1(nr: u64, a0: u64) -> i64 {
    let ret: i64;
    unsafe {
        asm!("svc #0",
            in("x8") nr,
            inlateout("x0") a0 => ret,
            options(nostack),
        );
    }
    ret
}

#[cfg(target_arch = "aarch64")]
#[inline(always)]
pub unsafe fn syscall2(nr: u64, a0: u64, a1: u64) -> i64 {
    let ret: i64;
    unsafe {
        asm!("svc #0",
            in("x8") nr,
            inlateout("x0") a0 => ret,
            in("x1") a1,
            options(nostack),
        );
    }
    ret
}

#[cfg(target_arch = "aarch64")]
#[inline(always)]
pub unsafe fn syscall3(nr: u64, a0: u64, a1: u64, a2: u64) -> i64 {
    let ret: i64;
    unsafe {
        asm!("svc #0",
            in("x8") nr,
            inlateout("x0") a0 => ret,
            in("x1") a1,
            in("x2") a2,
            options(nostack),
        );
    }
    ret
}

#[cfg(target_arch = "aarch64")]
#[inline(always)]
pub unsafe fn syscall4(nr: u64, a0: u64, a1: u64, a2: u64, a3: u64) -> i64 {
    let ret: i64;
    unsafe {
        asm!("svc #0",
            in("x8") nr,
            inlateout("x0") a0 => ret,
            in("x1") a1,
            in("x2") a2,
            in("x3") a3,
            options(nostack),
        );
    }
    ret
}

// ── Typed wrappers ────────────────────────────────────────────────────────────

/// Write `buf` to a kernel-side serial/debug output (fd 1 = stdout).
pub fn write(buf: &[u8]) -> i64 {
    unsafe { syscall3(SYS_WRITE, 1, buf.as_ptr() as u64, buf.len() as u64) }
}

pub fn read(fd: i64, buf: &mut [u8]) -> i64 {
    if fd < 0 {
        return -1;
    }
    unsafe {
        syscall3(
            SYS_READ,
            fd as u64,
            buf.as_mut_ptr() as u64,
            buf.len() as u64,
        )
    }
}

pub fn close(fd: i64) -> i64 {
    if fd < 0 {
        return -1;
    }
    unsafe { syscall1(SYS_CLOSE, fd as u64) }
}

/// Open a path relative to `dirfd` using Linux openat semantics.
/// `path_z` must be NUL-terminated.
pub fn openat(dirfd: i64, path_z: &[u8], flags: u64, mode: u64) -> i64 {
    if path_z.is_empty() {
        return -1;
    }
    unsafe {
        syscall4(
            SYS_OPENAT,
            dirfd as u64,
            path_z.as_ptr() as u64,
            flags,
            mode,
        )
    }
}

/// Terminate the calling process with exit code `code`.
pub fn exit(code: i32) -> ! {
    unsafe {
        syscall1(SYS_EXIT, code as u64);
        core::hint::unreachable_unchecked()
    }
}

/// Register the calling process as the Flutter engine host.
/// Returns the assigned host PID on success.
pub fn engine_host_register() -> i64 {
    unsafe { syscall0(SYS_ENGINE_HOST_REGISTER) }
}

/// Read the expected library path into `buf`.
/// Returns the number of bytes written.
pub fn engine_library_path_read(buf: &mut [u8]) -> i64 {
    unsafe {
        syscall2(
            SYS_ENGINE_LIBRARY_PATH_READ,
            buf.as_mut_ptr() as u64,
            buf.len() as u64,
        )
    }
}

/// Open an ELF shared object from the VFS.
/// Returns a positive handle on success or a negative errno.
pub fn dlopen(path: &[u8], flags: u32) -> i64 {
    unsafe {
        syscall3(
            SYS_DLOPEN,
            path.as_ptr() as u64,
            path.len() as u64,
            flags as u64,
        )
    }
}

/// Resolve a symbol from a loaded library.
/// Returns the VA in this process's address space, or 0 if not found.
pub fn dlsym(handle: u32, name: &[u8]) -> u64 {
    let ret = unsafe {
        syscall3(
            SYS_DLSYM,
            handle as u64,
            name.as_ptr() as u64,
            name.len() as u64,
        )
    };
    if ret <= 0 {
        0
    } else {
        ret as u64
    }
}

/// Close a library handle.
pub fn dlclose(handle: u32) -> i64 {
    unsafe { syscall1(SYS_DLCLOSE, handle as u64) }
}

pub const SYS_DL_GET_INIT_ARRAY: u64 = 0x382;

/// Query the DT_INIT / DT_INIT_ARRAY info for a loaded library so the
/// embedder can call C++ global constructors from user space.
///
/// Any of the out-pointers may be null to skip that field.
pub fn dl_get_init_array(
    handle: u32,
    out_init_fn: *mut u64,
    out_array_va: *mut u64,
    out_count: *mut u64,
) -> i64 {
    unsafe {
        syscall4(
            SYS_DL_GET_INIT_ARRAY,
            handle as u64,
            out_init_fn as u64,
            out_array_va as u64,
            out_count as u64,
        )
    }
}

/// Create a compositor surface of the given dimensions.
/// Returns a positive surface ID or a negative errno.
pub fn surface_create(width: u32, height: u32) -> i64 {
    unsafe { syscall2(SYS_SURFACE_CREATE, width as u64, height as u64) }
}

/// Destroy a compositor surface.
pub fn surface_destroy(surface_id: u32) -> i64 {
    unsafe { syscall1(SYS_SURFACE_DESTROY, surface_id as u64) }
}

/// Returns `(width << 32) | height` for the physical framebuffer.
pub fn fb_size_packed() -> u64 {
    let ret = unsafe { syscall0(SYS_FB_SIZE_PACKED) };
    if ret < 0 {
        0
    } else {
        ret as u64
    }
}

pub fn fb_map(info: &mut FbInfo) -> i64 {
    unsafe { syscall1(SYS_FB_MAP, info as *mut FbInfo as u64) }
}

/// Upload tightly-packed RGBA32 pixels and present the surface immediately.
pub fn gpu_submit(surface_id: u32, pixels: &[u8]) -> i64 {
    unsafe {
        syscall3(
            SYS_GPU_SUBMIT,
            surface_id as u64,
            pixels.as_ptr() as u64,
            pixels.len() as u64,
        )
    }
}

/// Stride-aware GPU submit. `row_bytes = 0` means tight-packed.
pub fn gpu_submit_strided(surface_id: u32, pixels: &[u8], row_bytes: usize) -> i64 {
    unsafe {
        syscall3(
            SYS_GPU_SUBMIT_STRIDED,
            surface_id as u64,
            pixels.as_ptr() as u64,
            row_bytes as u64,
        )
    }
}

/// Register a FlutterEngineProcTable with the kernel.
pub fn engine_proctable_set(ptr: u64, size: usize) -> i64 {
    unsafe { syscall2(SYS_ENGINE_PROCTABLE_SET, ptr, size as u64) }
}

/// Post a vsync baton to be included in the next EV_VSYNC event.
pub fn engine_vsync_baton_post(baton: u64) -> i64 {
    unsafe { syscall1(SYS_ENGINE_VSYNC_BATON_POST, baton) }
}

/// Create a new thread using raw OSCortex ABI.
pub fn thread_create(entry: unsafe extern "C" fn(*mut ()), arg: *mut ()) -> i64 {
    unsafe { syscall3(SYS_THREAD_CREATE, entry as *const () as u64, arg as u64, 0) }
}

/// Poll for the next WM event for this process.
/// Returns 1 if an event was written to `ev`, 0 if queue empty.
pub fn wm_event_poll(ev: &mut WmEvent) -> i64 {
    unsafe {
        syscall2(
            SYS_WM_EVENT_POLL,
            ev as *mut WmEvent as u64,
            WM_EVENT_SIZE as u64,
        )
    }
}

/// Block until a WM event is available (simple busy-poll via poll loop).
/// Uses SYS_WM_EVENT_WAIT with a timeout of `timeout_ms` ms; 0 = no timeout.
pub fn wm_event_wait(ev: &mut WmEvent, timeout_ms: u64) -> i64 {
    unsafe {
        syscall3(
            SYS_WM_EVENT_WAIT,
            ev as *mut WmEvent as u64,
            WM_EVENT_SIZE as u64,
            timeout_ms,
        )
    }
}

pub fn wm_event_inject(kind: u32, arg1: u64, arg2: u64) -> i64 {
    unsafe { syscall3(SYS_WM_EVENT_INJECT, kind as u64, arg1, arg2) }
}

/// Post a platform-channel message.
/// Returns the u64 sequence number cast to i64, or a negative errno.
pub fn platform_msg_post(channel: &[u8], payload: &[u8]) -> i64 {
    unsafe {
        syscall4(
            SYS_PLATFORM_MSG_POST,
            channel.as_ptr() as u64,
            channel.len() as u64,
            payload.as_ptr() as u64,
            payload.len() as u64,
        )
    }
}

/// Receive the oldest pending platform-channel message into `buf`.
/// Returns bytes written, or 0 if none pending.
pub fn platform_msg_recv(buf: &mut [u8]) -> i64 {
    unsafe {
        syscall2(
            SYS_PLATFORM_MSG_RECV,
            buf.as_mut_ptr() as u64,
            buf.len() as u64,
        )
    }
}

/// Reply to a platform-channel message identified by `seq`.
pub fn platform_msg_reply(seq: u64, data: &[u8]) -> i64 {
    unsafe {
        syscall3(
            SYS_PLATFORM_MSG_REPLY,
            seq,
            data.as_ptr() as u64,
            data.len() as u64,
        )
    }
}

/// Collect the reply for `seq` into `buf`.  Returns bytes written or 0.
pub fn platform_msg_ack(seq: u64, buf: &mut [u8]) -> i64 {
    unsafe {
        syscall3(
            SYS_PLATFORM_MSG_ACK,
            seq,
            buf.as_mut_ptr() as u64,
            buf.len() as u64,
        )
    }
}

/// Get this process's PID.
pub fn getpid() -> u32 {
    let r = unsafe { syscall0(SYS_GETPID) };
    r.max(0) as u32
}

/// Phase 33-A — set the compositor vsync rate (1–240 Hz).
/// Call after engine init to match the display's refresh rate.
pub fn vsync_set_hz(hz: u32) -> i64 {
    unsafe { syscall1(SYS_VSYNC_SET_HZ, hz as u64) }
}

/// Phase 34-C — load a Dart AOT snapshot from the VFS into this process's
/// address space.  Writes the mapped VA to `*out_va` and the byte size to
/// `*out_size`.  Returns 0 on success or a negative errno.
pub fn aot_snapshot_load(path: &[u8], out_va: &mut u64, out_size: &mut u64) -> i64 {
    // Strip optional trailing NUL — the kernel-side syscall treats the path
    // as a UTF-8 byte slice of exactly `len` bytes, so a literal `\0` would
    // make the lookup string include the NUL and miss every VFS entry.
    let effective_len = if !path.is_empty() && *path.last().unwrap() == 0 {
        path.len() - 1
    } else {
        path.len()
    };
    unsafe {
        syscall4(
            SYS_AOT_SNAPSHOT_LOAD,
            path.as_ptr() as u64,
            effective_len as u64,
            out_va as *mut u64 as u64,
            out_size as *mut u64 as u64,
        )
    }
}

/// Phase 35 — Spawn a Dart isolate inside a previously mapped AOT snapshot.
///
/// `aot_va`/`aot_size` come from `aot_snapshot_load`.  `entry_offset` is
/// the byte offset of the isolate entry function (0 = root isolate).
/// `stack_size` of 0 uses the kernel default (128 KiB).
/// Returns the new isolate ID (>0) or a negative errno.
pub fn isolate_spawn(aot_va: u64, aot_size: u64, entry_offset: u64, stack_size: u64) -> i64 {
    unsafe {
        syscall4(
            SYS_ISOLATE_SPAWN,
            aot_va,
            aot_size,
            entry_offset,
            stack_size,
        )
    }
}

/// Phase 35 — Kill a live isolate by its ID.
/// Returns 0 on success or a negative errno.
pub fn isolate_kill(id: u32) -> i64 {
    unsafe { syscall1(SYS_ISOLATE_KILL, id as u64) }
}

/// Phase 35 — Query or change isolate state.
///
/// `op`: 0 = get state, 1 = pause, 2 = resume.
/// Returns the current `ISOLATE_*` constant (1/2/3) for op=0, 0 for op=1/2,
/// or a negative errno.
pub fn isolate_ctrl(id: u32, op: u32) -> i64 {
    unsafe { syscall2(SYS_ISOLATE_CTRL, id as u64, op as u64) }
}

// ── App registry + VFS (shell) ───────────────────────────────────────────────

pub const SYS_APP_INSTALL: u64 = 0x36F;
pub const SYS_APP_LIST: u64 = 0x370;
pub const SYS_APP_LAUNCH: u64 = 0x371;
pub const SYS_APP_UNINSTALL: u64 = 0x372;
pub const SYS_VFS_READ:      u64 = 0x37A;
pub const SYS_VFS_STAT:      u64 = 0x37C;
pub const SYS_VFS_LIST:      u64 = 0x379;


pub fn app_install(bundle: &[u8], id_out: &mut u32) -> i64 {
    unsafe {
        syscall3(
            SYS_APP_INSTALL,
            bundle.as_ptr() as u64,
            bundle.len() as u64,
            id_out as *mut u32 as u64,
        )
    }
}

pub fn app_list(buf: &mut [u8]) -> i64 {
    unsafe { syscall2(SYS_APP_LIST, buf.as_mut_ptr() as u64, buf.len() as u64) }
}

pub fn app_launch(app_id: u32) -> i64 {
    unsafe { syscall2(SYS_APP_LAUNCH, app_id as u64, 0) }
}

pub fn app_uninstall(app_id: u32) -> i64 {
    unsafe { syscall1(SYS_APP_UNINSTALL, app_id as u64) }
}

fn path_len(path: &[u8]) -> usize {
    if !path.is_empty() && *path.last().unwrap() == 0 {
        path.len() - 1
    } else {
        path.len()
    }
}

pub fn vfs_read(path: &[u8], buf: &mut [u8]) -> i64 {
    let len = path_len(path);
    unsafe {
        syscall4(
            SYS_VFS_READ,
            path.as_ptr() as u64,
            len as u64,
            buf.as_mut_ptr() as u64,
            buf.len() as u64,
        )
    }
}

/// Return the byte size of a VFS file, or negative errno.
pub fn vfs_stat(path: &[u8]) -> i64 {
    let len = path_len(path);
    let mut size: u64 = 0;
    let rc = unsafe {
        syscall3(
            SYS_VFS_STAT,
            path.as_ptr() as u64,
            len as u64,
            &mut size as *mut u64 as u64,
        )
    };
    if rc == 0 {
        size as i64
    } else {
        rc
    }
}

pub fn vfs_list(path: &[u8], buf: &mut [u8]) -> i64 {
    let len = path_len(path);
    unsafe {
        syscall4(
            SYS_VFS_LIST,
            path.as_ptr() as u64,
            len as u64,
            buf.as_mut_ptr() as u64,
            buf.len() as u64,
        )
    }
}


/// Phase 36 — Send a message to a destination isolate's inbox.
///
/// `dst_id` is the isolate ID returned by `isolate_spawn`.  `data` is an
/// opaque byte blob (maximum `MAX_MSG_SIZE` = 64 KiB).
/// Returns 0 on success or a negative errno.
pub fn isolate_msg_send(dst_id: u32, data: &[u8]) -> i64 {
    unsafe {
        syscall3(
            SYS_ISOLATE_MSG_SEND,
            dst_id as u64,
            data.as_ptr() as u64,
            data.len() as u64,
        )
    }
}

/// Phase 36 — Receive the next message from `isolate_id`'s inbox into `buf`.
///
/// Returns the number of bytes written (>0), 0 if no message is pending, or a
/// negative errno.  Messages larger than `buf` are truncated and consumed.
pub fn isolate_msg_recv(isolate_id: u32, buf: &mut [u8]) -> i64 {
    unsafe {
        syscall3(
            SYS_ISOLATE_MSG_RECV,
            isolate_id as u64,
            buf.as_mut_ptr() as u64,
            buf.len() as u64,
        )
    }
}

/// Phase 36 — Return the number of messages pending in `isolate_id`'s inbox.
pub fn isolate_msg_pending(isolate_id: u32) -> i64 {
    unsafe { syscall1(SYS_ISOLATE_MSG_PENDING, isolate_id as u64) }
}

/// Phase 61 — Allocate anonymous read-write pages via mmap(hint=0, size, prot=3).
pub fn mmap_anon(size: usize) -> u64 {
    let ret = unsafe {
        syscall3(SYS_MMAP, 0, size as u64, 3 /* PROT_READ|PROT_WRITE */)
    };
    if ret <= 0 {
        0
    } else {
        ret as u64
    }
}

/// Phase 61 — Create a fullscreen compositor surface (replaces direct FB map).
pub fn surface_fullscreen() -> i64 {
    unsafe { syscall0(SYS_SURFACE_FULLSCREEN) }
}

/// Phase 61 — Upload RGBA32 pixels to a compositor surface.
pub fn surface_upload_rgba32(surface_id: u32, pixels: &[u8]) -> i64 {
    unsafe {
        syscall3(
            SYS_SURFACE_UPLOAD_RGBA32,
            surface_id as u64,
            pixels.as_ptr() as u64,
            pixels.len() as u64,
        )
    }
}

/// Phase 61 — Flip / present a compositor surface.
pub fn surface_flip(surface_id: u32) -> i64 {
    unsafe { syscall1(SYS_SURFACE_FLIP, surface_id as u64) }
}

/// Phase 61 — Non-blocking: get the next WM event for this process.
/// Returns 1 if an event was read, 0 if queue empty, negative on error.
pub fn wm_next_event(ev: &mut WmEvent) -> i64 {
    unsafe { syscall1(SYS_WM_NEXT_EVENT, ev as *mut WmEvent as u64) }
}

/// Yield the CPU to the next runnable process via the kernel cooperative
/// scheduler (syscall 0x390).  This MUST NOT touch the WM event queue: the
/// previous implementation used `wm_event_wait(1ms)` which popped and discarded
/// a WM event on every yield, silently eating pointer/key input before the main
/// loop's EV_POINTER handler could process it (clicks never reached Flutter).
pub fn sched_yield() {
    unsafe {
        syscall0(SYS_SCHED_YIELD);
    }
}

/// Phase 30 Slice 3 — Change memory protection
pub fn mprotect(va: u64, size: u64, prot: u64) -> i64 {
    unsafe {
        syscall3(SYS_MPROTECT, va, size, prot)
    }
}

// ── Phase 70 — Flutter platform-contract OS capabilities ─────────────────────

/// Set the active mouse-cursor shape drawn by the compositor.
/// `shape` is one of the `CURSOR_SHAPE_*` constants.
pub fn cursor_shape_set(shape: u32) -> i64 {
    unsafe { syscall1(SYS_CURSOR_SHAPE_SET, shape as u64) }
}

/// Replace the system-wide clipboard with `data`.
pub fn clipboard_set(data: &[u8]) -> i64 {
    unsafe { syscall2(SYS_CLIPBOARD_SET, data.as_ptr() as u64, data.len() as u64) }
}

/// Copy the system-wide clipboard into `buf`. Returns the FULL stored length
/// (may exceed `buf.len()`, signalling truncation), or a negative errno.
pub fn clipboard_get(buf: &mut [u8]) -> i64 {
    unsafe { syscall2(SYS_CLIPBOARD_GET, buf.as_mut_ptr() as u64, buf.len() as u64) }
}

/// Query the system clipboard length without copying.
pub fn clipboard_len() -> i64 {
    unsafe { syscall2(SYS_CLIPBOARD_GET, 0, 0) }
}

/// Close the foreground app and return focus to the shell. Returns the shell pid.
pub fn app_close_foreground() -> i64 {
    unsafe { syscall0(SYS_APP_CLOSE_FOREGROUND) }
}

/// Emit a PC-speaker tone (freq 0 = default system beep).
pub fn beep(freq_hz: u32, duration_ms: u32) -> i64 {
    unsafe { syscall2(SYS_BEEP, freq_hz as u64, duration_ms as u64) }
}

