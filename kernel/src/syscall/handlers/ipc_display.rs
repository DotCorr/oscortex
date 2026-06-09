use super::fd::{read_user_bytes, write_user_bytes};
use crate::embedder::abi as eabi;
use crate::syscall::poll::{
    check_timerfds_and_wake, force_wake_all_task_runners, monotonic_ns, KICK_REQUESTED,
};
use crate::syscall::state::{
    ENGINE_HOST_PID, ENGINE_LIBRARY_PATH, WM_WAITER_DEADLINE, WM_WAITER_PID,
};
use core::sync::atomic::Ordering;

pub(crate) fn sys_ipc_send(dst_pid: u64, msg_ptr: u64, msg_len: u64) -> i64 {
    let bytes = match unsafe { read_user_bytes(msg_ptr, (msg_len as usize).min(64)) } {
        Some(b) => b,
        None => return -14,
    };
    crate::ipc::send(dst_pid as u32, bytes);
    0
}

pub(crate) fn sys_ipc_recv(buf_ptr: u64, buf_len: u64) -> i64 {
    let pid = crate::process::current_pid();
    if pid == 0 {
        return -11; // no active userspace pid yet
    }
    match crate::ipc::recv(pid) {
        Some(msg) => {
            let copy_len = (buf_len as usize).min(msg.len());
            if buf_ptr == 0 {
                return -14;
            }
            unsafe {
                core::ptr::copy_nonoverlapping(msg.as_ptr(), buf_ptr as *mut u8, copy_len);
            }
            copy_len as i64
        }
        None => -11, // EAGAIN
    }
}

pub(crate) fn sys_surface_create(width: u64, height: u64) -> i64 {
    match crate::compositor::create_surface_for(wm_consumer_pid(), width as u32, height as u32) {
        Ok(id) => id as i64,
        Err("invalid size") => -22, // EINVAL
        Err(_) => -12,              // ENOMEM/table full
    }
}

pub(crate) fn sys_surface_move(id: u64, packed_xy: u64, z: u64) -> i64 {
    let x = ((packed_xy >> 32) as u32) as i32;
    let y = (packed_xy as u32) as i32;
    match crate::compositor::move_surface_for(wm_consumer_pid(), id as u32, x, y, z as i32) {
        Ok(()) => 0,
        Err("permission denied") => -1, // EPERM
        Err(_) => -3,                   // ESRCH
    }
}

pub(crate) fn sys_surface_destroy(id: u64) -> i64 {
    match crate::compositor::destroy_surface_for(wm_consumer_pid(), id as u32) {
        Ok(()) => 0,
        Err("permission denied") => -1, // EPERM
        Err(_) => -3,                   // ESRCH
    }
}

pub(crate) fn sys_surface_upload(id: u64, buf_ptr: u64, buf_len: u64) -> i64 {
    let bytes = match unsafe { read_user_bytes(buf_ptr, buf_len as usize) } {
        Some(b) => b,
        None => return -14, // EFAULT
    };
    match crate::compositor::upload_surface_rgba32_for(wm_consumer_pid(), id as u32, bytes) {
        Ok(()) => 0,
        Err("bad payload size") => -22,
        Err("permission denied") => -1,
        Err("no such surface") => -3,
        Err(_) => -12,
    }
}

pub(crate) fn sys_surface_present(id: u64) -> i64 {
    static SURFACE_PRESENT_LOG: core::sync::atomic::AtomicU32 =
        core::sync::atomic::AtomicU32::new(0);
    let n = SURFACE_PRESENT_LOG.fetch_add(1, Ordering::Relaxed);
    if n < 128 {
        log::warn!(
            "[frame-boundary] surface_present #{} pid={} sid={}",
            n,
            crate::process::current_pid(),
            id
        );
    }
    match crate::compositor::present_surface_for(wm_consumer_pid(), id as u32) {
        Ok(()) => 0,
        Err("permission denied") => -1,
        Err(_) => -3,
    }
}

pub(crate) fn sys_fb_size_packed() -> i64 {
    crate::compositor::framebuffer_size_packed() as i64
}

pub(crate) fn sys_vsync_counter() -> i64 {
    crate::compositor::frame_counter() as i64
}

pub(crate) fn sys_vsync_wait_nonblock(last_seen: u64) -> i64 {
    match crate::compositor::wait_vsync(last_seen) {
        Some(v) => v as i64,
        None => -11, // EAGAIN
    }
}

pub(crate) fn sys_surface_owner(id: u64) -> i64 {
    match crate::compositor::surface_owner(id as u32) {
        Some(pid) => pid as i64,
        None => -3, // ESRCH
    }
}

pub(crate) fn sys_surface_z_get(id: u64) -> i64 {
    match crate::compositor::surface_z_get(id as u32) {
        Some(z) => z as i64,
        None => -3, // ESRCH
    }
}

pub(crate) fn sys_surface_z_set(id: u64, z: u64) -> i64 {
    match crate::compositor::surface_z_set_for(wm_consumer_pid(), id as u32, z as i32) {
        Ok(()) => 0,
        Err("permission denied") => -1, // EPERM
        Err(_) => -3,                   // ESRCH
    }
}

pub(crate) fn sys_surface_geometry_get(id: u64) -> i64 {
    match crate::compositor::surface_geometry_get(id as u32) {
        Some((x, y, w, h)) => {
            // Pack as: a[31:0]=x, a[63:32]=y; b[31:0]=w, b[63:32]=h
            // Return as two i64 halves combined: ((y as u64) << 32) | (x as u32 as u64)
            // Actually, we need to return packed geometry. Use a convention:
            // RAX returns lower 64 bits: ((x as u32 as u64) << 32) | (y as u32 as u64)
            // This is the coordinate pair.
            let x_packed = ((x as u32 as u64) << 32) | (y as u32 as u64);
            x_packed as i64
        }
        None => -3, // ESRCH
    }
}

pub(crate) fn sys_surface_geometry_set(id: u64, xy_packed: u64, wh_packed: u64) -> i64 {
    let x = ((xy_packed >> 32) as u32) as i32;
    let y = (xy_packed as u32) as i32;
    let w = ((wh_packed >> 32) as u32) as u32;
    let h = (wh_packed as u32) as u32;

    match crate::compositor::surface_geometry_set_for(wm_consumer_pid(), id as u32, x, y, w, h) {
        Ok(()) => 0,
        Err("invalid size") => -22,      // EINVAL
        Err("surface too large") => -12, // ENOMEM
        Err("permission denied") => -1,  // EPERM
        Err(_) => -3,                    // ESRCH
    }
}

pub(crate) fn sys_surface_visibility_get(id: u64) -> i64 {
    match crate::compositor::surface_visibility_get(id as u32) {
        Some(visible) => {
            if visible {
                1
            } else {
                0
            }
        }
        None => -3, // ESRCH
    }
}

pub(crate) fn sys_surface_visibility_set(id: u64, visible: u64) -> i64 {
    match crate::compositor::surface_visibility_set_for(wm_consumer_pid(), id as u32, visible != 0)
    {
        Ok(()) => 0,
        Err("permission denied") => -1,
        Err(_) => -3,
    }
}

pub(crate) fn sys_surface_clip_set(id: u64, xy_packed: u64, wh_packed: u64) -> i64 {
    let x = ((xy_packed >> 32) as u32) as i32;
    let y = (xy_packed as u32) as i32;
    let w = ((wh_packed >> 32) as u32) as u32;
    let h = (wh_packed as u32) as u32;

    match crate::compositor::surface_clip_set_for(wm_consumer_pid(), id as u32, x, y, w, h) {
        Ok(()) => 0,
        Err("permission denied") => -1,
        Err(_) => -3,
    }
}

pub(crate) fn sys_surface_damage_set(id: u64, xy_packed: u64, wh_packed: u64) -> i64 {
    let x = ((xy_packed >> 32) as u32) as i32;
    let y = (xy_packed as u32) as i32;
    let w = ((wh_packed >> 32) as u32) as u32;
    let h = (wh_packed as u32) as u32;
    match crate::compositor::surface_damage_set_for(wm_consumer_pid(), id as u32, x, y, w, h) {
        Ok(()) => 0,
        Err("permission denied") => -1,
        Err(_) => -3,
    }
}

pub(crate) fn sys_surface_damage_get(id: u64) -> i64 {
    match crate::compositor::surface_damage_get(id as u32) {
        Some((x, y, _w, _h, true)) => {
            // Return packed xy; caller uses separate surface_geometry syscall for dimensions.
            ((x as u32 as u64) << 32 | y as u32 as u64) as i64
        }
        Some((_, _, _, _, false)) => -11, // EAGAIN — no pending damage
        None => -3,                       // ESRCH
    }
}

pub(crate) fn sys_surface_flip(id: u64) -> i64 {
    match crate::compositor::surface_flip_for(wm_consumer_pid(), id as u32) {
        Ok(()) => 0,
        Err("permission denied") => -1,
        Err(_) => -3,
    }
}

pub(crate) fn sys_app_notify(target_pid: u64, subkind: u64, surface_id: u64) -> i64 {
    crate::wm::push_app_event(target_pid as u32, subkind as u32, surface_id as u32);
    0
}

pub(crate) fn sys_proc_surface_count(pid: u64) -> i64 {
    crate::compositor::surface_count_for(pid as u32) as i64
}

pub(crate) fn sys_app_launch_path(path_ptr: u64, path_len: u64, flags: u64) -> i64 {
    let bytes = match unsafe { read_user_bytes(path_ptr, path_len as usize) } {
        Some(b) => b,
        None => return -14,
    };
    let path = match core::str::from_utf8(bytes) {
        Ok(s) => s,
        Err(_) => return -22,
    };
    let elf = match crate::fs::lookup(path) {
        Some(data) => data,
        None => return -2,
    };

    let mode = flags & 0xFF;
    let app_id = flags >> 32;
    let rdi = if mode == 0 {
        crate::app_registry::HOST_MODE_SHELL
    } else {
        mode
    };
    let rsi = if rdi == crate::app_registry::HOST_MODE_APP {
        app_id
    } else {
        0
    };

    let bootstrap = crate::process::SpawnBootstrap {
        rdi,
        rsi,
        rdx: 0,
        parent_pid: crate::process::current_pid().max(1),
    };

    match crate::process::spawn_with_bootstrap(elf, path, bootstrap) {
        Ok(pid) => {
            if rdi == crate::app_registry::HOST_MODE_APP && app_id != 0 {
                let (aot_va, _) =
                    match crate::app_registry::map_aot_into_process(app_id as u32, pid) {
                        Ok(v) => v,
                        Err(e) => {
                            let _ = crate::process::kill(pid);
                            return e;
                        }
                    };
                crate::process::set_bootstrap_regs(pid, rdi, rsi, aot_va);
            }
            crate::wm::push_app_event(pid, eabi::APP_LAUNCH, 0);
            let host_pid = ENGINE_HOST_PID.load(Ordering::Acquire);
            if host_pid != 0 {
                crate::wm::push_event_for(host_pid, eabi::EV_APP, eabi::APP_LAUNCH, pid as u64, 0);
            }
            pid as i64
        }
        Err(_) => -12,
    }
}

pub(crate) fn sys_engine_policy_get() -> i64 {
    // packed: [63:32]=engine target family, [31:0]=loader strategy
    let packed =
        ((eabi::ENGINE_TARGET_FLUTTER_3_29 as u64) << 32) | (eabi::ENGINE_LOADER_DYNAMIC as u64);
    packed as i64
}

pub(crate) fn sys_engine_version_packed() -> i64 {
    // Flutter 3.29.0 baseline.
    let packed = (3u64 << 32) | (29u64 << 16);
    packed as i64
}

pub(crate) fn sys_engine_host_register(_flags: u64) -> i64 {
    let current = crate::process::current_pid();
    if current == 0 {
        return -1; // EPERM (kernel context cannot be engine host)
    }
    let pid = crate::process::get_group_leader(current);
    ENGINE_HOST_PID.store(pid, Ordering::Release);
    crate::wm::set_focus_pid(pid);
    pid as i64
}

pub(crate) fn sys_engine_host_pid_get() -> i64 {
    ENGINE_HOST_PID.load(Ordering::Acquire) as i64
}

pub(crate) fn sys_engine_library_path_read(dst_ptr: u64, dst_len: u64) -> i64 {
    let bytes = ENGINE_LIBRARY_PATH.as_bytes();
    if (dst_len as usize) < bytes.len() {
        return -22; // EINVAL
    }
    if unsafe { write_user_bytes(dst_ptr, bytes) } {
        bytes.len() as i64
    } else {
        -14 // EFAULT
    }
}

#[inline(always)]
pub(crate) fn wm_consumer_pid() -> u32 {
    let pid = crate::process::current_pid();
    if pid == 0 {
        1
    } else {
        crate::process::get_group_leader(pid)
    }
}

pub(crate) fn sys_wm_event_poll() -> i64 {
    crate::wm::pending_count_for(wm_consumer_pid()) as i64
}

pub(crate) fn sys_wm_event_read(ev_ptr: u64, ev_len: u64) -> i64 {
    wm_event_copy_to_user(ev_ptr, ev_len, false)
}

fn wm_wait_drain_event(ev_ptr: u64, ev_len: u64) -> i64 {
    wm_event_copy_to_user(ev_ptr, ev_len, true)
}

static WM_LAST_DRAIN_SLACK: core::sync::atomic::AtomicBool =
    core::sync::atomic::AtomicBool::new(false);

#[inline]
fn wm_drain_requested_platform_poll() -> bool {
    WM_LAST_DRAIN_SLACK.swap(false, Ordering::AcqRel)
}

fn wm_event_copy_to_user(ev_ptr: u64, ev_len: u64, platform_poll_slack: bool) -> i64 {
    WM_LAST_DRAIN_SLACK.store(false, Ordering::Release);

    let need = crate::wm::event_size();
    if (ev_len as usize) < need {
        return -22; // EINVAL
    }

    let consumer = wm_consumer_pid();
    let ev = match crate::wm::pop_event_for(consumer) {
        Some(e) => e,
        None => return -11, // EAGAIN
    };

    if platform_poll_slack && ev.kind == crate::wm::EV_VSYNC && ev.b == 0 {
        static VSYNC_POLL_SLACK: core::sync::atomic::AtomicU32 =
            core::sync::atomic::AtomicU32::new(0);
        let t = VSYNC_POLL_SLACK.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
        if (t & 1) == 0 {
            crate::wm::push_event(ev.kind, ev.flags, ev.a, ev.b);
            WM_LAST_DRAIN_SLACK.store(true, Ordering::Release);
            return -11;
        }
    }

    let bytes = unsafe {
        core::slice::from_raw_parts((&ev as *const crate::wm::WmEvent) as *const u8, need)
    };
    if unsafe { write_user_bytes(ev_ptr, bytes) } {
        need as i64
    } else {
        -14 // EFAULT
    }
}

pub(crate) fn sys_wm_event_inject(kind: u64, arg1: u64, arg2: u64) -> i64 {
    let caller = wm_consumer_pid();
    match kind as u32 {
        eabi::EV_POINTER => {
            let x = ((arg1 >> 32) as u32) as i32;
            let y = (arg1 as u32) as i32;
            crate::wm::push_pointer(x, y, arg2 as u32);
            0
        }
        eabi::EV_KEY => {
            crate::wm::push_key(arg1 as u32, arg2 != 0);
            0
        }
        eabi::EV_APP => {
            crate::wm::push_event_for(caller, kind as u32, 0, arg1, arg2);
            0
        }
        eabi::EV_VSYNC => {
            crate::wm::push_event(kind as u32, 0, arg1, arg2);
            0
        }
        _ => -22,
    }
}

pub(crate) fn sys_embedder_abi_version() -> i64 {
    eabi::EMBEDDER_ABI_VERSION as i64
}

pub(crate) fn sys_wm_event_size() -> i64 {
    eabi::WM_EVENT_SIZE as i64
}

pub(crate) fn sys_wm_event_stats_packed() -> i64 {
    let pending = crate::wm::pending_count_for(wm_consumer_pid()).min(u32::MAX as usize) as u64;
    let dropped = crate::wm::dropped_count().min(u32::MAX as u64);
    // packed: (dropped << 32) | pending
    ((dropped << 32) | pending) as i64
}

pub(crate) fn sys_wm_focus_pid_get() -> i64 {
    crate::wm::focus_pid() as i64
}

pub(crate) fn sys_wm_focus_surface_set(surface_id: u64) -> i64 {
    let caller = wm_consumer_pid();
    let owner = match crate::compositor::surface_owner(surface_id as u32) {
        Some(pid) => pid,
        None => return -3, // ESRCH
    };
    if owner != caller {
        return -1; // EPERM
    }
    crate::wm::set_focus_pid(owner);
    owner as i64
}

pub(crate) fn sys_wm_focus_mirror_get() -> i64 {
    if crate::wm::focus_mirror_enabled() {
        1
    } else {
        0
    }
}

pub(crate) fn sys_wm_focus_mirror_set(enabled: u64) -> i64 {
    let on = enabled != 0;
    crate::wm::set_focus_mirror_enabled(on);
    if on {
        1
    } else {
        0
    }
}

pub(crate) fn sys_wm_event_wait(ev_ptr: u64, ev_len: u64, timeout_ms: u64) -> i64 {
    let cur = crate::process::current_pid();
    if cur == 0 {
        return -1;
    }

    // The embedder only calls wm_event_wait once it has finished engine init and
    // entered its main event loop.  Mark init complete so the cooperative
    // fairness-yield in epoll_wait is allowed to run (it must stay disabled
    // during init or it deadlocks the engine threads that drive FlutterEngineRunInitialized).
    if !crate::wm::flutter_init_ready() {
        crate::wm::set_flutter_init_ready();
    }

    static WM_WAIT_RET_LOG: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
    let mut wm_log_ret = |tag: &str, val: i64| {
        let n = WM_WAIT_RET_LOG.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
        if n < 24 {
            log::warn!("[wm-event-wait-ret] #{} pid={} {}={}", n, cur, tag, val);
        }
        val
    };

    // Ensure timerfd deadlines are checked on every event-poll, not just on WM
    // timeout expiry.  This keeps task-runner threads woken at ≤16ms latency
    // instead of the ~640ms cortex-bg cycle.
    check_timerfds_and_wake();

    // Cooperative yield via enter_user resumes userspace at the syscall site
    // without completing the prior in-kernel wait loop — always drain the queue
    // and honor a deferred timeout before re-blocking.
    let n = wm_wait_drain_event(ev_ptr, ev_len);
    if n >= 0 {
        crate::wm::set_wm_waiter(0);
        WM_WAITER_PID.store(0, Ordering::Release);
        WM_WAITER_DEADLINE.store(0, Ordering::Release);
        crate::process::set_state(cur, crate::process::ProcState::Running);
        return wm_log_ret("event", n);
    }
    if n == -11 && wm_drain_requested_platform_poll() {
        return wm_log_ret("eagain-platform", -11);
    }
    if WM_WAITER_PID.load(Ordering::Acquire) == cur {
        let dl = WM_WAITER_DEADLINE.load(Ordering::Relaxed);
        if dl != 0 && monotonic_ns() >= dl {
            crate::wm::set_wm_waiter(0);
            WM_WAITER_PID.store(0, Ordering::Release);
            WM_WAITER_DEADLINE.store(0, Ordering::Release);
            crate::process::set_state(cur, crate::process::ProcState::Running);
            return wm_log_ret("eagain-top", -11);
        }
    }

    let is_reentrant = WM_WAITER_PID.load(Ordering::Acquire) == cur;

    // On a fresh (non-reentrant) wm_event_wait: kick task runners when the APIC
    // ISR has flagged a deferred kick (~500 ms).
    if !is_reentrant {
        let kicked = KICK_REQUESTED.swap(false, Ordering::AcqRel);
        if kicked {
            let _ = force_wake_all_task_runners("wm-kick");
        }
    }
    let deadline_ns = if is_reentrant {
        WM_WAITER_DEADLINE.load(Ordering::Relaxed)
    } else {
        if timeout_ms == 0 {
            return -11; // EAGAIN immediately if timeout is 0 and no event is ready
        }

        let dl = monotonic_ns().saturating_add(timeout_ms.saturating_mul(1_000_000));
        WM_WAITER_PID.store(cur, Ordering::Release);
        WM_WAITER_DEADLINE.store(dl, Ordering::Release);
        dl
    };

    loop {
        // Set the waiter and block before checking, to close lost-wake race window.
        crate::wm::set_wm_waiter(cur);
        crate::process::set_state(cur, crate::process::ProcState::Blocked);

        static WM_WAIT_BLOCK_LOG: core::sync::atomic::AtomicU32 =
            core::sync::atomic::AtomicU32::new(0);
        let block_log_n = WM_WAIT_BLOCK_LOG.fetch_add(1, Ordering::Relaxed);
        if block_log_n < 32 {
            log::warn!(
                "[wm-wait-block] #{} pid={} timeout_ms={} deadline={} pending={} input={} baton_due={} baton_queued={}",
                block_log_n,
                cur,
                timeout_ms,
                deadline_ns,
                crate::wm::pending_count_for(cur),
                crate::wm::input_pending_for(cur),
                crate::wm::embedder_baton_due(),
                crate::wm::baton_vsync_queued_for(1)
            );
        }

        if block_log_n < 32 {
            log::warn!("[wm-wait-drain-before] #{} pid={}", block_log_n, cur);
        }
        let n = wm_wait_drain_event(ev_ptr, ev_len);
        if block_log_n < 32 {
            log::warn!("[wm-wait-drain-after] #{} pid={} n={}", block_log_n, cur, n);
        }
        if n >= 0 {
            crate::wm::set_wm_waiter(0);
            WM_WAITER_PID.store(0, Ordering::Release);
            WM_WAITER_DEADLINE.store(0, Ordering::Release);
            crate::process::set_state(cur, crate::process::ProcState::Running);
            return wm_log_ret("event-loop", n);
        }
        if n == -11 && wm_drain_requested_platform_poll() {
            crate::wm::set_wm_waiter(0);
            WM_WAITER_PID.store(0, Ordering::Release);
            WM_WAITER_DEADLINE.store(0, Ordering::Release);
            crate::process::set_state(cur, crate::process::ProcState::Running);
            return wm_log_ret("eagain-platform-loop", -11);
        }

        // Check if we already exceeded the timeout before blocking.
        let now = monotonic_ns();
        if deadline_ns == 0 || now >= deadline_ns {
            crate::wm::set_wm_waiter(0);
            WM_WAITER_PID.store(0, Ordering::Release);
            WM_WAITER_DEADLINE.store(0, Ordering::Release);
            crate::process::set_state(cur, crate::process::ProcState::Running);
            return wm_log_ret("eagain-loop", -11); // EAGAIN
        }

        // Save return context and registers so we can be resumed.
        let urip = crate::arch::syscall::user_rip();
        let ursp = crate::arch::syscall::user_rsp();
        crate::process::save_return_context_reexec(cur, urip, ursp);
        crate::process::save_full_user_gprs(cur);
        crate::process::set_rax(cur, eabi::SYS_WM_EVENT_WAIT);
        if block_log_n < 32 {
            log::warn!("[wm-wait-xstate-before] #{} pid={}", block_log_n, cur);
        }
        crate::process::save_xstate(cur);
        let wait_irq_flags = crate::arch::interrupts_save_and_disable();
        if block_log_n < 32 {
            log::warn!("[wm-wait-xstate-after] #{} pid={}", block_log_n, cur);
        }

        let pending_after_save = crate::wm::pending_count_for(cur);
        let baton_after_save = crate::wm::baton_vsync_queued_for(1);
        if block_log_n < 32 {
            log::warn!(
                "[wm-wait-post-save] #{} pid={} pending={} baton={}",
                block_log_n,
                cur,
                pending_after_save,
                baton_after_save
            );
        }
        if pending_after_save > 0 || baton_after_save {
            if block_log_n < 32 {
                log::warn!(
                    "[wm-wait-race] #{} pid={} pending={} baton_queued={} -> redrain",
                    block_log_n,
                    cur,
                    pending_after_save,
                    baton_after_save
                );
            }
            crate::process::set_state(cur, crate::process::ProcState::Running);
            crate::arch::interrupts_restore(wait_irq_flags);
            continue;
        }

        // Cooperatively switch to another runnable process.
        let mut next = crate::process::next_runnable_pid(cur);
        if next.is_none() {
            let (_fds, woken) = force_wake_all_task_runners("wm-wait-no-runnable");
            if woken > 0 {
                next = crate::process::next_runnable_pid(cur);
            }
        }
        if let Some(next) = next {
            if next != cur {
                crate::process::enter_user_by_pid_noreturn(next);
            }
        }
        crate::arch::interrupts_restore(wait_irq_flags);

        // Loop-sleep (sti; hlt; cli) as long as we are Blocked.
        // Cooperative yield already happened once above.  Re-yield only every
        // 16th hlt wake so reentrant wm_event_wait can hit eagain-top for
        // platform-recv without the sched ping-pong that starved pid=2 init.
        while crate::process::is_blocked(cur) {
            #[cfg(target_arch = "x86_64")]
            unsafe {
                { crate::arch::enable_and_halt(); }
            }

            if !crate::process::is_blocked(cur) {
                break;
            }

            // Check if timeout has expired.
            let now = monotonic_ns();
            let current_dl = WM_WAITER_DEADLINE.load(Ordering::Relaxed);
            if current_dl == 0 || now >= current_dl {
                crate::wm::set_wm_waiter(0);
                WM_WAITER_PID.store(0, Ordering::Release);
                WM_WAITER_DEADLINE.store(0, Ordering::Release);
                crate::process::set_state(cur, crate::process::ProcState::Running);
                break;
            }

            if let Some(next) = crate::process::next_runnable_pid(cur) {
                if next != cur {
                    crate::process::enter_user_by_pid_noreturn(next);
                }
            }
        }
    }
}
