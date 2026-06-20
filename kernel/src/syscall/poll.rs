use super::state::{CondWaitState, FutexKey, COND_WAIT_STATE, FUTEX_WAITERS, WM_WAITER_DEADLINE, WM_WAITER_PID};
use super::tables::pipe_readable_for_fd;
use super::trace::{POSTEXIT_TRACE_ACTIVE, POSTEXIT_TRACE_COUNT, POSTEXIT_TRACE_LIMIT};
use super::wait::futex_waiter_remove;
use alloc::collections::BTreeMap;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};

/// Deferred task-runner kick flag (set from APIC timer ISR).
pub static KICK_REQUESTED: AtomicBool = AtomicBool::new(false);

/// Running thread waiting to re-lock `mutex` after a cond wait step.
pub fn acqmutex_waiter_for(mutex: u64, exclude: u32) -> Option<u32> {
    let cws = COND_WAIT_STATE.lock();
    for (&pid, st) in cws.iter() {
        if pid == exclude {
            continue;
        }
        if let CondWaitState::AcquiringMutex { mutex: m, .. } = st {
            if *m == mutex && crate::process::get_user_context(pid).is_some() {
                return Some(pid);
            }
        }
    }
    None
}

/// Pick a thread that needs to finish a cond-wait state machine step.
// (Not used directly anymore, functionality inline in cooperative_yield_to under lock)

fn perform_cooperative_yield(cur: u32, sys_nr: u64, next: u32) -> ! {
    let urip = crate::arch::syscall::user_rip();
    let ursp = crate::arch::syscall::user_rsp();
    // RE-EXEC mode (rip rewound to the `syscall` instruction), NOT a plain return. We set
    // rax=sys_nr so the thread RE-EXECUTES this syscall on resume. The old
    // `save_return_context` (RETURN mode, rip PAST the syscall) made the thread RETURN with
    // rax=sys_nr instead — i.e. the syscall handed its own number back as the result. For
    // epoll_wait (sys_nr=0x47B) the engine read that 1147 as an event count, overran its
    // 16-slot array, and #GP'd on a garbage DescriptorInfo* (the x86 interact-freeze). The
    // sibling de-starve helper already uses `_reexec`; this one was the odd one out.
    crate::process::save_return_context_reexec(cur, urip, ursp);
    crate::process::save_full_user_gprs(cur);
    crate::process::set_rax(cur, sys_nr);
    crate::process::save_xstate(cur);
    crate::process::enter_user_by_pid_noreturn(next);
}

/// Yield from high-frequency syscall paths so sibling threads make progress.
/// Cooperative-only (no userspace timer preemption on x86_64 yet).
pub fn cooperative_yield_for_cond_resched(cur: u32, sys_nr: u64) {
    cooperative_yield_to(cur, sys_nr, None);
}

/// Like [`cooperative_yield_for_cond_resched`] but prefer a specific target pid.
pub fn cooperative_yield_to(cur: u32, sys_nr: u64, prefer: Option<u32>) {
    if cur == 0 {
        return;
    }

    static YIELD_TICK: AtomicU32 = AtomicU32::new(0);
    let tick = YIELD_TICK.fetch_add(1, Ordering::Relaxed);
    let my_cpu = crate::arch::smp::this_cpu().cpu_id;

    let next = {
        let _g = crate::process::PTABLE_LOCK.lock();
        let next_candidate = prefer
            .filter(|&p| {
                if p == cur { return false; }
                let slot = unsafe { &mut crate::process::PTABLE[crate::process::idx_of(p)] };
                if slot.pid == p && slot.state == crate::process::ProcState::Running {
                    if slot.current_cpu.is_none() || slot.current_cpu == Some(my_cpu) {
                        slot.current_cpu = Some(my_cpu);
                        return true;
                    }
                }
                false
            })
            .or_else(|| {
                // cond_resched_target_locked
                let cws = COND_WAIT_STATE.lock();
                for (&pid, st) in cws.iter() {
                    if pid == cur { continue; }
                    if let CondWaitState::AcquiringMutex { .. } = st {
                        let slot = unsafe { &mut crate::process::PTABLE[crate::process::idx_of(pid)] };
                        if slot.pid == pid {
                            if slot.state == crate::process::ProcState::Blocked {
                                slot.state = crate::process::ProcState::Running;
                                crate::arch::smp::broadcast_resched_ipi();
                            }
                            if slot.state == crate::process::ProcState::Running {
                                if slot.current_cpu.is_none() || slot.current_cpu == Some(my_cpu) {
                                    slot.current_cpu = Some(my_cpu);
                                    return Some(pid);
                                }
                            }
                        }
                    }
                }
                None
            })
            .or_else(|| {
                if (tick & 31) == 0 {
                    crate::process::next_runnable_sibling_thread_locked(cur, my_cpu).map(|sib| {
                        let slot = unsafe { &mut crate::process::PTABLE[crate::process::idx_of(sib)] };
                        slot.current_cpu = Some(my_cpu);
                        sib
                    })
                } else {
                    None
                }
            });
        next_candidate
    };

    let Some(next) = next else {
        return;
    };

    perform_cooperative_yield(cur, sys_nr, next);
}

/// Pick the next thread for cooperative scheduling.
///
/// Prefer the embedder when it is blocked in `wm_event_wait`, then core Flutter
/// runner threads (typically pid 2–7), avoiding Dart worker isolates (8+) that
/// starve init when given equal round-robin weight.
pub fn coop_target_ready_locked(pid: u32, my_cpu: u32) -> bool {
    if pid == 0 {
        return false;
    }
    let p = unsafe { &mut crate::process::PTABLE[crate::process::idx_of(pid)] };
    if p.pid != pid {
        return false;
    }
    if matches!(p.state, crate::process::ProcState::Blocked) {
        let wm = WM_WAITER_PID.load(Ordering::Acquire);
        let focus = crate::wm::focus_pid();
        let input_target = if focus != 0 { focus } else { 1 };
        let input_priority_active =
            crate::wm::flutter_init_ready() && !crate::wm::flutter_bootstrap_spin_active();
        let input_is_waiting = input_priority_active
            && input_target != 0
            && crate::wm::input_pending_for(input_target) > 0;
        let has_pending = crate::wm::pending_count_for(pid) > 0 || crate::wm::embedder_baton_due(pid);
        if (wm == pid && has_pending) || (input_is_waiting && pid == input_target) {
            p.state = crate::process::ProcState::Running;
            crate::arch::smp::broadcast_resched_ipi();
        } else if input_is_waiting && pid != input_target {
            return false;
        }
    }
    p.state == crate::process::ProcState::Running
}

pub fn coop_target_ready(pid: u32) -> bool {
    let my_cpu = crate::arch::smp::this_cpu().cpu_id;
    let _g = crate::process::PTABLE_LOCK.lock();
    coop_target_ready_locked(pid, my_cpu)
}

/// When a vsync baton is waiting, prefer the engine whose baton is due over
/// engine spins. Per-engine: try the foreground app first (so a launched app
/// gets its own frames), then the shell (pid 1). Previously hardcoded pid 1,
/// which ran the shell for the app's baton and froze the app.
pub fn prefer_embedder_if_baton_due(cur: u32) -> Option<u32> {
    let my_cpu = crate::arch::smp::this_cpu().cpu_id;
    let _g = crate::process::PTABLE_LOCK.lock();
    let focus = crate::wm::focus_pid();
    let candidates = [focus, 1];
    for &cand in candidates.iter() {
        if cand != 0
            && cand != cur
            && crate::wm::embedder_baton_due(cand)
            && coop_target_ready_locked(cand, my_cpu)
        {
            let p = unsafe { &mut crate::process::PTABLE[crate::process::idx_of(cand)] };
            if p.current_cpu.is_none() || p.current_cpu == Some(my_cpu) {
                p.current_cpu = Some(my_cpu);
                return Some(cand);
            }
        }
    }
    None
}

pub fn cooperative_sched_target_locked(cur: u32, my_cpu: u32) -> Option<u32> {
    static COOP_TICK: AtomicU32 = AtomicU32::new(0);
    let tick = COOP_TICK.fetch_add(1, Ordering::Relaxed);

    let focus = crate::wm::focus_pid();
    let input_target = if focus != 0 { focus } else { 1 };
    if crate::wm::flutter_init_ready()
        && !crate::wm::flutter_bootstrap_spin_active()
        && input_target != 0
        && input_target != cur
        && crate::wm::input_pending_for(input_target) > 0
        && coop_target_ready_locked(input_target, my_cpu)
    {
        let p = unsafe { &mut crate::process::PTABLE[crate::process::idx_of(input_target)] };
        if p.current_cpu.is_none() || p.current_cpu == Some(my_cpu) {
            p.current_cpu = Some(my_cpu);
            return Some(input_target);
        }
    }

    let wm = WM_WAITER_PID.load(Ordering::Acquire);
    // Embedder must drain WM events (especially vsync batons) promptly — run the
    // waiting engine if ITS baton is due (per-engine, was a global pid-1 check).
    if wm != 0 && wm != cur && crate::wm::embedder_baton_due(wm) {
        if coop_target_ready_locked(wm, my_cpu) {
            let p = unsafe { &mut crate::process::PTABLE[crate::process::idx_of(wm)] };
            if p.current_cpu.is_none() || p.current_cpu == Some(my_cpu) {
                p.current_cpu = Some(my_cpu);
                return Some(wm);
            }
        }
    }
    // When a vsync baton is waiting, always prefer the embedder — even over
    // Flutter runner threads that would otherwise monopolise the CPU.
    if wm != 0 && wm != cur && crate::wm::vsync_baton_pending() {
        if coop_target_ready_locked(wm, my_cpu) {
            let p = unsafe { &mut crate::process::PTABLE[crate::process::idx_of(wm)] };
            if p.current_cpu.is_none() || p.current_cpu == Some(my_cpu) {
                p.current_cpu = Some(my_cpu);
                return Some(wm);
            }
        }
    }
    // Do not preempt Flutter runner threads (2–7) for baton=0 vsync ticks only.
    if wm != 0 && wm != cur && !(2..=7).contains(&cur) && !crate::wm::vsync_baton_pending() {
        let pending = crate::wm::pending_count_for(wm);
        let wm_due = pending > 0 || (tick & 3) == 0;
        if wm_due {
            if coop_target_ready_locked(wm, my_cpu) {
                let p = unsafe { &mut crate::process::PTABLE[crate::process::idx_of(wm)] };
                if p.current_cpu.is_none() || p.current_cpu == Some(my_cpu) {
                    p.current_cpu = Some(my_cpu);
                    return Some(wm);
                }
            }
        }
    }

    // FAIRNESS FIX (scalable — removes hardcoded thread pids). Previously this
    // preferred a HARDCODED set [2,3,4,7,1] and only considered other siblings
    // when (tick & 31)==0 AND sib<=7. That permanently STARVED the Dart VM's
    // later helper threads (pids 8,9,10,11: JIT compiler, GC, thread pool): the
    // hot mutator (pid 2) would yield only ever back to 2/3/4/7/1, never to the
    // helper it was blocked waiting on → single-core livelock where a non-trivial
    // app's first frame never completes (a trivial ColoredBox squeaked through
    // because it never spawns those helpers). A fair round-robin over ALL
    // runnable threads lets every engine/VM thread make progress. Input/baton
    // responsiveness is still handled by the priority checks above and the
    // matching checks inside next_runnable_pid_locked.
    let _ = tick;
    crate::process::next_runnable_pid_locked(cur, my_cpu)
}

pub fn cooperative_sched_target(cur: u32) -> Option<u32> {
    let my_cpu = crate::arch::smp::this_cpu().cpu_id;
    let _g = crate::process::PTABLE_LOCK.lock();
    cooperative_sched_target_locked(cur, my_cpu)
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

// `struct epoll_event` ABI differs by arch. glibc marks it `__attribute__((packed))`
// (`__EPOLL_PACKED`) ONLY on x86_64 → 12 bytes with `data` at offset 4. On aarch64
// (and most other arches) the attribute is empty → the struct is naturally aligned:
// 16 bytes with the 8-byte `data` union at offset 8. The engine's
// EventHandlerImplementation::Poll reads events with a 16-byte stride + data@8 on
// aarch64; if the kernel writes the 12-byte/offset-4 layout the engine reads a
// garbage `data` pointer (e.g. 0x100000000) and faults dereferencing it.
#[cfg(target_arch = "aarch64")]
pub(crate) const EPOLL_EVENT_SIZE: u64 = 16;
#[cfg(target_arch = "aarch64")]
pub(crate) const EPOLL_DATA_OFF: u64 = 8;
#[cfg(not(target_arch = "aarch64"))]
pub(crate) const EPOLL_EVENT_SIZE: u64 = 12;
#[cfg(not(target_arch = "aarch64"))]
pub(crate) const EPOLL_DATA_OFF: u64 = 4;

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
        crate::process::wake_process(pid);
    }
}

pub(crate) fn epoll_wake_try(fd: u32) {
    let watching_epfds: Vec<u32> = {
        let epoll_tbl = match EPOLL_TABLE.try_lock() {
            Some(t) => t,
            None => return,
        };
        epoll_tbl.iter()
            .filter(|(_, entries)| entries.iter().any(|e| e.fd == fd))
            .map(|(&epfd, _)| epfd)
            .collect()
    };
    if watching_epfds.is_empty() { return; }
    let pids_to_wake: Vec<u32> = {
        let mut blocked = match EPOLL_BLOCKED.try_lock() {
            Some(b) => b,
            None => return,
        };
        let pids: Vec<u32> = blocked.iter()
            .filter(|(_, &epfd)| watching_epfds.contains(&epfd))
            .map(|(&pid, _)| pid)
            .collect();
        for &pid in &pids { blocked.remove(&pid); }
        pids
    };
    for pid in pids_to_wake {
        crate::process::wake_process(pid);
    }
}

/// Linux-correct cleanup on `close(fd)`: a closed epoll/timerfd/eventfd must be removed
/// from its backing table AND from every epoll watch-list, and any thread blocked in
/// epoll_wait on an affected epfd woken to re-evaluate. Without this the entry lingers
/// and `epoll_collect_ready` keeps reporting the dead fd with its now-FREED `data` cookie
/// — which dart:io's EventHandler dereferences (the #GP). See docs/foundation-redesign.md.
pub(crate) fn on_fd_closed(fd: u32) {
    // Drop `fd` as an epoll instance, and remove it from every other epfd's watch-list.
    let mut affected: Vec<u32> = Vec::new();
    {
        let mut tbl = EPOLL_TABLE.lock();
        tbl.remove(&fd);
        for (epfd, list) in tbl.iter_mut() {
            let before = list.len();
            list.retain(|e| e.fd != fd);
            if list.len() != before {
                affected.push(*epfd);
            }
        }
    }
    // Drop timerfd / eventfd backing state.
    TIMERFD_TABLE.lock().remove(&fd);
    EVENTFD_TABLE.lock().remove(&fd);
    // Wake any thread blocked in epoll_wait on an epfd whose watch-set changed, so it
    // re-evaluates instead of parking on a fd that no longer exists.
    if !affected.is_empty() {
        let to_wake: Vec<u32> = {
            let blocked = EPOLL_BLOCKED.lock();
            blocked
                .iter()
                .filter(|(_, &epfd)| affected.contains(&epfd))
                .map(|(&pid, _)| pid)
                .collect()
        };
        for pid in to_wake {
            crate::process::wake_process(pid);
        }
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
                // Mark ready WITHOUT accumulating. This is a spurious wake to break
                // an epoll_wait parking deadlock, NOT a real expiration. The old
                // `saturating_add(1)` inflated an actively-serviced timerfd's pending
                // every time force-wake fired (frequent during app bring-up), so the
                // next timerfd read returned a large BOGUS expiration count — a
                // prime suspect for corrupting dart:io's timeout processing (the
                // intermittent app-launch EventHandler #GP). Cap at 1: still reports
                // EPOLLIN to unpark the waiter, but never piles up fake expirations.
                if t.pending == 0 {
                    t.pending = 1;
                }
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

/// Complete one timed-out cond waiter stuck in AcquiringMutex with a free mutex.
fn finish_cond_timedout_return(pid: u32, mutex: u64) {
    if pid == 0 {
        return;
    }
    // 1. Try to acquire the locks first in a consistent order
    let mut cws = match COND_WAIT_STATE.try_lock() {
        Some(c) => c,
        None => return,
    };
    let mut g_ptable = match crate::process::PTABLE_LOCK.try_lock() {
        Some(g) => g,
        None => return,
    };

    // 2. Try to acquire the userspace mutex
    let atom = unsafe { &*(mutex as *const core::sync::atomic::AtomicU32) };
    if atom.compare_exchange(0, 1, Ordering::Acquire, Ordering::Acquire).is_err() {
        return;
    }

    // 3. Perform register modifications and state update
    let p = unsafe { &mut crate::process::PTABLE[crate::process::idx_of(pid)] };
    if p.pid == pid {
        let rip = p.regs.rip;
        let rsp = p.regs.rsp;
        let fs = crate::arch::cpu::get_fs_base();
        // `rip` points AT the syscall instruction (re-exec mode set by
        // save_return_context_reexec when the waiter parked). To make it RETURN
        // ETIMEDOUT instead of re-waiting, advance past the syscall instruction:
        // 2 bytes on x86 (`syscall`), 4 bytes on aarch64 (`svc #0`). The old
        // hardcoded `+2` was an x86-ism that, on aarch64, landed in the middle of
        // the 4-byte `svc` → a misaligned resume PC (svc+2) → EC=0x22 PC-alignment
        // fault (the deterministic ~280-frame crash on long aarch64 runs).
        p.regs.rip    = rip.wrapping_add(crate::process::SYSCALL_INSN_LEN);
        p.regs.rsp    = rsp;
        p.regs.rflags = 0x202;
        p.preempted_by_timer = false;
        p.fs_base = fs;
        p.regs.rax = 110; // ETIMEDOUT
        // aarch64: a syscall returns its result in x0, but build_image only maps
        // rax→x0 when aarch64_ret_in_x0 is set. The re-exec park cleared it; set it
        // so the waiter actually sees ETIMEDOUT (not a stale arg0) on resume.
        #[cfg(target_arch = "aarch64")]
        {
            p.aarch64_ret_in_x0 = true;
        }
    }

    drop(g_ptable);
    cws.remove(&pid);
    drop(cws);

    crate::process::wake_process(pid);

    static FINISH_LOG: AtomicU32 = AtomicU32::new(0);
    let n = FINISH_LOG.fetch_add(1, Ordering::Relaxed);
    if n < 16 || n % 64 == 0 {
        log::warn!("[cond-timedout-finish] #{} pid={} mutex={:#x}", n, pid, mutex);
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
}

pub fn check_timerfds_and_wake_try() {
    let now = monotonic_ns();
    let mut to_wake = Vec::new();
    {
        let mut tbl = match TIMERFD_TABLE.try_lock() {
            Some(t) => t,
            None => return,
        };
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
        epoll_wake_try(fd);
    }

    // Proactively expire any Waiting cond-waiter whose timeout has fired.
    // This is critical: if the scheduler never picks the blocked thread
    // (because it prefers higher-priority siblings), the thread can never
    // reach the in-syscall timeout check. The timer ISR (this function) MUST
    // drive the expiry to un-block the thread.
    {
        // Single lock acquisition: collect + transition atomically.
        // (pid, mutex VA, pre-computed cond key) — the cond key was captured when
        // the waiter parked, so this ISR never re-derives it (Step 4).
        let expired: alloc::vec::Vec<(u32, u64, FutexKey)> = {
            let mut cws = match COND_WAIT_STATE.try_lock() {
                Some(c) => c,
                None => return, // contention — retry next tick
            };
            let mut exps = alloc::vec::Vec::new();
            for (&pid, st) in cws.iter_mut() {
                if let CondWaitState::Waiting { timeout_ns, mutex, cond_key, .. } = *st {
                    if timeout_ns != 0 && now >= timeout_ns {
                        // Atomically transition to AcquiringMutex inside the lock.
                        *st = CondWaitState::AcquiringMutex { mutex, timed_out: true };
                        exps.push((pid, mutex, cond_key));
                    }
                }
            }
            exps
        };
        for (pid, mutex, cond_key) in expired {
            // ISR-safe: never block on FUTEX_WAITERS here. An interrupted syscall
            // (e.g. spawn_thread, a futex op) may hold it; a non-try lock would
            // self-deadlock this single core with IRQs masked. On contention the
            // removal is simply retried on the next timer tick.
            //
            // Use the PRE-COMPUTED cond_key captured when this waiter parked
            // (foundation blueprint Step 4): the parked thread (`pid`) is NOT the
            // process current on this CPU, so the key cannot be re-derived here, and
            // deriving it would touch PTABLE_LOCK / the page tables under the IRQ
            // mask — the per-op translate that double-faulted in Attempt-1.
            super::futex_waiter_remove_try(cond_key, pid);
            static COND_EXPIRE_LOG: AtomicU32 = AtomicU32::new(0);
            let n = COND_EXPIRE_LOG.fetch_add(1, Ordering::Relaxed);
            if n < 32 {
                log::warn!("[cond-expire] #{} timer-ISR expired pid={} mutex={:#x}", n, pid, mutex);
            }
            crate::process::wake_process(pid);
        }
    }

    // Finish at most one timed-out AcquiringMutex waiter per tick (mutex free).
    {
        let stuck = {
            let cws = match COND_WAIT_STATE.try_lock() {
                Some(c) => c,
                None => return,
            };
            cws.iter()
                .find_map(|(&pid, st)| {
                    if let CondWaitState::AcquiringMutex {
                        mutex,
                        timed_out: true,
                    } = *st
                    {
                        let atom = unsafe {
                            &*(mutex as *const core::sync::atomic::AtomicU32)
                        };
                        if atom.load(Ordering::Acquire) == 0 {
                            Some((pid, mutex))
                        } else {
                            None
                        }
                    } else {
                        None
                    }
                })
        };
        if let Some((pid, mutex)) = stuck {
            finish_cond_timedout_return(pid, mutex);
        }
    }

    // Nudge blocked WM waiters when their deadline has passed.  Do not clear
    // WM_WAITER_DEADLINE here — the embedder's wm_event_wait loop uses it to
    // return EAGAIN; zeroing it early caused spurious eagain-top returns.
    let wm_deadline = WM_WAITER_DEADLINE.load(Ordering::Acquire);
    if wm_deadline != 0 && now >= wm_deadline {
        let wm_waiter = WM_WAITER_PID.load(Ordering::Acquire);
        if wm_waiter != 0 && crate::process::is_blocked_try(wm_waiter) {
            crate::process::wake_process(wm_waiter);
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
    let tsc_ns = crate::arch::rdtsc_ns();
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
    if n < 32 {
        log::warn!("[tfd-debug] fd={} raw: {:x?}", fd, raw_bytes);
        log::warn!("[tfd-debug] fd={} fields: int_s={} int_ns={} val_s={} val_ns={}", fd, int_s, int_ns, val_s, val_ns);
    }
    let mut period_ns = (int_s.max(0) as u64).saturating_mul(1_000_000_000)
        .saturating_add(int_ns.max(0) as u64);
    let value_ns = (val_s.max(0) as u64).saturating_mul(1_000_000_000)
        .saturating_add(val_ns.max(0) as u64);
    // Flutter MessageLoopLinux arms periodic absolute timers with
    // it_interval == it_value — keep period_ns intact (run-visible.log profile).
    let now = monotonic_ns();
    let mut deadline = if value_ns == 0 {
        0 // POSIX: if it_value is zero, the timer is disarmed.
    } else if flags & 1 != 0 {
        value_ns // TFD_TIMER_ABSTIME: caller already has absolute ns timestamp
    } else {
        now.saturating_add(value_ns)
    };
    let original_expired = deadline != 0 && deadline <= now;
    // Periodic already-expired: 1ms delay (matches working run-visible.log).
    // True one-shot (period=0): fire immediately via pending below.
    let fire_now_oneshot = original_expired && period_ns == 0;
    if original_expired && period_ns != 0 {
        deadline = now.saturating_add(1_000_000);
    }
    let mut tbl = TIMERFD_TABLE.lock();
    {
        let n = TFD_SETTIME_LOG.fetch_add(1, Ordering::Relaxed);
        let pid = crate::process::current_pid();
        if n < 32 {
            if fire_now_oneshot {
                log::warn!("[tfd-settime] #{} pid={} fd={} flags={} val={}ns period={}ns → fire-now (now={}ns)",
                    n, pid, fd, flags, value_ns, period_ns, now);
            } else if original_expired {
                log::warn!("[tfd-settime] #{} pid={} fd={} flags={} val={}ns period={}ns → forced-delay-1ms (now={}ns)",
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
    let (final_deadline, init_pending) = if fire_now_oneshot {
        (0u64, 1u64)
    } else {
        (deadline, 0u64)
    };
    tbl.entry(fd as u32)
        .and_modify(|t| {
            t.deadline_ns = final_deadline;
            t.period_ns = period_ns;
            if fire_now_oneshot {
                t.pending = t.pending.saturating_add(1).max(1);
            } else {
                t.pending = 0;
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
            let data = unsafe { core::ptr::read_unaligned((event_ptr + EPOLL_DATA_OFF) as *const u64) };
            // BISECTION PROBE: a valid epoll `data` cookie (a user DescriptorInfo* or small
            // int) is < 0x0000_8000_0000_0000. If the engine REGISTERS a non-canonical
            // cookie, its memory was already corrupt upstream (before the kernel).
            if data >= 0x0000_8000_0000_0000 {
                static EPCTL_GARBAGE: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
                let k = EPCTL_GARBAGE.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
                if k < 32 {
                    log::error!("[epoll-ctl-GARBAGE] #{} non-canonical cookie data={:#x} fd={} pid={}",
                        k, data, fd, crate::process::current_pid());
                }
            }
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
    static ECR_LOG: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
    let n = ECR_LOG.fetch_add(1, Ordering::Relaxed);
    if n < 48 {
        log::warn!("[ec-ready-enter] #{} epfd={} ev_out={:#x} max={}", n, epfd, events_out, maxevents);
    }
    if events_out == 0 || maxevents <= 0 {
        if n < 48 {
            log::warn!("[ec-ready-exit-early] #{} epfd={}", n, epfd);
        }
        return 0;
    }
    let now = monotonic_ns();
    let mut count: i32 = 0;
    let entries = {
        let tbl = EPOLL_TABLE.lock();
        let Some(list) = tbl.get(&epfd) else {
            if n < 48 {
                log::warn!("[ec-ready-exit-no-epfd] #{} epfd={}", n, epfd);
            }
            return 0;
        };
        list.clone()
    };
    let mut tfd_tbl = TIMERFD_TABLE.lock();
    for entry in entries.iter() {
        if count >= maxevents { break; }
        // BISECTION PROBE: if the cookie was canonical at registration (no ctl-GARBAGE
        // above) but is non-canonical HERE, the kernel's EPOLL_TABLE storage was corrupted
        // between epoll_ctl and epoll_wait. If neither probe fires yet the engine still
        // #GPs on r12, the corruption is engine-side after the kernel returns.
        if entry.data >= 0x0000_8000_0000_0000 {
            static EPRET_GARBAGE: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
            let k = EPRET_GARBAGE.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
            if k < 32 {
                log::error!("[epoll-ret-GARBAGE] #{} non-canonical cookie data={:#x} fd={} epfd={}",
                    k, entry.data, entry.fd, epfd);
            }
        }
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
                let slot = events_out + (count as u64) * EPOLL_EVENT_SIZE;
                unsafe {
                    core::ptr::write_unaligned(slot as *mut u32, 0x1 /* EPOLLIN */);
                    core::ptr::write_unaligned((slot + EPOLL_DATA_OFF) as *mut u64, entry.data);
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
                let slot = events_out + (count as u64) * EPOLL_EVENT_SIZE;
                unsafe {
                    core::ptr::write_unaligned(slot as *mut u32, 0x1 /* EPOLLIN */);
                    core::ptr::write_unaligned((slot + EPOLL_DATA_OFF) as *mut u64, entry.data);
                }
                count += 1;
                continue;
            }
            // Check pipe readability for non-timerfd fds.
            if pipe_readable_for_fd(entry.fd) {
                let slot = events_out + (count as u64) * EPOLL_EVENT_SIZE;
                unsafe {
                    core::ptr::write_unaligned(slot as *mut u32, 0x1 /* EPOLLIN */);
                    core::ptr::write_unaligned((slot + EPOLL_DATA_OFF) as *mut u64, entry.data);
                }
                count += 1;
            }
        }
    }
    // OSCORTEX freeze-hunt: re-read every cookie we just wrote and verify it is canonical
    // AT THE MOMENT WE RETURN. This is the decisive localizer for the x86 interact-freeze
    // (#GP at eventhandler_linux.cc:361 dereferencing a non-canonical di). If a cookie is
    // garbage HERE, the corruption is in-kernel (write/buffer). If every cookie is canonical
    // here yet the engine still #GPs, the corruption happens POST-RETURN (engine-side or a
    // thread-stack VA collision overwriting the events_out buffer between return and read).
    if events_out != 0 && count > 0 {
        for i in 0..(count as u64) {
            let slot = events_out + i * EPOLL_EVENT_SIZE;
            let cookie =
                unsafe { core::ptr::read_unaligned((slot + EPOLL_DATA_OFF) as *const u64) };
            if cookie != 0 && cookie >= 0x0000_8000_0000_0000 {
                static EC_BADCOOKIE_LOG: core::sync::atomic::AtomicU32 =
                    core::sync::atomic::AtomicU32::new(0);
                let bn = EC_BADCOOKIE_LOG.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
                if bn < 32 {
                    log::error!(
                        "[ec-BADCOOKIE] epfd={} i={}/{} cookie={:#x} events_out={:#x}",
                        epfd, i, count, cookie, events_out
                    );
                }
            }
        }
    }
    if n < 48 {
        log::warn!("[ec-ready-exit] #{} epfd={} count={}", n, epfd, count);
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
        // ERROR-level dump (greppable) emitted each time the kernel resume guard catches
        // another epoll_wait-nr leak. Dumped from here (the syscall path), never from the
        // boot-fragile resume path. fired>0 == the guard is actively neutralizing the rare
        // re-exec race that would otherwise #GP the engine. Change-triggered (swap) so the
        // count is visible even on short runs that do far fewer than 65536 epoll_waits.
        #[cfg(not(target_arch = "aarch64"))]
        {
            static LAST_FIRED_DUMP: core::sync::atomic::AtomicU32 =
                core::sync::atomic::AtomicU32::new(0);
            let fired = crate::process::EPOLL_NR_GUARD_FIRED
                .load(core::sync::atomic::Ordering::Relaxed);
            let prev = LAST_FIRED_DUMP.swap(fired, core::sync::atomic::Ordering::Relaxed);
            // Rate-limited (first 8, then every 64th) so a release isn't spammed while still
            // proving the guard is live: each line is a prevented #GP. Regression signal —
            // fired==0 together with a 0x140d84cdb #GP would mean the guard stopped matching.
            if fired != prev && (fired <= 8 || fired % 64 == 0) {
                log::error!("[epoll-guard] fired={}", fired);
            }
        }
    }

    // ── Input de-starve (single-core fairness) ───────────────────────────────
    // The Flutter engine task-runner threads busy-loop on epoll_wait. On a
    // single CPU they monopolize the core and never yield to the embedder host
    // (pid 1 / focus), which is the ONLY consumer of WM pointer/key events — so
    // queued clicks pile up and never reach Flutter. On EVERY epoll_wait entry
    // by an engine thread, if the focus consumer has queued input and is
    // runnable, hand the core straight to it. Gated on flutter_init_ready so it
    // never fires during engine init (would deadlock RunInitialized). pid 1
    // re-blocks in wm_event_wait once it has drained, then this engine thread's
    // epoll_wait is re-executed on resume.
    let bootstrap_spin_active = crate::wm::flutter_bootstrap_spin_active();
    if cur != 0 && !bootstrap_spin_active {
        let focus = crate::wm::focus_pid();
        let target = if focus != 0 { focus } else { 1 };
        if target != cur && crate::wm::input_pending_for(target) > 0 {
            static DESTARVE_LOG: core::sync::atomic::AtomicU32 =
                core::sync::atomic::AtomicU32::new(0);
            let ready_focus = coop_target_ready(target);
            let d = DESTARVE_LOG.fetch_add(1, Ordering::Relaxed);
            if d < 60 {
                log::warn!(
                    "[destarve-epoll] #{} cur={} focus={} target={} pend={} coop_ready={}",
                    d, cur, focus, target, crate::wm::input_pending_for(target), ready_focus
                );
            }
            if ready_focus {
                let my_cpu = crate::arch::smp::this_cpu().cpu_id;
                if crate::process::try_claim_cpu_for(target, my_cpu) {
                    let urip = crate::arch::syscall::user_rip();
                    let ursp = crate::arch::syscall::user_rsp();
                    crate::process::save_return_context_reexec(cur, urip, ursp);
                    crate::process::save_full_user_gprs(cur);
                    crate::process::set_rax(cur, 0x47B); // re-execute epoll_wait on resume
                    crate::process::save_xstate(cur);
                    crate::process::enter_user_by_pid_noreturn(target);
                }
            }
        }
    }
    if epfd == 70 {
        static EPOLL_70_ENTER_LOG: core::sync::atomic::AtomicU32 =
            core::sync::atomic::AtomicU32::new(0);
        let n = EPOLL_70_ENTER_LOG.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
        if n < 4 {
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
        if n < 4 {
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
            if n < 4 && events_out != 0 {
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
        if n < 4 {
            log::warn!("[epoll_fire] #{} pid={} epfd={} ready={}", n, cur, epfd, ready);
        }
        // Fairness de-starve: on a single core the engine threads busy-loop on
        // epoll_wait (their timerfd is perpetually ready), so the fast path
        // returns immediately every iteration and NEVER yields. That starves the
        // WM input consumer (focus pid / embedder host pid 1) which only drains
        // pointer/key events when scheduled — so clicks never reach Flutter.
        // ONLY yield when the focus consumer is genuinely runnable right now
        // (woken by input arrival via push_event_for) — never when it is blocked
        // on init/cond, otherwise the engine threads would ping-pong forever and
        // stall boot. Switch straight to focus so it drains input promptly; the
        // engine re-executes epoll_wait on resume (events still ready).
        let focus = crate::wm::focus_pid();
        {
            static YIELD_GATE_LOG: core::sync::atomic::AtomicU32 =
                core::sync::atomic::AtomicU32::new(0);
            let pend = crate::wm::input_pending_for(focus);
            if pend > 0 && focus != cur {
                let g = YIELD_GATE_LOG.fetch_add(1, Ordering::Relaxed);
                if g < 8 {
                    log::warn!(
                        "[yield-gate] #{} cur={} focus={} init_ready={} pend={} coop_ready={}",
                        g, cur, focus,
                        crate::wm::flutter_init_ready(),
                        pend,
                        coop_target_ready(focus)
                    );
                }
            }
        }
        if cur != 0
            && focus != 0
            && focus != cur
            && !bootstrap_spin_active
            && crate::wm::input_pending_for(focus) > 0
            && coop_target_ready(focus)
        {
            let my_cpu = crate::arch::smp::this_cpu().cpu_id;
            if crate::process::try_claim_cpu_for(focus, my_cpu) {
                let urip = crate::arch::syscall::user_rip();
                let ursp = crate::arch::syscall::user_rsp();
                crate::process::save_return_context_reexec(cur, urip, ursp);
                crate::process::save_full_user_gprs(cur);
                crate::process::set_rax(cur, 0x47B); // SYS epoll_wait (re-enter)
                crate::process::save_xstate(cur);
                crate::process::enter_user_by_pid_noreturn(focus);
            }
        }
        return ready as i64;
    }
    if epfd == 70 {
        static EPOLL_70_EMPTY_LOG: core::sync::atomic::AtomicU32 =
            core::sync::atomic::AtomicU32::new(0);
        let n = EPOLL_70_EMPTY_LOG.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
        if n < 4 {
            log::warn!("[epoll70-empty] #{} pid={} no-ready", n, cur);
        }
    }
    if timeout_signed == 0 { return 0; }

    let start = monotonic_ns();
    loop {
        // Block ourselves on epoll BEFORE checking readiness to prevent lost wakeups
        {
            let mut blocked = EPOLL_BLOCKED.lock();
            blocked.insert(cur as u32, epfd as u32);
        }
        crate::process::set_state(cur, crate::process::ProcState::Blocked);

        let ready = epoll_collect_ready(epfd as u32, events_out, max);
        if ready > 0 {
            {
                let mut blocked = EPOLL_BLOCKED.lock();
                blocked.remove(&(cur as u32));
            }
            crate::process::set_state(cur, crate::process::ProcState::Running);

            if epfd == 72 || cur == 8 {
                static EPOLL_72_READY_LOOP_LOG: core::sync::atomic::AtomicU32 =
                    core::sync::atomic::AtomicU32::new(0);
                let n = EPOLL_72_READY_LOOP_LOG.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
                if n < 4 && events_out != 0 {
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
            if n < 4 {
                log::warn!("[epoll_fire_loop] #{} pid={} epfd={} ready={}", n, cur, epfd, ready);
            }
            return ready as i64;
        }
        if !infinite {
            let elapsed_ms = (monotonic_ns().saturating_sub(start)) / 1_000_000;
            if elapsed_ms >= timeout_ms {
                {
                    let mut blocked = EPOLL_BLOCKED.lock();
                    blocked.remove(&(cur as u32));
                }
                crate::process::set_state(cur, crate::process::ProcState::Running);
                return 0; // timeout expired
            }
        }

        // Pre-init Flutter runner timers are often re-armed just a few hundred
        // microseconds into the future. On single-core boots, dropping into the
        // kernel hlt path here can strand the thread before the next scan. Give
        // the embedder thread group a short spin window so freshly armed timerfd
        // deadlines can mature and be observed without relying on IRQ wakeups.
        if cur >= 2
            && crate::process::get_group_leader(cur) == 1
            && crate::wm::flutter_bootstrap_spin_active()
        {
            let start_time = monotonic_ns();
            let spin_until = start_time.saturating_add(2_000_000);
            let mut iters = 0u64;
            loop {
                iters += 1;
                check_timerfds_and_wake();
                let ready = epoll_collect_ready(epfd as u32, events_out, max);
                if ready > 0 {
                    {
                        let mut blocked = EPOLL_BLOCKED.lock();
                        blocked.remove(&(cur as u32));
                    }
                    crate::process::set_state(cur, crate::process::ProcState::Running);

                    static EPOLL_SPIN_READY_LOG: core::sync::atomic::AtomicU32 =
                        core::sync::atomic::AtomicU32::new(0);
                    let n = EPOLL_SPIN_READY_LOG.fetch_add(1, Ordering::Relaxed);
                    if n < 32 {
                        log::warn!(
                            "[epoll-spin-ready] #{} pid={} epfd={} ready={} init_ready={}",
                            n,
                            cur,
                            epfd,
                            ready,
                            crate::wm::flutter_init_ready()
                        );
                    }
                    return ready as i64;
                }
                let now = core::hint::black_box(monotonic_ns());
                if now >= spin_until {
                    static SPIN_TIMEOUT_LOG: core::sync::atomic::AtomicU32 =
                        core::sync::atomic::AtomicU32::new(0);
                    let n = SPIN_TIMEOUT_LOG.fetch_add(1, Ordering::Relaxed);
                    if n < 8 {
                        log::warn!(
                            "[epoll-spin-timeout] #{} pid={} epfd={} iters={} start={} now={} limit={}",
                            n, cur, epfd, iters, start_time, now, spin_until
                        );
                    }
                    break;
                }
                core::hint::spin_loop();
            }
        }

        if cur != 0 {
            let coop_target = cooperative_sched_target(cur);
            if let Some(next) = coop_target.filter(|&n| n != cur) {
                if epfd == 70 || epfd == 72 || cur == 2 || cur == 3 || cur == 4 || cur == 7 {
                    static EPOLL_BLOCK_LOG: core::sync::atomic::AtomicU32 =
                        core::sync::atomic::AtomicU32::new(0);
                    let n = EPOLL_BLOCK_LOG.fetch_add(1, Ordering::Relaxed);
                    if n < 48 {
                        log::warn!(
                            "[epoll-block] #{} pid={} epfd={} next={} rip={:#x}",
                            n,
                            cur,
                            epfd,
                            next,
                            crate::arch::syscall::user_rip()
                        );
                    }
                }
                // Re-execute the epoll_wait syscall on resume (do NOT return
                // -1/EINTR to userspace). Returning EINTR here makes the
                // Flutter engine retry epoll_wait immediately; if it is woken
                // before any fd is actually ready it gets EINTR again and
                // busy-spins, monopolizing the single core and starving the
                // embedder host (pid 1) input loop. Saving the context at the
                // syscall instruction (urip - 2) with rax = the epoll_wait
                // syscall number makes the thread re-enter the kernel on wake
                // and re-check readiness — returning real events when ready or
                // blocking again — exactly like sys_wm_event_wait does.
                let urip = crate::arch::syscall::user_rip();
                let ursp = crate::arch::syscall::user_rsp();
                crate::process::save_return_context_reexec(cur, urip, ursp);
                crate::process::save_full_user_gprs(cur);
                crate::process::set_rax(cur, 0x47B); // SYS epoll_wait (re-enter)
                crate::process::save_xstate(cur);
                crate::process::enter_user_by_pid_noreturn(next);
            } else if cur >= 2
                && crate::process::get_group_leader(cur) == 1
                && !crate::wm::flutter_init_ready()
                && crate::process::current_pid() != 1
            {
                static EPOLL_INIT_HANDOFF_LOG: core::sync::atomic::AtomicU32 =
                    core::sync::atomic::AtomicU32::new(0);
                let n = EPOLL_INIT_HANDOFF_LOG.fetch_add(1, Ordering::Relaxed);
                if n < 32 {
                    log::warn!(
                        "[epoll-init-handoff] #{} pid={} epfd={} -> pid1 rip={:#x} target={:?}",
                        n,
                        cur,
                        epfd,
                        crate::arch::syscall::user_rip(),
                        coop_target
                    );
                }
                crate::process::set_state(1, crate::process::ProcState::Running);
                let urip = crate::arch::syscall::user_rip();
                let ursp = crate::arch::syscall::user_rsp();
                crate::process::save_return_context_reexec(cur, urip, ursp);
                crate::process::save_full_user_gprs(cur);
                crate::process::set_rax(cur, 0x47B);
                crate::process::save_xstate(cur);
                crate::process::enter_user_by_pid_noreturn(1);
            }
        }

        if cur != 0 {
            let coop_target = cooperative_sched_target(cur);
            if let Some(next) = coop_target.filter(|&n| n != cur) {
                if epfd == 70 || epfd == 72 || cur == 2 || cur == 3 || cur == 4 || cur == 7 {
                    static EPOLL_BLOCK_LOG: core::sync::atomic::AtomicU32 =
                        core::sync::atomic::AtomicU32::new(0);
                    let n = EPOLL_BLOCK_LOG.fetch_add(1, Ordering::Relaxed);
                    if n < 48 {
                        log::warn!(
                            "[epoll-block] #{} pid={} epfd={} next={} rip={:#x}",
                            n,
                            cur,
                            epfd,
                            next,
                            crate::arch::syscall::user_rip()
                        );
                    }
                }
                // Re-execute the epoll_wait syscall on resume (do NOT return
                // -1/EINTR to userspace). Returning EINTR here makes the
                // Flutter engine retry epoll_wait immediately; if it is woken
                // before any fd is actually ready it gets EINTR again and
                // busy-spins, monopolizing the single core and starving the
                // embedder host (pid 1) input loop. Saving the context at the
                // syscall instruction (urip - 2) with rax = the epoll_wait
                // syscall number makes the thread re-enter the kernel on wake
                // and re-check readiness — returning real events when ready or
                // blocking again — exactly like sys_wm_event_wait does.
                let urip = crate::arch::syscall::user_rip();
                let ursp = crate::arch::syscall::user_rsp();
                crate::process::save_return_context_reexec(cur, urip, ursp);
                crate::process::save_full_user_gprs(cur);
                crate::process::set_rax(cur, 0x47B); // SYS epoll_wait (re-enter)
                crate::process::save_xstate(cur);
                crate::process::enter_user_by_pid_noreturn(next);
            } else if cur >= 2
                && crate::process::get_group_leader(cur) == 1
                && !crate::wm::flutter_init_ready()
                && crate::process::current_pid() != 1
            {
                static EPOLL_INIT_HANDOFF_LOG: core::sync::atomic::AtomicU32 =
                    core::sync::atomic::AtomicU32::new(0);
                let n = EPOLL_INIT_HANDOFF_LOG.fetch_add(1, Ordering::Relaxed);
                if n < 32 {
                    log::warn!(
                        "[epoll-init-handoff] #{} pid={} epfd={} -> pid1 rip={:#x} target={:?}",
                        n,
                        cur,
                        epfd,
                        crate::arch::syscall::user_rip(),
                        coop_target
                    );
                }
                crate::process::set_state(1, crate::process::ProcState::Running);
                let urip = crate::arch::syscall::user_rip();
                let ursp = crate::arch::syscall::user_rsp();
                crate::process::save_return_context_reexec(cur, urip, ursp);
                crate::process::save_full_user_gprs(cur);
                crate::process::set_rax(cur, 0x47B);
                crate::process::save_xstate(cur);
                crate::process::enter_user_by_pid_noreturn(1);
            }
        }

        // Sleep with IRQs unmasked so the timer ISR fires + (aarch64) its
        // kernel-mode wake-assist can run. Was x86-only → aarch64 busy-spun in EL1.
        unsafe {
            { crate::arch::enable_and_halt(); }
        }
    }
}
