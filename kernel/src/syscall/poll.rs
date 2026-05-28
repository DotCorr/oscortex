use super::state::{CondWaitState, COND_WAIT_STATE, FUTEX_WAITERS, WM_WAITER_DEADLINE, WM_WAITER_PID};
use super::tables::pipe_readable_for_fd;
use super::trace::{POSTEXIT_TRACE_ACTIVE, POSTEXIT_TRACE_COUNT, POSTEXIT_TRACE_LIMIT};
use super::wait::futex_waiter_remove;
use alloc::collections::BTreeMap;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};

/// Deferred task-runner kick flag (set from APIC timer ISR).
pub static KICK_REQUESTED: AtomicBool = AtomicBool::new(false);
/// Set when a pthread_cond waiter timed out or must finish AcquiringMutex but
/// cooperative scheduling may be starving it (no userspace timer preemption).
pub static COND_RESCHED_NEEDED: AtomicBool = AtomicBool::new(false);

/// Yield to a sibling that needs to finish a cond-wait state machine step.
/// Called from high-frequency syscall paths (timerfd read) where other threads
/// may otherwise never receive a cooperative schedule point.
pub fn cooperative_yield_for_cond_resched(cur: u32, sys_nr: u64) {
    if cur == 0 || !COND_RESCHED_NEEDED.swap(false, Ordering::AcqRel) {
        return;
    }
    let target = {
        let cws = COND_WAIT_STATE.lock();
        cws.iter().find_map(|(&pid, st)| {
            if pid != cur && matches!(st, CondWaitState::AcquiringMutex { .. }) {
                Some(pid)
            } else {
                None
            }
        })
    };
    let Some(next) = target.or_else(|| crate::process::next_runnable_pid(cur)) else {
        return;
    };
    if next == cur {
        return;
    }
    let urip = crate::arch::syscall::user_rip();
    let ursp = crate::arch::syscall::user_rsp();
    crate::process::save_return_context(cur, urip, ursp);
    crate::process::save_full_user_gprs(cur);
    crate::process::set_rax(cur, sys_nr);
    crate::process::save_xstate(cur);
    crate::process::enter_user_by_pid_noreturn(next);
}

// ── Synthetic FD allocator ────────────────────────────────────────────────────
// epoll_create / inotify_init1 / timerfd_create return integer fds that
// user space then stores in std::map<fd, T> keyed event loops. Returning the
// same constant for two different objects causes map::at() to fail
// ("key not found") when one overwrites the other. Hand out unique numbers
// from a private high range that won't collide with real sys_open fds.
static SYNTH_FD_NEXT: AtomicU32 = AtomicU32::new(64);
#[inline]
pub(crate) fn alloc_synth_fd() -> i64 {
    SYNTH_FD_NEXT.fetch_add(1, Ordering::Relaxed) as i64
}

// ── Real-ish timerfd + epoll support ──────────────────────────────────────────
// User-space event loops (Flutter, Dart isolates) rely on:
//   timerfd_create() → fd
//   timerfd_settime(fd, flags, &itimerspec{it_interval, it_value}, NULL)
//   epoll_ctl(epfd, ADD, fd, &event)
//   epoll_wait(epfd, events, maxevents, timeout_ms)  → expects EPOLLIN when
//   the timerfd's deadline has passed.
// Stubbing epoll_wait to return 0 immediately starves the event loop and the
// downstream Dart code aborts. We track timerfd deadlines and per-epfd
// interest sets so epoll_wait can return real events.

#[derive(Clone, Copy)]
pub(crate) struct TimerState {
    pub deadline_ns: u64,
    pub period_ns: u64,
    pub pending: u64,
}

#[derive(Clone, Copy)]
pub(crate) struct EpollEntry {
    pub fd: u32,
    pub events: u32,
    pub data: u64,
}

pub(crate) static TIMERFD_TABLE: spin::Mutex<BTreeMap<u32, TimerState>> = spin::Mutex::new(BTreeMap::new());
/// eventfd counters: synth fd → counter (u64). A write adds to the counter;
/// a read drains it atomically. epoll fires EPOLLIN when counter > 0.
pub(crate) static EVENTFD_TABLE: spin::Mutex<BTreeMap<u32, u64>> = spin::Mutex::new(BTreeMap::new());
pub(crate) static EPOLL_TABLE: spin::Mutex<BTreeMap<u32, Vec<EpollEntry>>> = spin::Mutex::new(BTreeMap::new());
/// Threads blocked in epoll_wait(timeout=-1): pid → epfd they're waiting on.
static EPOLL_BLOCKED: spin::Mutex<BTreeMap<u32, u32>> = spin::Mutex::new(BTreeMap::new());

/// Wake every thread that is blocked in epoll_wait and whose epoll watches `tfd`.
/// Called after a timerfd is armed. The blocked thread already has rax=1 and a
/// fake EPOLLIN event written to its events_out buffer; it will resume and call
/// TimerDrain, which will then successfully read the now-armed timerfd.
pub(crate) fn epoll_wake(fd: u32) {
    // Step 1: find all epfds that watch this fd (lock EPOLL_TABLE, then drop).
    let watching_epfds: Vec<u32> = {
        let epoll_tbl = EPOLL_TABLE.lock();
        epoll_tbl.iter()
            .filter(|(_, entries)| entries.iter().any(|e| e.fd == fd))
            .map(|(&epfd, _)| epfd)
            .collect()
    };
    if watching_epfds.is_empty() { return; }
    // Step 2: collect pids to wake from EPOLL_BLOCKED (separate lock acquisition).
    let pids_to_wake: Vec<u32> = {
        let mut blocked = EPOLL_BLOCKED.lock();
        let pids: Vec<u32> = blocked.iter()
            .filter(|(_, &epfd)| watching_epfds.contains(&epfd))
            .map(|(&pid, _)| pid)
            .collect();
        for &pid in &pids { blocked.remove(&pid); }
        pids
    };
    for pid in pids_to_wake {
        // rax=1 and the fake event were written before yielding.
        crate::process::set_state(pid, crate::process::ProcState::Running);
    }
}

/// Force every armed (or even disarmed) timerfd in the system to fire one
/// expiration RIGHT NOW, and wake every thread parked in epoll_wait whose
/// interest set includes it. This is the chicken-and-egg breaker for Flutter
/// embedder bring-up: pid 1 spins in cond_broadcast waiting for its task
/// runners (pids 2/3/4) to consume queued tasks, but those runners sit in
/// epoll_wait on timerfds that pid 1 never gets around to arming because
/// it is stuck in the broadcast spin. We unblock by simulating a WakeUp()
/// on every task runner.
///
/// Logs how many timerfds were poked and how many epoll waiters were released.
pub fn force_wake_all_task_runners(reason: &str) -> (usize, usize) {
    // Snapshot all timerfd ids.
    let tfds: Vec<u32> = {
        let tbl = TIMERFD_TABLE.lock();
        tbl.keys().copied().collect()
    };
    // Mark EVERY timerfd as pending (one expiration), armed or not.
    // CRITICAL: disarmed timerfds (deadline_ns=0, pending=0) are treated as
    // ready by epoll_collect_ready when pending>0.  This breaks the stall
    // where the raster thread (pid=3) holds a mutex while blocked in an
    // infinite epoll_wait on a disarmed timerfd: incrementing pending causes
    // epoll_wait to return, pid=3 runs, and eventually releases the mutex.
    let mut all_tfds = Vec::new();
    {
        let mut tbl = TIMERFD_TABLE.lock();
        for fd in &tfds {
            if let Some(t) = tbl.get_mut(fd) {
                t.pending = t.pending.saturating_add(1);
                all_tfds.push(*fd);
            }
        }
    }
    // Also poke any eventfds (Flutter MessageLoopLinux WakeUp() writes 1 to one).
    let efds: Vec<u32> = {
        let tbl = EVENTFD_TABLE.lock();
        tbl.keys().copied().collect()
    };
    {
        let mut tbl = EVENTFD_TABLE.lock();
        for fd in &efds {
            if let Some(c) = tbl.get_mut(fd) {
                *c = c.saturating_add(1);
            }
        }
    }
    // Release every epoll waiter whose epfd watches any of these fds.
    let touched: Vec<u32> = all_tfds.iter().chain(efds.iter()).copied().collect();
    let watching_epfds: Vec<u32> = {
        let epoll_tbl = EPOLL_TABLE.lock();
        epoll_tbl.iter()
            .filter(|(_, entries)| entries.iter().any(|e| touched.contains(&e.fd)))
            .map(|(&epfd, _)| epfd)
            .collect()
    };
    let pids_to_wake: Vec<u32> = {
        let mut blocked = EPOLL_BLOCKED.lock();
        let pids: Vec<u32> = blocked.iter()
            .filter(|(_, &epfd)| watching_epfds.contains(&epfd))
            .map(|(&pid, _)| pid)
            .collect();
        for &pid in &pids { blocked.remove(&pid); }
        pids
    };
    let woken = pids_to_wake.len();
    for pid in pids_to_wake {
        crate::process::set_state(pid, crate::process::ProcState::Running);
    }
    static FORCE_WAKE_LOG: core::sync::atomic::AtomicU32 =
        core::sync::atomic::AtomicU32::new(0);
    let n = FORCE_WAKE_LOG.fetch_add(1, Ordering::Relaxed);
    if n < 8 {
        log::trace!(
            "[force-wake] #{} reason={} timerfds={} (all poked) eventfds={} epoll_waiters_released={}",
            n, reason, all_tfds.len(), efds.len(), woken
        );
    }
    (tfds.len() + efds.len(), woken)
}

/// Complete a timed-out pthread_cond_timedwait without re-entering the full
/// state machine (cooperative scheduling may never run the thread again).
fn inject_cond_timedout_return(pid: u32, mutex: u64) {
    if pid == 0 {
        return;
    }
    let atom = unsafe { &*(mutex as *const core::sync::atomic::AtomicU32) };
    let _ = atom.compare_exchange(0, 1, Ordering::Acquire, Ordering::Acquire);
    if let Some((rip, rsp)) = crate::process::get_saved_rip_rsp(pid) {
        // Yield paths save RIP at the SYSCALL insn; advance past it.
        crate::process::save_return_context(pid, rip.wrapping_add(2), rsp);
    }
    crate::process::set_rax(pid, 110); // ETIMEDOUT
    COND_WAIT_STATE.lock().remove(&pid);
    crate::process::set_state(pid, crate::process::ProcState::Running);
    COND_RESCHED_NEEDED.store(true, Ordering::Release);
    static INJECT_LOG: AtomicU32 = AtomicU32::new(0);
    let n = INJECT_LOG.fetch_add(1, Ordering::Relaxed);
    if n < 16 || n % 64 == 0 {
        log::warn!("[cond-timeout-inject] #{} pid={} mutex={:#x}", n, pid, mutex);
    }
}

pub fn check_timerfds_and_wake() {
    let now = monotonic_ns();
    let mut to_wake = Vec::new();
    {
        let mut tbl = TIMERFD_TABLE.lock();
        for (&fd, t) in tbl.iter_mut() {
            if t.deadline_ns != 0 && now >= t.deadline_ns {
                if t.period_ns != 0 {
                    let elapsed = now.saturating_sub(t.deadline_ns);
                    let n = 1u64 + elapsed / t.period_ns;
                    t.deadline_ns = t.deadline_ns.saturating_add(n * t.period_ns);
                    t.pending = t.pending.saturating_add(n);
                } else {
                    t.deadline_ns = 0; // disarm one-shot
                    t.pending = t.pending.saturating_add(1);
                }
                to_wake.push(fd);
            }
        }
    }
    for fd in to_wake {
        epoll_wake(fd);
    }

    // Expire timed-out pthread_cond_timedwait waiters.
    // Without this, Blocked threads whose deadline passed never get rescheduled
    // because next_runnable_pid only considers ProcState::Running threads and
    // the cond-wait state machine only advances when re-entered via syscall.
    let mut expired: Vec<(u32, u64, u64)> = Vec::new(); // (pid, cond, mutex)
    {
        let mut cws = COND_WAIT_STATE.lock();
        for (&pid, state) in cws.iter_mut() {
            if let CondWaitState::Waiting { cond, mutex, timeout_ns, .. } = *state {
                if timeout_ns != 0 && now >= timeout_ns {
                    *state = CondWaitState::AcquiringMutex { mutex, timed_out: true };
                    expired.push((pid, cond, mutex));
                }
            }
        }
    }
    for (pid, cond, mutex) in expired {
        futex_waiter_remove(cond, pid);
        inject_cond_timedout_return(pid, mutex);
        static TIMEOUT_EXPIRE_LOG: core::sync::atomic::AtomicU32 =
            core::sync::atomic::AtomicU32::new(0);
        let n = TIMEOUT_EXPIRE_LOG.fetch_add(1, Ordering::Relaxed);
        if n < 16 || n % 64 == 0 {
            log::warn!("[cond-timeout-expire] #{} pid={} cond={:#x}", n, pid, cond);
        }
    }

    // Finish any timed-out cond waiters stuck in AcquiringMutex (mutex free
    // but thread never got a cooperative schedule point).
    {
        let stuck: alloc::vec::Vec<(u32, u64)> = {
            let cws = COND_WAIT_STATE.lock();
            cws.iter()
                .filter_map(|(&pid, st)| {
                    if let CondWaitState::AcquiringMutex {
                        mutex,
                        timed_out: true,
                    } = st
                    {
                        let mutex_addr = *mutex;
                        let atom = unsafe {
                            &*(mutex_addr as *const core::sync::atomic::AtomicU32)
                        };
                        if atom.load(Ordering::Acquire) == 0 {
                            Some((pid, mutex_addr))
                        } else {
                            None
                        }
                    } else {
                        None
                    }
                })
                .collect()
        };
        for (pid, mutex) in stuck {
            inject_cond_timedout_return(pid, mutex);
        }
    }

    // Expire timed-out WM event waiters.
    let wm_deadline = WM_WAITER_DEADLINE.load(Ordering::Acquire);
    if wm_deadline != 0 && now >= wm_deadline {
        WM_WAITER_DEADLINE.store(0, Ordering::Release);
        let wm_waiter = WM_WAITER_PID.load(Ordering::Acquire);
        if wm_waiter != 0 {
            crate::wm::set_wm_waiter(0);
            crate::process::set_state(wm_waiter, crate::process::ProcState::Running);
            static WM_TIMEOUT_EXPIRE_LOG: core::sync::atomic::AtomicU32 =
                core::sync::atomic::AtomicU32::new(0);
            let n = WM_TIMEOUT_EXPIRE_LOG.fetch_add(1, Ordering::Relaxed);
            if n < 16 || n % 64 == 0 {
                log::warn!("[wm-timeout-expire] #{} pid={}", n, wm_waiter);
            }
        }
    }
}

#[inline]
pub fn monotonic_ns() -> u64 {
    // Must match sys_clock_gettime(CLOCK_MONOTONIC) exactly.
    // sys_clock_gettime returns secs = 1_700_000_000 + tsc_ns/10^9,
    // so the absolute nanosecond timestamp is:
    //   1_700_000_000 * 10^9 + tsc_ns
    // timerfd_settime with TFD_TIMER_ABSTIME passes this value, so our
    // "now" must use the same epoch or timers will never fire.
    let lo: u32;
    let hi: u32;
    unsafe { core::arch::asm!("rdtsc", out("eax") lo, out("edx") hi, options(nomem, nostack)); }
    let tsc_ns = (((hi as u64) << 32) | lo as u64) / 3;
    tsc_ns.saturating_add(1_700_000_000u64 * 1_000_000_000u64)
}

pub(crate) fn sys_timerfd_create_real() -> i64 {
    let fd = alloc_synth_fd();
    TIMERFD_TABLE.lock().insert(fd as u32, TimerState { deadline_ns: 0, period_ns: 0, pending: 0 });
    {
        static TFD_LOG: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
        if TFD_LOG.fetch_add(1, core::sync::atomic::Ordering::Relaxed) < 8 {
            log::warn!("[timerfd_create] pid={} → fd={}", crate::process::current_pid(), fd);
        }
    }
    fd
}

/// eventfd2(initval, flags) — allocates an eventfd.
/// Used by Flutter's MessageLoopLinux as a wakeup fd (write 1 → epoll fires).
pub(crate) fn sys_eventfd2(initval: u64, _flags: u64) -> i64 {
    let fd = alloc_synth_fd();
    EVENTFD_TABLE.lock().insert(fd as u32, initval);
    log::warn!("[eventfd2] pid={} → fd={} initval={}", crate::process::current_pid(), fd, initval);
    fd
}

/// Write to an eventfd: add the u64 value to its counter and wake epoll waiters.
pub(crate) fn eventfd_write(fd: u32, val: u64) {
    let new_count = {
        let mut tbl = EVENTFD_TABLE.lock();
        if let Some(c) = tbl.get_mut(&fd) {
            *c = c.saturating_add(val.max(1));
            *c
        } else {
            return;
        }
    };
    {
        static EFD_WRITE_LOG: core::sync::atomic::AtomicU32 =
            core::sync::atomic::AtomicU32::new(0);
        let n = EFD_WRITE_LOG.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
        if n < 64 {
            log::warn!(
                "[eventfd-write] #{} pid={} fd={} val={} new_count={}",
                n,
                crate::process::current_pid(),
                fd,
                val.max(1),
                new_count
            );
        }
    }
    if new_count > 0 {
        epoll_wake(fd);
    }
}

/// flags bit 0 = TFD_TIMER_ABSTIME. new_value points to itimerspec:
///   it_interval { tv_sec:i64, tv_nsec:i64 }
///   it_value    { tv_sec:i64, tv_nsec:i64 }
pub(crate) fn sys_timerfd_settime_real(fd: u64, flags: u64, new_value: u64, _old_value: u64) -> i64 {
    // Rate-limited log for EVERY timerfd_settime call (not just already-expired).
    static TFD_SETTIME_LOG: core::sync::atomic::AtomicU32 =
        core::sync::atomic::AtomicU32::new(0);
    if new_value == 0 { return -22; }
    let (int_s, int_ns, val_s, val_ns) = unsafe {
        (
            core::ptr::read_unaligned(new_value as *const i64),
            core::ptr::read_unaligned((new_value + 8) as *const i64),
            core::ptr::read_unaligned((new_value + 16) as *const i64),
            core::ptr::read_unaligned((new_value + 24) as *const i64),
        )
    };
    let raw_bytes = unsafe { core::slice::from_raw_parts(new_value as *const u8, 32) };
    let n = TFD_SETTIME_LOG.load(Ordering::Relaxed);
    if n < 0 {
        log::warn!("[tfd-debug] fd={} raw: {:x?}", fd, raw_bytes);
        log::warn!("[tfd-debug] fd={} fields: int_s={} int_ns={} val_s={} val_ns={}", fd, int_s, int_ns, val_s, val_ns);
    }
    let mut period_ns = (int_s.max(0) as u64).saturating_mul(1_000_000_000)
        .saturating_add(int_ns.max(0) as u64);
    let value_ns = (val_s.max(0) as u64).saturating_mul(1_000_000_000)
        .saturating_add(val_ns.max(0) as u64);
    if (flags & 1 != 0) && value_ns > 0 && period_ns == value_ns {
        // Treat as a one-shot timer if it was set via TFD_TIMER_ABSTIME and it_interval == it_value
        period_ns = 0;
    }
    let now = monotonic_ns();
    let mut deadline = if value_ns == 0 {
        0 // POSIX: if it_value is zero, the timer is disarmed.
    } else if flags & 1 != 0 {
        value_ns // TFD_TIMER_ABSTIME: caller already has absolute ns timestamp
    } else {
        now.saturating_add(value_ns)
    };
    let original_expired = deadline != 0 && deadline <= now;
    // When a one-shot timer's deadline is already in the past, POSIX semantics
    // are that it fires immediately (pending becomes 1 on the next read).
    // Previously we forced a 1ms artificial delay AND reset pending=0, which
    // meant every WakeUp() call from Flutter's message loop (all of which have
    // orig_exp=true because the task is always already due) would reset the
    // timer to "not ready" and prevent pid=2 from ever reading tfd=65 post-vsync.
    //
    // Fix: for one-shot (period=0) already-expired timers, disarm the deadline
    // and set pending=1 so the very next epoll_collect_ready sees it as ready.
    // For periodic timers, keep the 1ms delay to avoid CPU starvation.
    if original_expired && period_ns != 0 {
        // Periodic timer already past first fire: force 1ms delay to avoid hotloop.
        deadline = now.saturating_add(1_000_000);
    }
    // For one-shot already-expired (original_expired && period_ns==0): deadline
    // stays at the original (past) value; the and_modify below will set pending=1.
    let mut tbl = TIMERFD_TABLE.lock();
    {
        let n = TFD_SETTIME_LOG.fetch_add(1, Ordering::Relaxed);
        let pid = crate::process::current_pid();
        if n < 32 {
            if original_expired {
                log::warn!("[tfd-settime] #{} pid={} fd={} flags={} val={}ns period={}ns → fire-now (now={}ns)",
                    n, pid, fd, flags, value_ns, period_ns, now);
            } else {
                log::warn!("[tfd-settime] #{} pid={} fd={} flags={} deadline={}ns now={}ns delta={}ns period={}ns",
                    n, pid, fd, flags, deadline, now, deadline.saturating_sub(now), period_ns);
            }
        }
        // Always-on logging for the UI thread timerfd (65) and raster thread timerfd (67).
        // These are rate-limited separately to 300 entries each so post-vsync arming is visible.
        if fd == 65 {
            static TFD65_LOG: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
            let k = TFD65_LOG.fetch_add(1, Ordering::Relaxed);
            if k < 300 {
                log::warn!("[tfd65-arm] #{} pid={} delta={}ns period={}ns orig_exp={}",
                    k, pid, deadline.saturating_sub(now), period_ns, original_expired);
            }
        } else if fd == 67 {
            static TFD67_LOG: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
            let k = TFD67_LOG.fetch_add(1, Ordering::Relaxed);
            if k < 300 {
                log::warn!("[tfd67-arm] #{} pid={} delta={}ns period={}ns orig_exp={}",
                    k, pid, deadline.saturating_sub(now), period_ns, original_expired);
            }
        }
    }
    // For one-shot already-expired: set pending=1 (fire immediately), deadline=0
    // (disarmed; no future deadline needed since it already triggered).
    let (final_deadline, init_pending) = if original_expired && period_ns == 0 {
        (0u64, 1u64)
    } else {
        (deadline, 0u64)
    };
    tbl.entry(fd as u32)
        .and_modify(|t| {
            t.deadline_ns = final_deadline;
            t.period_ns = period_ns;
            if original_expired && period_ns == 0 {
                // Preserve any existing pending count, but ensure at least 1.
                t.pending = t.pending.saturating_add(1).max(1);
            } else {
                t.pending = 0; // normal re-arm resets pending
            }
        })
        .or_insert(TimerState {
            deadline_ns: final_deadline,
            period_ns,
            pending: init_pending,
        });
    drop(tbl);
    // Wake any thread blocked in epoll_wait on an epoll that watches this timerfd,
    // so it retries and picks up the newly armed event.
    epoll_wake(fd as u32);
    0
}

pub(crate) fn sys_epoll_create_real() -> i64 {
    let fd = alloc_synth_fd();
    let pid = crate::process::current_pid();
    log::warn!("[epoll_create] pid={} → fd={}", pid, fd);
    EPOLL_TABLE.lock().insert(fd as u32, Vec::new());
    fd
}

/// epoll_ctl(epfd, op, fd, event*). op: 1=ADD, 2=DEL, 3=MOD.
/// event layout (packed): u32 events; u64 data.
pub(crate) fn sys_epoll_ctl_real(epfd: u64, op: u64, fd: u64, event_ptr: u64) -> i64 {
    {
        static ECT_LOG: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
        let n = ECT_LOG.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
        if n < 32 {
            let events_val = if event_ptr != 0 {
                unsafe { core::ptr::read_unaligned(event_ptr as *const u32) }
            } else { 0 };
            log::warn!("[epoll_ctl] pid={} epfd={} op={} fd={} events={:#x}",
                crate::process::current_pid(), epfd, op, fd as i64, events_val);
            if event_ptr != 0 {
                let mut bytes = [0u8; 16];
                for i in 0..16 {
                    bytes[i] = unsafe { core::ptr::read_unaligned((event_ptr + i as u64) as *const u8) };
                }
                log::warn!("[epoll_ctl_raw] raw_bytes={:?}", bytes);
            }
        }
    }
    let mut tbl = EPOLL_TABLE.lock();
    let list = tbl.entry(epfd as u32).or_insert_with(Vec::new);
    let fd32 = fd as u32;
    match op {
        2 => { // DEL
            list.retain(|e| e.fd != fd32);
            0
        }
        1 | 3 => { // ADD or MOD
            if event_ptr == 0 { return -22; }
            let events = unsafe { core::ptr::read_unaligned(event_ptr as *const u32) };
            let data = unsafe { core::ptr::read_unaligned((event_ptr + 4) as *const u64) };
            if let Some(e) = list.iter_mut().find(|e| e.fd == fd32) {
                e.events = events;
                e.data = data;
            } else {
                list.push(EpollEntry { fd: fd32, events, data });
            }
            0
        }
        _ => -22,
    }
}

/// Returns ready events. event slot layout (packed): u32 events; u64 data.
fn epoll_collect_ready(epfd: u32, events_out: u64, maxevents: i32) -> i32 {
    if events_out == 0 || maxevents <= 0 { return 0; }
    let now = monotonic_ns();
    let mut count: i32 = 0;
    let tbl = EPOLL_TABLE.lock();
    let Some(list) = tbl.get(&epfd) else { return 0; };
    let mut tfd_tbl = TIMERFD_TABLE.lock();
    for entry in list.iter() {
        if count >= maxevents { break; }
        if epfd == 70 && entry.fd == 71 {
            static EPOLL_70_SCAN_LOG: core::sync::atomic::AtomicU32 =
                core::sync::atomic::AtomicU32::new(0);
            let n = EPOLL_70_SCAN_LOG.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
            if n < 128 {
                if let Some(t) = tfd_tbl.get(&entry.fd) {
                    log::warn!(
                        "[epoll70-scan] #{} now={} deadline={} pending={} period={}",
                        n,
                        now,
                        t.deadline_ns,
                        t.pending,
                        t.period_ns
                    );
                } else {
                    log::warn!("[epoll70-scan] #{} fd71 not in TIMERFD_TABLE", n);
                }
            }
        }
        if let Some(t) = tfd_tbl.get_mut(&entry.fd) {
            // Timer fires if: (a) has pending expirations from settime/previous fires,
            // or (b) deadline has arrived.
            let deadline_fired = t.deadline_ns != 0 && now >= t.deadline_ns;
            if deadline_fired {
                // Accumulate expiry into pending count.
                if t.period_ns != 0 {
                    let elapsed = now.saturating_sub(t.deadline_ns);
                    let n = 1u64 + elapsed / t.period_ns;
                    t.deadline_ns = t.deadline_ns.saturating_add(n * t.period_ns);
                    t.pending = t.pending.saturating_add(n);
                } else {
                    t.deadline_ns = 0; // disarm one-shot
                    t.pending = t.pending.saturating_add(1);
                }
            }
            if t.pending > 0 {
                static EPOLL_TFD_READY_LOG: core::sync::atomic::AtomicU32 =
                    core::sync::atomic::AtomicU32::new(0);
                let n = EPOLL_TFD_READY_LOG.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
                if n < 32 {
                    log::warn!(
                        "[epoll-ready-tfd] #{} epfd={} tfd={} pending={} deadline={} period={}",
                        n,
                        epfd,
                        entry.fd,
                        t.pending,
                        t.deadline_ns,
                        t.period_ns
                    );
                }
                // Report EPOLLIN; leave pending for the read() call to drain.
                let slot = events_out + (count as u64) * 12;
                unsafe {
                    core::ptr::write_unaligned(slot as *mut u32, 0x1 /* EPOLLIN */);
                    core::ptr::write_unaligned((slot + 4) as *mut u64, entry.data);
                }
                count += 1;
            }
        } else {
            // Check eventfd readability first (counter > 0 → EPOLLIN).
            let eventfd_count = EVENTFD_TABLE.lock().get(&entry.fd).copied().unwrap_or(0);
            if eventfd_count > 0 {
                static EPOLL_EFD_READY_LOG: core::sync::atomic::AtomicU32 =
                    core::sync::atomic::AtomicU32::new(0);
                let n = EPOLL_EFD_READY_LOG.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
                if n < 64 {
                    log::warn!(
                        "[epoll-ready-efd] #{} pid={} epfd={} efd={} count={}",
                        n,
                        crate::process::current_pid(),
                        epfd,
                        entry.fd,
                        eventfd_count
                    );
                }
                let slot = events_out + (count as u64) * 12;
                unsafe {
                    core::ptr::write_unaligned(slot as *mut u32, 0x1 /* EPOLLIN */);
                    core::ptr::write_unaligned((slot + 4) as *mut u64, entry.data);
                }
                count += 1;
                continue;
            }
            // Check pipe readability for non-timerfd fds.
            if pipe_readable_for_fd(entry.fd) {
                let slot = events_out + (count as u64) * 12;
                unsafe {
                    core::ptr::write_unaligned(slot as *mut u32, 0x1 /* EPOLLIN */);
                    core::ptr::write_unaligned((slot + 4) as *mut u64, entry.data);
                }
                count += 1;
            }
        }
    }
    count
}

/// epoll_wait(epfd, events, maxevents, timeout_ms).
pub(crate) fn sys_epoll_wait_real(epfd: u64, events_out: u64, maxevents: u64, timeout_ms: u64) -> i64 {
    let max = maxevents as i32;
    let timeout_signed = timeout_ms as i64;
    // timeout=-1 as a 32-bit int becomes 0xFFFFFFFF as u64 in our ABI.
    let infinite = timeout_signed == -1 || timeout_ms == 0xFFFF_FFFF;
    let cur = crate::process::current_pid();
    {
        static EW_LOG: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
        let n = EW_LOG.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
        if n < 8 {
            log::warn!("[epoll_wait] #{} pid={} epfd={} max={} to={}", n, cur, epfd, maxevents, timeout_signed);
        }
    }
    if epfd == 70 {
        static EPOLL_70_ENTER_LOG: core::sync::atomic::AtomicU32 =
            core::sync::atomic::AtomicU32::new(0);
        let n = EPOLL_70_ENTER_LOG.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
        if n < 128 {
            log::warn!(
                "[epoll70-enter] #{} pid={} to={} rip={:#x}",
                n,
                cur,
                timeout_signed,
                crate::arch::syscall::user_rip()
            );
        }
    }
    if epfd == 72 || cur == 8 {
        static EPOLL_72_ENTER_LOG: core::sync::atomic::AtomicU32 =
            core::sync::atomic::AtomicU32::new(0);
        let n = EPOLL_72_ENTER_LOG.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
        if n < 24 {
            let watched = {
                let tbl = EPOLL_TABLE.lock();
                tbl.get(&(epfd as u32))
                    .map(|list| {
                        list.iter()
                            .map(|e| (e.fd, e.events, e.data))
                            .collect::<alloc::vec::Vec<(u32, u32, u64)>>()
                    })
                    .unwrap_or_default()
            };
            log::warn!(
                "[epoll72-enter] #{} pid={} epfd={} max={} to={} watched={:?}",
                n,
                cur,
                epfd,
                maxevents,
                timeout_signed,
                watched
            );

            // Deadlock breaker (diagnostic): if the epfd72 loop never receives
            // pipe writes or timer arming, force one synthetic tick on fd73 so
            // we can observe the next stage instead of stalling forever.
            if epfd == 72 {
                static EPOLL_72_BOOTSTRAP: core::sync::atomic::AtomicBool =
                    core::sync::atomic::AtomicBool::new(false);
                if !EPOLL_72_BOOTSTRAP.swap(true, core::sync::atomic::Ordering::AcqRel) {
                    let mut tbl = TIMERFD_TABLE.lock();
                    if let Some(t) = tbl.get_mut(&73u32) {
                        t.pending = t.pending.saturating_add(1);
                        log::warn!("[epoll72-bootstrap] injected pending tick on tfd=73");
                    } else {
                        log::warn!("[epoll72-bootstrap] tfd=73 missing from TIMERFD_TABLE");
                    }
                    drop(tbl);
                    epoll_wake(73);
                }
            }
        }
    }

    // Fast path: any events already ready?
    let ready = epoll_collect_ready(epfd as u32, events_out, max);
    if ready > 0 {
        if epfd == 72 || cur == 8 {
            static EPOLL_72_READY_LOG: core::sync::atomic::AtomicU32 =
                core::sync::atomic::AtomicU32::new(0);
            let n = EPOLL_72_READY_LOG.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
            if n < 24 && events_out != 0 {
                let ev = unsafe { core::ptr::read_unaligned(events_out as *const u32) };
                let data = unsafe { core::ptr::read_unaligned((events_out + 4) as *const u64) };
                log::warn!(
                    "[epoll72-ready-fast] #{} pid={} epfd={} ready={} ev={:#x} data={:#x}",
                    n,
                    cur,
                    epfd,
                    ready,
                    ev,
                    data
                );
            }
        }
        static EW_FIRE_LOG: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
        let n = EW_FIRE_LOG.fetch_add(1, Ordering::Relaxed);
        if n < 32 {
            log::warn!("[epoll_fire] #{} pid={} epfd={} ready={}", n, cur, epfd, ready);
        }
        return ready as i64;
    }
    if epfd == 70 {
        static EPOLL_70_EMPTY_LOG: core::sync::atomic::AtomicU32 =
            core::sync::atomic::AtomicU32::new(0);
        let n = EPOLL_70_EMPTY_LOG.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
        if n < 128 {
            log::warn!("[epoll70-empty] #{} pid={} no-ready", n, cur);
        }
    }
    if timeout_signed == 0 { return 0; }

    let start = monotonic_ns();
    loop {
        let ready = epoll_collect_ready(epfd as u32, events_out, max);
        if ready > 0 {
            if epfd == 72 || cur == 8 {
                static EPOLL_72_READY_LOOP_LOG: core::sync::atomic::AtomicU32 =
                    core::sync::atomic::AtomicU32::new(0);
                let n = EPOLL_72_READY_LOOP_LOG.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
                if n < 24 && events_out != 0 {
                    let ev = unsafe { core::ptr::read_unaligned(events_out as *const u32) };
                    let data = unsafe { core::ptr::read_unaligned((events_out + 4) as *const u64) };
                    log::warn!(
                        "[epoll72-ready-loop] #{} pid={} epfd={} ready={} ev={:#x} data={:#x}",
                        n,
                        cur,
                        epfd,
                        ready,
                        ev,
                        data
                    );
                }
            }
            static EW_FIRE_LOOP_LOG: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
            let n = EW_FIRE_LOOP_LOG.fetch_add(1, Ordering::Relaxed);
            if n < 32 {
                log::warn!("[epoll_fire_loop] #{} pid={} epfd={} ready={}", n, cur, epfd, ready);
            }
            return ready as i64;
        }
        if !infinite {
            let elapsed_ms = (monotonic_ns().saturating_sub(start)) / 1_000_000;
            if elapsed_ms >= timeout_ms {
                return 0; // timeout expired
            }
        }

        if cur != 0 {
            if let Some(next) = crate::process::next_runnable_pid(cur) {
                if next != cur {
                    // Block ourselves on epoll
                    {
                        let mut blocked = EPOLL_BLOCKED.lock();
                        blocked.insert(cur as u32, epfd as u32);
                    }
                    let urip = crate::arch::syscall::user_rip();
                    let ursp = crate::arch::syscall::user_rsp();
                    crate::process::save_return_context(cur, urip, ursp);
                    crate::process::save_full_user_gprs(cur);
                    crate::process::set_errno_to_deliver(cur, 4); // EINTR
                    crate::process::set_rax(cur, (-1i64) as u64);
                    crate::process::save_xstate(cur);
                    crate::process::set_state(cur, crate::process::ProcState::Blocked);
                    crate::process::enter_user_by_pid_noreturn(next);
                }
            }
        }

        #[cfg(target_arch = "x86_64")]
        unsafe {
            core::arch::asm!("sti; hlt; cli", options(nomem, nostack));
        }
    }
}

