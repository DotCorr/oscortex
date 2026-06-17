use super::handlers::{fd_dir_path, *};
use super::poll::{
    alloc_synth_fd, monotonic_ns, sys_epoll_create_real, sys_epoll_ctl_real, sys_epoll_wait_real,
    sys_eventfd2, sys_timerfd_create_real, sys_timerfd_settime_real,
};
use super::posix;
use super::poll::KICK_REQUESTED;
use super::state::*;
use super::tables::{MAX_OPEN_FILES, OPEN_FILES};
use super::trace::{record_syscall_trace, POSTEXIT_TRACE_ACTIVE, POSTEXIT_TRACE_COUNT, POSTEXIT_TRACE_LIMIT};
use crate::embedder::abi as eabi;
use core::sync::atomic::Ordering;

// ── Dispatcher ────────────────────────────────────────────────────────────────

/// Fast SYSCALL entry path (called from assembly stub).
#[unsafe(no_mangle)]
pub extern "C" fn dispatch_fast(number: u64, arg0: u64, arg1: u64, arg2: u64, arg3: u64, arg4: u64) -> i64 {
    // Fast path: libm math (sin/cos/floor/round/pow/...). These are pure compute
    // routed from the POSIX math trampolines, called extremely frequently by the
    // Flutter engine (Skia) and the Dart VM. They never switch tasks or touch IO,
    // so skip the syscall tracing and return-context persistence below.
    if number >= crate::process::posix_trampolines::LIBM_NR_LO
        && number <= crate::process::posix_trampolines::LIBM_NR_HI
    {
        return crate::process::posix_trampolines::libm_call(number, arg0, arg1, arg2);
    }

    // Eagerly snapshot this thread's user GPRs NOW, while the per-CPU entry
    // snapshot is still fresh for us — before any handler runs, yields, or opens
    // an interrupt window in which another thread's syscall entry could clobber
    // the shared per-CPU snapshot. This is what makes a thread that yields inside
    // a syscall (e.g. epoll_wait) resume with its OWN callee-saved registers
    // intact, instead of leaking another thread's rbx/rbp/r12-15 (or, on
    // aarch64, x30 → `ret` to a stale address).
    //
    // x86 reaches here with interrupts still masked (the SYSCALL entry stub
    // hasn't sti'd yet), so the snapshot is fresh. aarch64 UNMASKS IRQs in the
    // vector dispatch BEFORE the handler runs, so an unmasked capture here could
    // already be clobbered — the aarch64 eager capture is therefore done earlier,
    // inside the IRQ-masked SVC window in `vectors.rs`. Don't redo it here.
    #[cfg(not(target_arch = "aarch64"))]
    {
        let cur = crate::process::current_pid();
        if cur != 0 {
            crate::process::capture_user_gprs_at_entry(cur);
        }
    }

    let user_rip = crate::arch::syscall::user_rip();
    record_syscall_trace(number, arg0, arg1, arg2, user_rip);

    // Post-pid2-exit trace window: log every syscall (any pid) for the first
    // POSTEXIT_TRACE_LIMIT calls after PID-2's exit, so we can see exactly
    // what pid=1 does once it resumes from pthread_cond_wait.
    if POSTEXIT_TRACE_ACTIVE.load(Ordering::Relaxed) {
        let n = POSTEXIT_TRACE_COUNT.fetch_add(1, Ordering::Relaxed);
        if n < POSTEXIT_TRACE_LIMIT {
            // The user caller's return address is on the user stack at [rsp]
            // (the trampoline did a `ret` after syscall, so on syscall entry
            // the trampoline hasn't returned yet — but actually syscall is
            // *before* ret, so [rsp] holds the libflutter return address that
            // *called* the trampoline). Read it carefully.
            let user_rsp = crate::arch::syscall::user_rsp();
            // Read only offsets whose target VA is actually mapped in the
            // current address space. A thread's stack may be only a few
            // pages, and any off that crosses into an unmapped guard page
            // would page-fault the kernel.
            let cur_cr3 = crate::arch::memory::read_cr3() & 0x000f_ffff_ffff_f000;
            let safe_read = |off: u64| -> u64 {
                let addr = user_rsp.wrapping_add(off);
                let page = addr & !0xfff;
                if crate::mm::paging::translate_user_page(cur_cr3, page).is_some() {
                    unsafe { core::ptr::read_volatile(addr as *const u64) }
                } else {
                    0
                }
            };
            let (c0, c1, c2, c3, c4): (u64, u64, u64, u64, u64) = if user_rsp != 0 && user_rsp >= 0x1000 && user_rsp < 0x0000_8000_0000_0000 {
                (safe_read(0), safe_read(8), safe_read(16), safe_read(0x40), safe_read(0x48))
            } else {
                (0, 0, 0, 0, 0)
            };
            log::warn!(
                "[postexit-sc] #{:03} pid={} nr={:#x} a0={:#x} a1={:#x} a2={:#x} rip={:#x} rsp={:#x} stk=[{:#x},{:#x},{:#x},+0x40={:#x},+0x48={:#x}]",
                n, crate::process::current_pid(), number, arg0, arg1, arg2, user_rip, user_rsp, c0, c1, c2, c3, c4
            );
        } else if n == POSTEXIT_TRACE_LIMIT {
            log::warn!("[postexit-sc] limit reached ({} syscalls logged)", POSTEXIT_TRACE_LIMIT);
            POSTEXIT_TRACE_ACTIVE.store(false, Ordering::Relaxed);
        }
    }

    // ── Stability: persist user return context into the PCB immediately on
    // every SYSCALL entry. Several syscall handlers may invoke
    // `enter_user_by_pid_noreturn` (futex yield, exec/wait, exit-to-parent,
    // exec child switch) which reloads the target PID's saved RIP/RSP from
    // p.regs. Without this persist, a process that has run for a while will
    // be re-entered at its original ELF entry point (typically 0x400000),
    // losing all forward progress. This is the foundation of cooperative
    // user-process scheduling on OSCortex v0.1.
    let cur_pid = crate::process::current_pid();
    if cur_pid != 0 {
        let user_rsp = crate::arch::syscall::user_rsp();
        crate::process::save_return_context(cur_pid, user_rip, user_rsp);
    }

    // Bootstraps a non-zero FS base for userspace if no runtime has set one
    // yet via arch_prctl(ARCH_SET_FS). This prevents null fs:offset accesses
    // in early libc/libstdc++ constructor paths. Also fires for new pthreads
    // (which start with FS_BASE cleared to 0 by enter_user_by_pid_noreturn),
    // giving each thread a distinct TLS area near its own stack top.
    if cur_pid != 0 {
        let fs = crate::arch::cpu::get_fs_base();
        if fs == 0 {
            // If this thread already HAS a TLS base (every kernel-spawned thread
            // gets a real variant-I TLS block from spawn_thread, recorded in
            // p.fs_base), the register is merely not loaded on this core right
            // now — restore it. Do NOT fall through to the stack-region bootstrap
            // below, which would clobber the thread's real TLS block (and, via
            // set_proc_fs_base, PERMANENTLY): the Dart VM reads its current-thread
            // __thread var (OSThread::current_vm_thread_) at TP+0x68, so a
            // stack-region TP sends every __thread access to stack garbage and
            // the first VM thread-registration faults.
            let saved = crate::process::get_proc_fs_base(cur_pid);
            if saved != 0 {
                crate::arch::cpu::set_fs_base(saved);
            } else {
                let user_rsp = crate::arch::syscall::user_rsp();
                // No TLS base yet (the main/exec'd process before any runtime set
                // one). Bootstrap a non-zero FS near the stack top so early libc/
                // libstdc++ ctors don't do null fs:offset accesses.
                let bootstrap_fs = user_rsp.saturating_sub(0x200);
                if bootstrap_fs != 0 {
                    crate::arch::cpu::set_fs_base(bootstrap_fs);
                    crate::process::set_proc_fs_base(cur_pid, bootstrap_fs);
                    static FS_BOOT_COUNT: core::sync::atomic::AtomicU32 =
                        core::sync::atomic::AtomicU32::new(0);
                    let n = FS_BOOT_COUNT.fetch_add(1, Ordering::AcqRel);
                    if n < 8 {
                        log::warn!(
                            "[syscall] bootstrapped FS base pid={} fs={:#x} rsp={:#x}",
                            cur_pid, bootstrap_fs, user_rsp
                        );
                    }
                }
            }
        }
    }

    // Trace stat-family calls so we can verify Flutter's IsFile() reaches us.
    // NOTE: 0x45A (__fxstat64) and 0x458 (__fxstat) take (ver, fd, stat_ptr) —
    // arg1 is an fd, NOT a path pointer; exclude them from the path trace.
    if matches!(number, 4 | 6 | 21 | 257 | 262 | 269 | 0x441 | 0x45B | 0x45C | 0x45D) {
        let path_arg = match number {
            4 | 6 | 21 | 0x441 => arg0,
            _ => arg1,
        };
        let mut buf = [0u8; 96];
        let mut n = 0usize;
        if path_arg != 0 {
            unsafe {
                let p = path_arg as *const u8;
                while n < buf.len() && *p.add(n) != 0 { buf[n] = *p.add(n); n += 1; }
            }
        }
        let s = core::str::from_utf8(&buf[..n]).unwrap_or("<non-utf8>");
        log::warn!("[stat-trace] nr={:#x} path='{}'", number, s);
    }

    if cur_pid == 8 || cur_pid == 9 {
        static PID89_SC_LOG: core::sync::atomic::AtomicU32 =
            core::sync::atomic::AtomicU32::new(0);
        let n = PID89_SC_LOG.fetch_add(1, Ordering::Relaxed);
        if n < 128 {
            log::warn!(
                "[pid89-sys] #{} pid={} nr={:#x} a0={:#x} a1={:#x} a2={:#x} a3={:#x} rip={:#x}",
                n,
                cur_pid,
                number,
                arg0,
                arg1,
                arg2,
                arg3,
                user_rip
            );
        }
    }

    // ── Capability enforcement ────────────────────────────────────────────
    // Privileged syscalls require the matching capability on the caller's PCB.
    // Unprivileged calls (render/wm/vfs-read/posix) return None and run freely,
    // so this never affects the shell or normal apps. The kernel idle task
    // (pid 0) is exempt (kernel-internal callers).
    if let Some(required) = required_cap(number) {
        if crate::process::current_pid() != 0
            && !crate::process::current_has_caps(required)
        {
            log::warn!(
                "[security] pid={} denied syscall {:#x} — missing capability {:?}",
                crate::process::current_pid(), number, required
            );
            return -1; // EPERM
        }
    }

    let __ret: i64 = match number {
        // POSIX-compatible
        0   => sys_read(arg0, arg1, arg2),
        1   => sys_write(arg0, arg1, arg2),
        20  => sys_writev(arg0, arg1, arg2),
        2   => sys_open(arg0, arg1, arg2),
        3   => sys_close(arg0),
        4   => sys_stat_path(arg0, arg1),    // Linux stat (path, statbuf)
        5   => sys_fstat(arg0, arg1),         // Linux fstat
        6   => sys_stat_path(arg0, arg1),    // Linux lstat (no symlinks)
        8   => sys_lseek(arg0, arg1 as i64, arg2), // Linux lseek
        9   => {
            // Linux mmap(addr, len, prot, flags, fd, offset).
            let flags = arg3;
            let fd    = arg4;
            let off   = crate::arch::syscall::user_r9();
            let is_anon = (flags & 0x20) != 0;
            let is_fixed = (flags & 0x10) != 0; // MAP_FIXED
            let fd_signed = fd as i64;
            let file_backed = !is_anon && fd_signed >= 3 && (fd_signed as usize) < MAX_OPEN_FILES;
            // For file-backed mmap we must copy data into the pages, so
            // they need to be writable temporarily.  Our mprotect is a stub
            // so this is effectively final perms; that's acceptable for now.
            let effective_prot = if file_backed { arg2 | 0x2 } else { arg2 };
            // Without MAP_FIXED, `addr` is only a hint; the kernel is free
            // to choose any address. Our sys_mmap treats any non-zero hint
            // as MAP_FIXED, which clobbers libflutter pages when Dart's heap
            // allocator passes hints that overlap. Force hint=0 unless the
            // caller explicitly asked for MAP_FIXED.
            let hint = arg0;
            let va = sys_mmap(hint, arg1, effective_prot);
            if va < 0 { return va; }
            if file_backed {
                let idx = fd_signed as usize;
                let tbl = OPEN_FILES.lock();
                if tbl[idx].used && !tbl[idx].is_dir {
                    let data = tbl[idx].data;
                    let start = off as usize;
                    if start < data.len() {
                        let n = (data.len() - start).min(arg1 as usize);
                        unsafe {
                            core::ptr::copy_nonoverlapping(
                                data.as_ptr().add(start),
                                va as *mut u8,
                                n,
                            );
                        }
                        log::debug!(
                            "[mmap] file-backed fd={} off={:#x} len={:#x} → va={:#x} ({} copied)",
                            fd, off, arg1, va, n
                        );
                    }
                }
            }
            va
        }
        10  => sys_mprotect(arg0, arg1, arg2),
        11  => sys_munmap(arg0, arg1),
        12  => 0,                            // brk stub — return 0 (= current break)
        21  => sys_access_path(arg0, arg1),  // Linux access(path, mode)
        39  => sys_getpid(),
        56  => sys_clone(arg0, arg1),        // clone(2): thread creation (CLONE_VM only)
        57  => -38,  // fork — not implemented (ENOSYS)
        59  => sys_exec(arg0, arg1),
        60  => sys_exit(arg0),
        61  => sys_waitpid(arg0, arg1, arg2),
        62  => sys_kill(arg0, arg1),
        158 => sys_arch_prctl(arg0, arg1), // Linux arch_prctl (TLS FS base)
        186 => sys_gettid(),                 // gettid (each thread has a slot)
        202 => sys_futex(arg0, arg1 as u32, arg2 as u32, number), // Linux SYS_futex — libc++ std::future uses this directly
        // Native Linux epoll/poll/timerfd — GLib/Dart call these with Linux ABI numbers
        // directly (not through our POSIX trampolines). Without handlers these
        // return -38 (ENOSYS) which causes Flutter's MessageLoopLinux to fail its
        // constructor (timerfd_create returns -1), making task-runner threads exit
        // immediately instead of entering their epoll_wait event loop.
        283 => sys_timerfd_create_real(),                             // timerfd_create(2)
        284 => sys_eventfd2(arg0, arg1),                              // eventfd(initval, flags)
        286 => sys_timerfd_settime_real(arg0, arg1, arg2, arg3),      // timerfd_settime(2)
        287 => 0,                                                      // timerfd_gettime(2) stub
        290 => sys_eventfd2(arg0, arg1),                              // eventfd2(2)
        293 => sys_pipe2(arg0, arg1),                                 // pipe2(2) via Linux ABI
        7   => {
            static POLL_TRACE: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
            let n = POLL_TRACE.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
            if n < 8 { log::warn!("[native-poll] poll(2) nr=7 pid={} a0={:#x} a1={} a2={} rip={:#x}", crate::process::current_pid(), arg0, arg1, arg2, user_rip); }
            0                                                         // no events ready
        }
        213 => sys_epoll_create_real(),                               // epoll_create(2)
        228 => posix::sys_clock_gettime(arg0 as i32, arg1),          // clock_gettime(2)
        232 => sys_epoll_wait_real(arg0, arg1, arg2, arg3),           // epoll_wait(2)
        233 => sys_epoll_ctl_real(arg0, arg1, arg2, arg3),            // epoll_ctl(2)
        281 => sys_epoll_wait_real(arg0, arg1, arg2, arg3),           // epoll_pwait(2)
        291 => sys_epoll_create_real(),                               // epoll_create1(2)
        231 => sys_exit(arg0),  // exit_group
        257 => {
            // Linux openat(dirfd, path, flags, mode) — path is NUL-terminated.
            let path_ptr = arg1;
            let mut len: usize = 0;
            unsafe {
                if path_ptr != 0 {
                    let p = path_ptr as *const u8;
                    while len < 4096 && *p.add(len) != 0 { len += 1; }
                }
            }
            // Absolute path or AT_FDCWD → use as-is.
            let is_abs = unsafe { path_ptr != 0 && *(path_ptr as *const u8) == b'/' };
            let dirfd_i = arg0 as i64;
            let at_fdcwd = (arg0 as u32) == 0xFFFFFF9C; // AT_FDCWD = -100
            if is_abs || at_fdcwd {
                return sys_open(path_ptr, len as u64, arg2);
            }
            // Relative path against a directory fd: join "<dir_path>/<rel>".
            if dirfd_i >= 0 {
                if let Some(dir_path) = fd_dir_path(arg0) {
                    let rel = unsafe { core::slice::from_raw_parts(path_ptr as *const u8, len) };
                    let rel_str = core::str::from_utf8(rel).unwrap_or("");
                    let mut joined = dir_path;
                    if !joined.ends_with('/') { joined.push('/'); }
                    joined.push_str(rel_str);
                    log::debug!("[openat] dirfd={} rel='{}' → '{}'", dirfd_i, rel_str, joined);
                    let cstr_ptr = joined.as_ptr() as u64;
                    let cstr_len = joined.len() as u64;
                    return sys_open(cstr_ptr, cstr_len, arg2);
                }
            }
            sys_open(path_ptr, len as u64, arg2)
        }
        262 => sys_newfstatat(arg0, arg1, arg2, arg3), // newfstatat
        269 => sys_access_path(arg1, arg2),            // faccessat(dirfd, path, mode, flags)

        // IPC
        0x200 => sys_ipc_send(arg0, arg1, arg2),
        0x201 => sys_ipc_recv(arg0, arg1),

        // Compositor (M13 private ABI)
        eabi::SYS_SURFACE_CREATE => sys_surface_create(arg0, arg1),
        eabi::SYS_SURFACE_MOVE => sys_surface_move(arg0, arg1, arg2),
        eabi::SYS_SURFACE_DESTROY => sys_surface_destroy(arg0),
        eabi::SYS_SURFACE_UPLOAD_RGBA32 => sys_surface_upload(arg0, arg1, arg2),
        eabi::SYS_SURFACE_PRESENT => sys_surface_present(arg0),
        eabi::SYS_FB_SIZE_PACKED => sys_fb_size_packed(),
        eabi::SYS_VSYNC_COUNTER => sys_vsync_counter(),
        eabi::SYS_VSYNC_WAIT_NONBLOCK => sys_vsync_wait_nonblock(arg0),
        eabi::SYS_SURFACE_OWNER => sys_surface_owner(arg0),
        eabi::SYS_SURFACE_Z_GET => sys_surface_z_get(arg0),
        eabi::SYS_SURFACE_Z_SET => sys_surface_z_set(arg0, arg1),
        eabi::SYS_SURFACE_GEOMETRY_GET => sys_surface_geometry_get(arg0),
        eabi::SYS_SURFACE_GEOMETRY_SET => sys_surface_geometry_set(arg0, arg1, arg2),
        eabi::SYS_SURFACE_VISIBILITY_GET => sys_surface_visibility_get(arg0),
        eabi::SYS_SURFACE_VISIBILITY_SET => sys_surface_visibility_set(arg0, arg1),
        eabi::SYS_SURFACE_CLIP_SET => sys_surface_clip_set(arg0, arg1, arg2),
        eabi::SYS_SURFACE_DAMAGE_SET => sys_surface_damage_set(arg0, arg1, arg2),
        eabi::SYS_SURFACE_DAMAGE_GET => sys_surface_damage_get(arg0),
        eabi::SYS_SURFACE_FLIP => sys_surface_flip(arg0),
        eabi::SYS_WM_EVENT_POLL => sys_wm_event_poll(),
        eabi::SYS_WM_EVENT_READ => sys_wm_event_read(arg0, arg1),
        eabi::SYS_WM_EVENT_INJECT => sys_wm_event_inject(arg0, arg1, arg2),
        eabi::SYS_WM_EVENT_WAIT => sys_wm_event_wait(arg0, arg1, arg2),
        eabi::SYS_EMBEDDER_ABI_VERSION => sys_embedder_abi_version(),
        eabi::SYS_WM_EVENT_SIZE => sys_wm_event_size(),
        eabi::SYS_WM_EVENT_STATS => sys_wm_event_stats_packed(),
        eabi::SYS_WM_FOCUS_PID_GET => sys_wm_focus_pid_get(),
        eabi::SYS_WM_FOCUS_SURFACE_SET => sys_wm_focus_surface_set(arg0),
        eabi::SYS_WM_FOCUS_MIRROR_GET => sys_wm_focus_mirror_get(),
        eabi::SYS_WM_FOCUS_MIRROR_SET => sys_wm_focus_mirror_set(arg0),
        eabi::SYS_APP_NOTIFY => sys_app_notify(arg0, arg1, arg2),
        eabi::SYS_PROC_SURFACE_COUNT => sys_proc_surface_count(arg0),
        eabi::SYS_APP_LAUNCH_PATH => sys_app_launch_path(arg0, arg1, arg2),
        eabi::SYS_ENGINE_POLICY_GET => sys_engine_policy_get(),
        eabi::SYS_ENGINE_VERSION_PACKED => sys_engine_version_packed(),
        eabi::SYS_ENGINE_HOST_REGISTER => sys_engine_host_register(arg0),
        eabi::SYS_ENGINE_HOST_PID_GET => sys_engine_host_pid_get(),
        eabi::SYS_ENGINE_LIBRARY_PATH_READ => sys_engine_library_path_read(arg0, arg1),

        // Phase 30 Slice 3 — kernel dynamic linker + anonymous mmap
        eabi::SYS_DLOPEN   => sys_dlopen(arg0, arg1, arg2),
        eabi::SYS_DLSYM    => sys_dlsym(arg0, arg1, arg2),
        eabi::SYS_DLCLOSE  => sys_dlclose(arg0),
        eabi::SYS_DL_GET_INIT_ARRAY => sys_dl_get_init_array(arg0, arg1, arg2, arg3),
        eabi::SYS_MMAP     => sys_mmap(arg0, arg1, arg2),
        eabi::SYS_MUNMAP   => sys_munmap(arg0, arg1),
        eabi::SYS_MPROTECT => sys_mprotect(arg0, arg1, arg2),

        // Phase 31 Slice A — FlutterEngineProcTable bridge
        eabi::SYS_ENGINE_PROCTABLE_SET     => sys_engine_proctable_set(arg0, arg1),
        eabi::SYS_ENGINE_PROCTABLE_PTR_GET => sys_engine_proctable_ptr_get(),
        eabi::SYS_ENGINE_VSYNC_BATON_POST  => sys_engine_vsync_baton_post(arg0),

        // Phase 31 Slice B — GPU fast path
        eabi::SYS_GPU_SUBMIT => sys_gpu_submit(arg0, arg1, arg2),

        // Phase 32-C — platform channel
        eabi::SYS_PLATFORM_MSG_POST  => sys_platform_msg_post(arg0, arg1, arg2, arg3),
        eabi::SYS_PLATFORM_MSG_RECV  => sys_platform_msg_recv(arg0, arg1),
        eabi::SYS_PLATFORM_MSG_REPLY => sys_platform_msg_reply(arg0, arg1, arg2),
        eabi::SYS_PLATFORM_MSG_ACK   => sys_platform_msg_ack(arg0, arg1, arg2),

        // Phase 32-D — stride-aware GPU blit
        eabi::SYS_GPU_SUBMIT_STRIDED => sys_gpu_submit_strided(arg0, arg1, arg2),

        // Phase 33-A — vsync rate control
        eabi::SYS_VSYNC_SET_HZ => sys_vsync_set_hz(arg0),

        // Phase 34-C — Dart AOT snapshot loader
        eabi::SYS_AOT_SNAPSHOT_LOAD => sys_aot_snapshot_load(arg0, arg1, arg2, arg3),

        // Phase 35 — Dart isolate lifecycle
        eabi::SYS_ISOLATE_SPAWN => sys_isolate_spawn(arg0, arg1, arg2, arg3),
        eabi::SYS_ISOLATE_KILL  => sys_isolate_kill(arg0),
        eabi::SYS_ISOLATE_CTRL  => sys_isolate_ctrl(arg0, arg1),

        // Phase 36 — Dart isolate message passing
        eabi::SYS_ISOLATE_MSG_SEND    => sys_isolate_msg_send(arg0, arg1, arg2),
        eabi::SYS_ISOLATE_MSG_RECV    => sys_isolate_msg_recv(arg0, arg1, arg2),
        eabi::SYS_ISOLATE_MSG_PENDING => sys_isolate_msg_pending(arg0),

        // Phase 37 — PS/2 input device query
        eabi::SYS_INPUT_DEV_COUNT => sys_input_dev_count(),
        eabi::SYS_INPUT_DEV_INFO  => sys_input_dev_info(arg0),

        // Phase 38 — .osx app registry
        eabi::SYS_APP_INSTALL   => sys_app_install(arg0, arg1, arg2),
        eabi::SYS_APP_LIST      => sys_app_list(arg0, arg1),
        eabi::SYS_APP_LAUNCH    => sys_app_launch(arg0, arg1),
        eabi::SYS_APP_UNINSTALL => sys_app_uninstall(arg0),

        // Phase 70 — Flutter platform-contract OS capabilities.
        eabi::SYS_CURSOR_SHAPE_SET     => sys_cursor_shape_set(arg0),
        eabi::SYS_CLIPBOARD_SET        => sys_clipboard_set(arg0, arg1),
        eabi::SYS_CLIPBOARD_GET        => sys_clipboard_get(arg0, arg1),
        eabi::SYS_APP_CLOSE_FOREGROUND => sys_app_close_foreground(),
        eabi::SYS_BEEP                 => sys_beep(arg0, arg1),

        // On-demand package delivery
        eabi::SYS_PKG_RESOLVE    => sys_pkg_resolve(arg0, arg1),
        eabi::SYS_PKG_CATALOG    => sys_pkg_catalog(arg0, arg1),
        eabi::SYS_PKG_SET_SERVER => sys_pkg_set_server(arg0, arg1),
        eabi::SYS_PKG_EVICT      => sys_pkg_evict(arg0),

        // Phase 39 — Named port IPC namespace
        eabi::SYS_PORT_BIND   => sys_port_bind(arg0, arg1, arg2),
        eabi::SYS_PORT_LOOKUP => sys_port_lookup(arg0, arg1, arg2, arg3),
        eabi::SYS_PORT_UNBIND => sys_port_unbind(arg0, arg1),

        // Phase 41 — USB host-controller query
        eabi::SYS_USB_CONTROLLER_COUNT => sys_usb_controller_count(),

        // Phase 42 — Framebuffer map + WM event dequeue
        eabi::SYS_FB_MAP        => sys_fb_map(arg0),
        eabi::SYS_WM_NEXT_EVENT => sys_wm_next_event(arg0),

        // Phase 43 — VFS query (no open-fd)
        eabi::SYS_VFS_LIST      => sys_vfs_list(arg0, arg1, arg2, arg3),
        eabi::SYS_VFS_READ      => sys_vfs_read(arg0, arg1, arg2, arg3),

        // Phase 44 — Writable ramdisk (/tmp/)
        eabi::SYS_VFS_WRITE     => sys_vfs_write(arg0, arg1, arg2, arg3),
        eabi::SYS_VFS_STAT      => sys_vfs_stat(arg0, arg1, arg2),

        // Phase 45 — virtio-net networking
        eabi::SYS_NET_INFO      => sys_net_info(arg0, arg1),
        eabi::SYS_NET_SEND      => sys_net_send(arg0, arg1, arg2, arg3),
        eabi::SYS_NET_RECV      => sys_net_recv(arg0, arg1, arg2, arg3),

        // Phase 46 — Compositor bypass + full-screen surface
        eabi::SYS_FB_RELEASE    => sys_fb_release(),
        eabi::SYS_SURFACE_FULLSCREEN => sys_surface_fullscreen(),

        // Phase 47 — exec + blocking wait (cooperative exec round-trip)
        0x39E => sys_exec_wait(arg0, arg1),
        0x39F => sys_reap_children(),

        // Phase 48 — UART 16550 serial console
        0x383 => sys_serial_read(arg0, arg1),
        0x384 => sys_serial_write(arg0, arg1),

        // Phase 49 — virtio-blk block device
        0x385 => sys_blk_info(arg0),
        0x386 => sys_blk_read(arg0, arg1, arg2, arg3),
        0x387 => sys_blk_write(arg0, arg1, arg2, arg3),

        // Phase 50 — TCP/IP (smoltcp)
        0x388 => sys_tcp_connect(arg0, arg1),
        0x389 => sys_tcp_write(arg0, arg1, arg2),
        0x38A => sys_tcp_read(arg0, arg1, arg2),
        0x38B => sys_tcp_close(arg0),
        0x38C => sys_dhcp_discover(),
        eabi::SYS_TCP_STATUS => sys_tcp_status(arg0),
        eabi::SYS_DNS_RESOLVE => sys_dns_resolve(arg0, arg1),

        // Phase 51 — ext2 read-only filesystem
        0x38D => sys_ext2_mount(),
        0x38E => sys_ext2_ls(arg0, arg1, arg2, arg3),
        0x38F => sys_ext2_read(arg0, arg1, arg2, arg3),

        // Phase 53 — preemptive scheduler extras
        0x390 => sys_sched_yield(),
        0x391 => sys_get_cpu_time(arg0),

        // Phase 54 — fork
        0x392 => sys_fork(),

        // Phase 55 — signals
        0x393 => sys_kill_signal(arg0, arg1),
        0x394 => sys_sigaction(arg0, arg1),
        0x395 => sys_sigreturn(),

        // Phase 60 — pty/tty
        0x396 => sys_pty_open(arg0),
        0x397 => sys_pty_read(arg0, arg1, arg2),
        0x398 => sys_pty_write(arg0, arg1, arg2),
        0x399 => sys_pty_ioctl(arg0, arg1, arg2),

        // Phase 59 — NVMe
        0x39A => sys_nvme_info(arg0, arg1),
        0x39B => sys_nvme_read(arg0, arg1, arg2, arg3),
        0x39C => sys_nvme_write(arg0, arg1, arg2, arg3),

        // Phase 62 — Futex (needed by pthreads / Flutter Dart VM)
        0x39D => sys_futex(arg0, arg1 as u32, arg2 as u32, number),

        // Phase 31 Slice C — threads
        eabi::SYS_THREAD_CREATE => sys_thread_create(arg0, arg1, arg2, arg3),
        eabi::SYS_THREAD_EXIT   => sys_thread_exit(arg0),
        eabi::SYS_THREAD_JOIN   => sys_thread_join(arg0, arg1),

        // ── POSIX compat shim (0x3A0+) ───────────────────────────────────
        // Memory
        0x3A0 => posix::sys_malloc(arg0),
        0x3A1 => posix::sys_free(arg0),
        0x3A2 => posix::sys_calloc(arg0, arg1),
        0x3A3 => posix::sys_realloc(arg0, arg1),
        0x3A4 => posix::sys_aligned_alloc(arg0, arg1),
        0x3A5 => posix::sys_posix_memalign(arg0, arg1, arg2),
        0x3A6 => posix::sys_malloc_usable_size(arg0),
        0x3A7 => posix::sys_strdup(arg0),
        0x3A8 => posix::sys_strndup(arg0, arg1),
        // Strings
        0x3B0 => posix::sys_strlen(arg0),
        0x3B1 => posix::sys_memcpy(arg0, arg1, arg2),
        0x3B2 => posix::sys_memset(arg0, arg1, arg2),
        0x3B3 => posix::sys_memmove(arg0, arg1, arg2),
        0x3B4 => posix::sys_memcmp(arg0, arg1, arg2),
        0x3B5 => posix::sys_memchr(arg0, arg1, arg2),
        0x3B6 => posix::sys_bzero(arg0, arg1),
        0x3B7 => posix::sys_strcmp(arg0, arg1),
        0x3B8 => posix::sys_strncmp(arg0, arg1, arg2),
        0x3B9 => posix::sys_strcpy(arg0, arg1),
        0x3BA => posix::sys_strncpy(arg0, arg1, arg2),
        0x3BB => posix::sys_strcat(arg0, arg1),
        0x3BC => posix::sys_strncat(arg0, arg1, arg2),
        0x3BD => posix::sys_strstr(arg0, arg1),
        0x3BE => posix::sys_strchr(arg0, arg1),
        0x3BF => posix::sys_strrchr(arg0, arg1),
        0x3C0 => posix::sys_strnlen(arg0, arg1),
        0x3C1 => posix::sys_strcspn(arg0, arg1),
        0x3C2 => posix::sys_strspn(arg0, arg1),
        0x3C3 => 0, // strtok_r stub
        0x3C4 => posix::sys_strcasestr(arg0, arg1),
        0x3C5 => posix::sys_strtol(arg0, arg1, arg2),
        0x3C6 => posix::sys_strtoul(arg0, arg1, arg2),
        0x3C7 => posix::sys_strtoll(arg0, arg1, arg2),
        0x3C8 => posix::sys_strtoull(arg0, arg1, arg2),
        0x3C9 => 0, // strtod → 0.0 (integer path)
        0x3CA => posix::sys_atoi(arg0),
        0x3CB => 0, // qsort stub (noop)
        0x3CC => posix::sys_rand(),
        0x3CD => { posix::sys_srand(arg0 as u32); 0 },
        // Threading
        0x3D0 => 0, // pthread_detach noop
        0x3D1 => posix::sys_pthread_self(),
        0x3D2 => posix::sys_pthread_mutex_init(arg0),
        0x3D3 => posix::sys_pthread_mutex_lock(arg0, number),
        0x3D4 => posix::sys_pthread_mutex_unlock(arg0),
        0x3D5 => 0, // pthread_mutex_destroy noop
        0x3D6 => posix::sys_pthread_mutex_trylock(arg0),
        0x3D7 => posix::sys_pthread_once(arg0, arg1, number),
        0x3D8 => posix::sys_pthread_key_create(arg0, arg1),
        0x3D9 => 0, // pthread_key_delete noop
        0x3DA => posix::sys_pthread_setspecific(arg0, arg1),
        0x3DB => posix::sys_pthread_getspecific(arg0),
        0x3DC => posix::sys_pthread_mutex_init(arg0), // cond_init: zero the struct
        0x3DD => posix::sys_pthread_cond_wait(arg0, arg1, number),
        0x3DE => posix::sys_pthread_cond_signal(arg0),
        0x3DF => posix::sys_pthread_cond_broadcast(arg0),
        0x3E0 => 0, // cond_destroy noop
        0x3E1 => posix::sys_pthread_cond_timedwait(arg0, arg1, arg2, number),
        0x3E2 => posix::sys_pthread_mutex_lock(arg0, number),   // rwlock_rdlock
        0x3E3 => posix::sys_pthread_mutex_lock(arg0, number),   // rwlock_wrlock
        0x3E4 => posix::sys_pthread_mutex_unlock(arg0), // rwlock_unlock
        0x3E5 => posix::sys_pthread_mutex_init(arg0),   // rwlock_init
        0x3E6 => 0, // rwlock_destroy
        0x3E7 => posix::sys_pthread_attr_init(arg0),
        0x3E8 => posix::sys_pthread_attr_destroy(arg0),
        0x3E9 => posix::sys_pthread_attr_setstacksize(arg0, arg1),
        0x3EA => posix::sys_pthread_attr_setdetachstate(arg0, arg1),
        0x3EB => posix::sys_pthread_attr_getstack(arg0, arg1, arg2),
        0x3EC..=0x3EE => 0, // mutexattr_* noops
        0x3EF..=0x3F1 => 0, // condattr_* noops
        0x3F2 => 0, // pthread_sigmask noop
        0x3F3 => 0, // pthread_kill noop
        0x3F4 => posix::sys_pthread_setname_np(arg0, arg1),
        0x3F5 | 0x3F7 => posix::sys_pthread_attr_getter_noop(number, arg0, arg1, arg2),
        // pthread_getattr_np(pthread_t thread, attr*) — fill attr with thread's
        // stack bounds so the Dart VM can call pthread_attr_getstack on it.
        0x3F6 => {
            let tid = crate::process::current_pid();
            let (mut base, mut size) = crate::process::get_user_stack_bounds(tid);
            if base == 0 {
                // clone-thread (Dart VM mutator/GC/helper): no recorded bounds.
                // Derive them from the mapped stack around the current RSP so the
                // VM's GetAndValidateThreadStackBounds succeeds instead of aborting.
                let (cb, cs) = posix::computed_stack_bounds();
                base = cb;
                size = cs;
            }
            if arg1 >= 0x1000 && base != 0 {
                // Store base at attr+0x10, size at attr+0x18 (common layout).
                unsafe {
                    *((arg1 as usize + 0x10) as *mut u64) = base;
                    *((arg1 as usize + 0x18) as *mut u64) = size;
                }
            }
            log::debug!("[pthread_getattr_np] tid={} base={:#x} size={:#x}", tid, base, size);
            0
        },
        // Semaphores
        0x3F8 => posix::sys_sem_init(arg0, arg1, arg2),
        0x3F9 => 0, // sem_destroy noop
        0x3FA => posix::sys_sem_wait(arg0),
        0x3FB => posix::sys_sem_trywait(arg0),
        0x3FC => posix::sys_sem_post(arg0),
        // TLS
        0x3FD => posix::sys_tls_get_addr(arg0),
        // System
        0x400 => { posix::sys_abort(); },
        0x401 => 4096, // getpagesize
        0x402 => posix::sys_sysconf(arg0 as i32),
        0x403 => posix::sys_nanosleep(arg0, arg1),
        0x404 => posix::sys_gettimeofday(arg0, arg1),
        0x405 => posix::sys_clock_gettime(arg0 as i32, arg1),
        0x406 => posix::sys_time(arg0),
        0x407 => 0, // wait stub
        0x408 => posix::sys_getrusage(arg0, arg1),
        0x409 => posix::sys_getcwd(arg0, arg1),
        0x40A => posix::sys_gethostname(arg0, arg1),
        0x40B..=0x40D => 0, // prctl, setsid, setpriority stubs
        0x40E => posix::sys_uname(arg0),
        0x40F => posix::sys_strerror(arg0 as i32),
        0x410 => posix::sys_strerror_r(arg0 as i32, arg1, arg2),
        0x411 => posix::sys_passthrough_syscall(arg0, arg1, arg2, arg3),
        0x412 => posix::sys_madvise(arg0, arg1, arg2),
        0x413 => sys_dladdr(arg0, arg1),
        0x414 => 0, // dlerror → NULL (no error)
        0x415 => posix::sys_getenv(arg0, arg1),
        0x416 => posix::sys_setlocale(arg0, arg1),
        0x417 => posix::sys_uselocale(arg0),
        0x418 => posix::sys_newlocale(arg0, arg1, arg2),
        0x419 => posix::sys_freelocale(arg0),
        0x41A..=0x41C => 0, // cat* stubs
        0x41D => 0, // __cxa_atexit noop
        // IO (most are stubs)
        0x420 => posix::sys_open64(arg0, arg1, arg2, arg3),
        0x421 => posix::sys_lseek64(arg0, arg1 as i64, arg2 as i32),
        0x422 => posix::sys_fopen64(arg0, arg1, arg2, arg3),
        0x423 => posix::sys_fclose(arg0),
        0x424 => posix::sys_fread(arg0, arg1, arg2, arg3),
        0x425 => posix::sys_fwrite(arg0, arg1, arg2, arg3),
        0x426 => posix::sys_fseek(arg0, arg1 as i64, arg2 as i32),
        0x427 => posix::sys_ftell(arg0),
        0x428 => posix::sys_feof(arg0),
        0x429 => posix::sys_ferror(arg0),
        0x42A => posix::sys_fflush(arg0),
        0x42B => posix::sys_fgets(arg0, arg1 as i32, arg2),
        0x42C => 0,                    // fputc stub
        0x42D => posix::sys_puts(arg0), // fputs(str, FILE*) -> emit string
        0x42E..=0x431 => 0,            // fputwc/getwc/ungetc/ungetwc stubs
        0x432 => posix::sys_fprintf(arg0, arg1, arg2),
        0x433 => posix::sys_fprintf(arg0, arg1, arg2), // vfprintf alias
        0x434 => posix::sys_snprintf(arg0, arg1, arg2, arg3, arg4),
        0x435 => posix::sys_vsnprintf(arg0, arg1, arg2, arg3), // vsnprintf
        0x436 => posix::sys_snprintf(arg0, arg1, arg2, arg3, arg4), // sprintf alias
        0x437 => posix::sys_printf(arg0, arg1),
        0x438 => posix::sys_puts(arg0),
        0x439 => posix::sys_perror(arg0),
        0x43A => 0, // vasprintf stub
        0x43B..=0x43D => 0, // isoc99_*scanf stubs
        0x43E => posix::sys_fileno(arg0),
        0x43F => posix::sys_rewind(arg0),
        0x440 => posix::sys_clearerr(arg0),
        0x441 => posix::sys_stat(arg0, arg1, arg2),
        0x442..=0x44F => 0, // fdopen/opendir/closedir etc stubs
        0x457 => sys_pipe2(arg0, arg1),
        0x44A..=0x459 => 0, // filesystem stubs all return 0
        0x45A => sys_fstat(arg1, arg2),         // __fxstat64(ver, fd, stat_ptr)
        0x45B => 0,                             // __fxstatat64 stub
        0x45C => posix::sys_stat(arg1, 0, arg2), // __lxstat64(ver, path, stat_ptr)
        0x45D => posix::sys_stat(arg1, 0, arg2), // __xstat64(ver, path, stat_ptr)
        // Network — all return -ENOSYS (no network stack from POSIX syscalls)
        0x460..=0x477 => -38,
        // Epoll/inotify — return unique synthetic fds so user-space event
        // loops that key std::map<fd, T> don't collide and trigger
        // out_of_range("map::at: key not found").
        0x478 | 0x479 => sys_epoll_create_real(),         // epoll_create / epoll_create1
        0x47A => sys_epoll_ctl_real(arg0, arg1, arg2, arg3),
        0x47B => sys_epoll_wait_real(arg0, arg1, arg2, arg3),
        0x47C => alloc_synth_fd(),          // inotify_init1
        0x47D => 1,                         // inotify_add_watch → wd 1
        0x47E => 0,                         // inotify_rm_watch
        0x47F => sys_timerfd_create_real(),
        0x480 => sys_timerfd_settime_real(arg0, arg1, arg2, arg3),
        0x481 => 0,         // poll → 0 events
        0x482 => 0,         // ioctl → success
        0x483 => 0, // sigemptyset: zero the set (ptr in arg0)
        0x484 => 0, // sigaddset noop
        0x485 => posix::sys_sigfillset(arg0),
        0x486..=0x487 => 0, // setjmp/longjmp stubs
        0x488 => posix::sys_wcslen(arg0),
        0x489 => posix::sys_mbrtowc(arg0, arg1, arg2, arg3),
        0x48A => posix::sys_mbsnrtowcs(arg0, arg1, arg2, arg3, 0),
        0x48B => posix::sys_mbsrtowcs(arg0, arg1, arg2, arg3),
        0x48C => posix::sys_mbtowc(arg0, arg1, arg2),
        0x48D => posix::sys_wcrtomb(arg0, arg1, arg2),
        0x48E => posix::sys_wmemchr(arg0, arg1, arg2),
        0x48F..=0x494 => 0, // locale string conversion stubs
        0x495..=0x499 => 0, // tcgetattr/tcsetattr/tzset/localtime_r/dup3 stubs
        0x49A => -1, // execvp → ENOEXEC
        0x49B => 0, // syslog noop
        0x49C..=0x4A2 => 0, // ilogbf/modff/nextafterf/scalbnf/llround/llroundf/remainder stubs
        0x4A3 => 0, // fcntl/fcntl64 stub: F_GETFL/F_SETFL/F_GETFD/F_SETFD all succeed with 0

        // Linux getrandom(2) (nr=318 = 0x13E): Dart VM uses this for entropy.
        // Fill the buffer with bytes from a simple TSC-mixed PRNG.
        // Returns number of bytes written (matches Linux ABI).
        0x13E => {
            let buf = arg0;
            let len = arg1 as usize;
            if buf == 0 || len == 0 { return 0; }
            // Best-effort: cap at 256 MiB to avoid runaway.
            let n = len.min(256 * 1024 * 1024);
            let mut s: u64 = crate::arch::rdtsc() ^ 0xA5A5_F00D_DEAD_BEEFu64;
            unsafe {
                let p = buf as *mut u8;
                let mut i = 0usize;
                while i < n {
                    // xorshift64*
                    s ^= s << 13;
                    s ^= s >> 7;
                    s ^= s << 17;
                    let mut v = s.wrapping_mul(0x2545_F491_4F6C_DD1D);
                    let chunk = (n - i).min(8);
                    for _ in 0..chunk {
                        core::ptr::write_volatile(p.add(i), (v & 0xFF) as u8);
                        v >>= 8;
                        i += 1;
                    }
                }
            }
            n as i64
        }

    // GNU emulated TLS
    0x4B0 => posix::sys_emutls_get_address(arg0),
    0x4B1 => posix::sys_emutls_register_common(arg0, arg1, arg2, arg3),

        // Power
        0xC0 => sys_poweroff(),

        // Cortex PID-0 API
        0x1000..=0x100F => crate::cortex::pid0::dispatch(number, arg0, arg1, arg2),

        // Unknown
        _ => {
            // Log first N unknown syscalls to help identify missing handlers.
            static UNKNOWN_SC_LOG: core::sync::atomic::AtomicU32 =
                core::sync::atomic::AtomicU32::new(0);
            let n = UNKNOWN_SC_LOG.fetch_add(1, Ordering::Relaxed);
            if n < 32 {
                log::warn!(
                    "[unknown-syscall] #{} pid={} nr={:#x} a0={:#x} a1={:#x} a2={:#x} rip={:#x} → -38",
                    n, crate::process::current_pid(), number, arg0, arg1, arg2,
                    crate::arch::syscall::user_rip()
                );
            }
            -38 // ENOSYS
        }
    };
    __ret
}

/// Legacy INT 0x80 syscall path (handled by `arch::x86_64::syscall::legacy_syscall_entry`).
/// The capability a syscall requires, or `None` if it is unprivileged.
///
/// Deliberately conservative: only the genuinely privileged operations are
/// gated (raw network access here; CAP_CORTEX is enforced inside the PID-0
/// dispatch). Rendering, window-manager, VFS-read and POSIX calls — everything
/// a normal app makes — are unprivileged and never gated, so enforcement is
/// safe to switch on without breaking the shell or apps.
fn required_cap(number: u64) -> Option<crate::security::Capabilities> {
    use crate::embedder::abi as eabi;
    use crate::security::Capabilities;
    match number {
        // Raw network: userspace TCP/UDP + DHCP. The kernel's own package
        // fetch does NOT go through these (it calls net::tcp directly), so
        // gating them only constrains userspace — which only the trusted shell
        // uses today.
        n if n == eabi::SYS_NET_INFO
            || n == eabi::SYS_NET_SEND
            || n == eabi::SYS_NET_RECV => Some(Capabilities::NET),
        0x388 | 0x389 | 0x38A | 0x38B | 0x38C => Some(Capabilities::NET),
        n if n == eabi::SYS_TCP_STATUS || n == eabi::SYS_DNS_RESOLVE => Some(Capabilities::NET),
        _ => None,
    }
}

pub fn dispatch_legacy() {
    log::warn!("[syscall] dispatch_legacy called without saved register frame");
}

