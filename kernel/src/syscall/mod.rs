//! System call dispatch.
//!
//! Syscall ABI (x86_64):
//!   RAX = syscall number
//!   RDI = arg0, RSI = arg1, RDX = arg2, R10 = arg3, R8 = arg4, R9 = arg5
//!   Return value in RAX.
//!
//! Syscall numbers (Linux-compatible where possible):
//!   0  read        3  close      39 getpid
//!   1  write       57 fork       59 execve
//!   2  open        60 exit       61 wait4
//!   62 kill      231 exit_group
//!   0x1000–0x100F  Cortex PID-0 API
//!   0x200          ipc_send     0x201 ipc_recv
//!   0x300          surface_create
//!   0x301          surface_move
//!   0x302          surface_destroy
//!   0x303          surface_upload_rgba32
//!   0x304          surface_present
//!   0x305          fb_size_packed
//!   0x306          vsync_counter
//!   0x307          vsync_wait_nonblock
//!   0x308          surface_owner_pid
//!   0x309          surface_z_get
//!   0x30A          surface_z_set
//!   0x30B          surface_geometry_get
//!   0x30C          surface_geometry_set
//!   0x30D          surface_visibility_get
//!   0x30E          surface_visibility_set
//!   0x30F          surface_clip_set
//!   0x310          surface_damage_set
//!   0x311          surface_damage_get
//!   0x312          surface_flip
//!   0x320          wm_event_poll
//!   0x321          wm_event_read
//!   0x322          wm_event_inject
//!   0x323          wm_event_wait
//!   0x330          embedder_abi_version
//!   0x331          wm_event_size
//!   0x332          wm_event_stats_packed
//!   0x333          wm_focus_pid_get
//!   0x334          wm_focus_surface_set
//!   0x335          wm_focus_mirror_get
//!   0x336          wm_focus_mirror_set
//!   0x340          app_notify
//!   0x341          proc_surface_count
//!   0x342          app_launch_path
//!   0x343          engine_policy_get
//!   0x344          engine_version_packed
//!   0x345          engine_host_register
//!   0x346          engine_host_pid_get
//!   0x347          engine_library_path_read
//!   0x350          dlopen(path_ptr, path_len, flags) → handle
//!   0x351          dlsym(handle, name_ptr, name_len) → vaddr
//!   0x352          dlclose(handle) → 0
//!   0x353          mmap(hint_va, size, prot) → va
//!   0x354          munmap(va, size) → 0  (stub)
//!   0x355          mprotect(va, size, prot) → 0  (stub)
//!   0x366          aot_snapshot_load(path_ptr, path_len, out_va_ptr, out_size_ptr) → 0
//!   0x367          isolate_spawn(aot_va, aot_size, entry_offset, stack_size) → id
//!   0x368          isolate_kill(id) → 0
//!   0x369          isolate_ctrl(id, op) → state
//!   0x36A          isolate_msg_send(dst_id, data_ptr, data_len) → 0
//!   0x36B          isolate_msg_recv(isolate_id, buf_ptr, buf_len) → bytes
//!   0x36C          isolate_msg_pending(isolate_id) → count
//!   0x36D          input_dev_count() → n
//!   0x36E          input_dev_info(n) → packed_u32
//!   0x36F          app_install(bundle_ptr, bundle_len, id_out_ptr) → 0
//!   0x370          app_list(buf_ptr, buf_len) → count
//!   0x371          app_launch(app_id, flags) → isolate_id
//!   0x372          app_uninstall(app_id) → 0
//!   0x373          port_bind(name_ptr, name_len, isolate_id) → 0
//!   0x374          port_lookup(name_ptr, name_len, iso_out, pid_out) → 0
//!   0x375          port_unbind(name_ptr, name_len) → 0
//!   0x376          usb_controller_count() → n
//!   0x377          fb_map(info_out_ptr) → 0 / -ENXIO
//!   0x378          wm_next_event(event_out_ptr) → 0 / -EAGAIN
//!   0x379          vfs_list(path_ptr, path_len, buf_ptr, buf_len) → bytes
//!   0x37A          vfs_read(path_ptr, path_len, buf_ptr, buf_len) → bytes
//!   0x37B          vfs_write(path_ptr, path_len, data_ptr, data_len) → 0
//!   0x37C          vfs_stat(path_ptr, path_len, size_out_ptr) → 0 / -ENOENT
//!   0x37D          net_info(buf_ptr, buf_len) → bytes
//!   0x37E          net_send(dst_ip, dst_port, data_ptr, data_len) → 0
//!   0x37F          net_recv(buf_ptr, buf_len, src_ip_out, src_port_out) → bytes
//!   0x380          fb_release() → 0
//!   0x381          surface_fullscreen() → surface_id
//!   0xC0           sys_poweroff (ACPI S5)

use crate::embedder::abi as eabi;
use alloc::collections::BTreeMap;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};

const ENGINE_LIBRARY_PATH: &str = "/system/lib/libflutter_engine.so";
static ENGINE_HOST_PID: AtomicU32 = AtomicU32::new(0);
/// VA of the `FlutterEngineProcTable` registered by the engine host.
static ENGINE_PROC_TABLE_PTR: AtomicU64 = AtomicU64::new(0);
static FS_BOOTSTRAP_LOGGED: AtomicU32 = AtomicU32::new(0);

const SYSCALL_TRACE_DEPTH: usize = 64;

#[derive(Clone, Copy)]
struct SyscallTraceEntry {
    pid: u32,
    nr: u64,
    a0: u64,
    a1: u64,
    a2: u64,
    rip: u64,
}

static SYSCALL_TRACE_BUF: spin::Mutex<[SyscallTraceEntry; SYSCALL_TRACE_DEPTH]> = spin::Mutex::new(
    [const { SyscallTraceEntry { pid: 0, nr: 0, a0: 0, a1: 0, a2: 0, rip: 0 } }; SYSCALL_TRACE_DEPTH]
);
static SYSCALL_TRACE_HEAD: AtomicU32 = AtomicU32::new(0);
static FUTEX_WAITERS: spin::Mutex<BTreeMap<u64, Vec<u32>>> = spin::Mutex::new(BTreeMap::new());
// Pending wakes for "wake-before-wait" edge cases. When FUTEX_WAKE fires on a
// target address with no parked waiters, record one pending wake; the next
// FUTEX_WAIT on that address consumes it and returns immediately. This is the
// fix for the busy-spin: cond broadcasts bridge to PID1_WAIT before pid 1 has
// actually parked, so without this, the wake is lost and pid 1 sleeps forever.
static FUTEX_PENDING_WAKES: spin::Mutex<BTreeMap<u64, u32>> = spin::Mutex::new(BTreeMap::new());

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CondWaitState {
    Waiting { cond: u64, mutex: u64, seq: u32, timeout_ns: u64 },
    AcquiringMutex { mutex: u64, timed_out: bool },
}

static COND_WAIT_STATE: spin::Mutex<BTreeMap<u32, CondWaitState>> = spin::Mutex::new(BTreeMap::new());

// ── Post-exit syscall trace window ────────────────────────────────────────────
// When pid=2 exits, activate a one-shot bounded trace that logs the next
// POSTEXIT_TRACE_LIMIT syscalls (any pid) so we can see what pid=1 does after
// it resumes from pthread_cond_wait.
const POSTEXIT_TRACE_LIMIT: u32 = 5000;
static POSTEXIT_TRACE_ACTIVE: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);
static POSTEXIT_TRACE_COUNT: AtomicU32 = AtomicU32::new(0);

// ── Synthetic FD allocator ────────────────────────────────────────────────────
// epoll_create / inotify_init1 / timerfd_create return integer fds that
// user space then stores in std::map<fd, T> keyed event loops. Returning the
// same constant for two different objects causes map::at() to fail
// ("key not found") when one overwrites the other. Hand out unique numbers
// from a private high range that won't collide with real sys_open fds.
static SYNTH_FD_NEXT: AtomicU32 = AtomicU32::new(64);
#[inline]
fn alloc_synth_fd() -> i64 {
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
struct TimerState {
    deadline_ns: u64, // absolute monotonic ns; 0 = disarmed
    period_ns: u64,
    pending: u64,     // expiry count not yet consumed by read()
}

#[derive(Clone, Copy)]
struct EpollEntry {
    fd: u32,
    events: u32,
    data: u64,
}

static TIMERFD_TABLE: spin::Mutex<BTreeMap<u32, TimerState>> = spin::Mutex::new(BTreeMap::new());
/// eventfd counters: synth fd → counter (u64). A write adds to the counter;
/// a read drains it atomically. epoll fires EPOLLIN when counter > 0.
static EVENTFD_TABLE: spin::Mutex<BTreeMap<u32, u64>> = spin::Mutex::new(BTreeMap::new());
static EPOLL_TABLE: spin::Mutex<BTreeMap<u32, Vec<EpollEntry>>> = spin::Mutex::new(BTreeMap::new());
/// Threads blocked in epoll_wait(timeout=-1): pid → epfd they're waiting on.
static EPOLL_BLOCKED: spin::Mutex<BTreeMap<u32, u32>> = spin::Mutex::new(BTreeMap::new());

/// Wake every thread that is blocked in epoll_wait and whose epoll watches `tfd`.
/// Called after a timerfd is armed. The blocked thread already has rax=1 and a
/// fake EPOLLIN event written to its events_out buffer; it will resume and call
/// TimerDrain, which will then successfully read the now-armed timerfd.
fn epoll_wake(fd: u32) {
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
    // Mark each armed timerfd as pending (one expiration), so the next
    // epoll_collect_ready call returns EPOLLIN immediately.
    let mut armed_tfds = Vec::new();
    {
        let mut tbl = TIMERFD_TABLE.lock();
        for fd in &tfds {
            if let Some(t) = tbl.get_mut(fd) {
                if t.deadline_ns != 0 {
                    t.pending = t.pending.saturating_add(1);
                    armed_tfds.push(*fd);
                }
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
    let touched: Vec<u32> = armed_tfds.iter().chain(efds.iter()).copied().collect();
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
            "[force-wake] #{} reason={} timerfds={} eventfds={} epoll_waiters_released={}",
            n, reason, tfds.len(), efds.len(), woken
        );
    }
    (tfds.len() + efds.len(), woken)
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

#[inline]
fn monotonic_ns() -> u64 {
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

fn sys_timerfd_create_real() -> i64 {
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
fn sys_eventfd2(initval: u64, _flags: u64) -> i64 {
    let fd = alloc_synth_fd();
    EVENTFD_TABLE.lock().insert(fd as u32, initval);
    log::warn!("[eventfd2] pid={} → fd={} initval={}", crate::process::current_pid(), fd, initval);
    fd
}

/// Write to an eventfd: add the u64 value to its counter and wake epoll waiters.
fn eventfd_write(fd: u32, val: u64) {
    let new_count = {
        let mut tbl = EVENTFD_TABLE.lock();
        if let Some(c) = tbl.get_mut(&fd) {
            *c = c.saturating_add(val.max(1));
            *c
        } else {
            return;
        }
    };
    if new_count > 0 {
        epoll_wake(fd);
    }
}

/// flags bit 0 = TFD_TIMER_ABSTIME. new_value points to itimerspec:
///   it_interval { tv_sec:i64, tv_nsec:i64 }
///   it_value    { tv_sec:i64, tv_nsec:i64 }
fn sys_timerfd_settime_real(fd: u64, flags: u64, new_value: u64, _old_value: u64) -> i64 {
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
    let period_ns = (int_s.max(0) as u64).saturating_mul(1_000_000_000)
        .saturating_add(int_ns.max(0) as u64);
    let value_ns = (val_s.max(0) as u64).saturating_mul(1_000_000_000)
        .saturating_add(val_ns.max(0) as u64);
    let now = monotonic_ns();
    let mut deadline = if value_ns == 0 {
        0 // POSIX: if it_value is zero, the timer is disarmed.
    } else if flags & 1 != 0 {
        value_ns // TFD_TIMER_ABSTIME: caller already has absolute ns timestamp
    } else {
        now.saturating_add(value_ns)
    };
    let original_expired = deadline != 0 && deadline <= now;
    if original_expired {
        // Force a minimum 1ms delay (1,000,000 ns) to prevent CPU starvation/hot loops
        deadline = now.saturating_add(1_000_000);
    }
    let mut tbl = TIMERFD_TABLE.lock();
    // Auto-register if user-space called settime on an fd we didn't see
    // through timerfd_create (shouldn't happen, but be lenient).
    let already_expired = false;
    {
        let n = TFD_SETTIME_LOG.fetch_add(1, Ordering::Relaxed);
        let pid = crate::process::current_pid();
        if n < 32 {
            if original_expired {
                log::warn!("[tfd-settime] #{} pid={} fd={} flags={} val={}ns period={}ns → forced-delay-1ms (now={}ns)",
                    n, pid, fd, flags, value_ns, period_ns, now);
            } else {
                log::warn!("[tfd-settime] #{} pid={} fd={} flags={} deadline={}ns now={}ns delta={}ns period={}ns",
                    n, pid, fd, flags, deadline, now, deadline.saturating_sub(now), period_ns);
            }
        }
    }
    tbl.entry(fd as u32)
        .and_modify(|t| {
            t.deadline_ns = deadline;
            t.period_ns = period_ns;
            t.pending = 0; // re-arm resets pending
        })
        .or_insert(TimerState {
            deadline_ns: deadline,
            period_ns,
            pending: 0,
        });
    drop(tbl);
    // Wake any thread blocked in epoll_wait on an epoll that watches this timerfd,
    // so it retries and picks up the newly armed event.
    epoll_wake(fd as u32);
    0
}

fn sys_epoll_create_real() -> i64 {
    let fd = alloc_synth_fd();
    let pid = crate::process::current_pid();
    log::warn!("[epoll_create] pid={} → fd={}", pid, fd);
    EPOLL_TABLE.lock().insert(fd as u32, Vec::new());
    fd
}

/// epoll_ctl(epfd, op, fd, event*). op: 1=ADD, 2=DEL, 3=MOD.
/// event layout (packed): u32 events; u64 data.
fn sys_epoll_ctl_real(epfd: u64, op: u64, fd: u64, event_ptr: u64) -> i64 {
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
            let eventfd_ready = EVENTFD_TABLE.lock().get(&entry.fd).map_or(false, |&c| c > 0);
            if eventfd_ready {
                let slot = events_out + (count as u64) * 12;
                unsafe {
                    core::ptr::write_unaligned(slot as *mut u32, 0x1 /* EPOLLIN */);
                    core::ptr::write_unaligned((slot + 4) as *mut u64, entry.data);
                }
                count += 1;
                continue;
            }
            // Check pipe readability for non-timerfd fds.
            let fd_idx = entry.fd as usize;
            let pipe_ready = if fd_idx < MAX_OPEN_FILES {
                let open = OPEN_FILES.lock();
                if open[fd_idx].used && open[fd_idx].pipe_id >= 0 && !open[fd_idx].pipe_is_write {
                    let pid = open[fd_idx].pipe_id as usize;
                    drop(open);
                    let pipes = PIPES.lock();
                    pipes[pid].len > 0
                } else {
                    false
                }
            } else { false };
            if pipe_ready {
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
fn sys_epoll_wait_real(epfd: u64, events_out: u64, maxevents: u64, timeout_ms: u64) -> i64 {
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

#[inline]
fn record_syscall_trace(nr: u64, a0: u64, a1: u64, a2: u64, rip: u64) {
    let pid = crate::process::current_pid();
    let idx = (SYSCALL_TRACE_HEAD.fetch_add(1, Ordering::Relaxed) as usize) % SYSCALL_TRACE_DEPTH;
    let mut buf = SYSCALL_TRACE_BUF.lock();
    buf[idx] = SyscallTraceEntry { pid, nr, a0, a1, a2, rip };
}

pub fn dump_recent_syscalls(limit: usize) {
    let depth = SYSCALL_TRACE_DEPTH;
    let count = limit.min(depth);
    let head = SYSCALL_TRACE_HEAD.load(Ordering::Relaxed) as usize;
    let buf = SYSCALL_TRACE_BUF.lock();

    log::error!("[syscall-trace] last {} syscall(s):", count);
    for i in 0..count {
        let idx = (head + depth - 1 - i) % depth;
        let e = buf[idx];
        if e.nr == 0 && e.rip == 0 {
            continue;
        }
        log::error!(
            "[syscall-trace] #{:02} pid={} nr={:#x} a0={:#x} a1={:#x} a2={:#x} rip={:#x}",
            i,
            e.pid,
            e.nr,
            e.a0,
            e.a1,
            e.a2,
            e.rip
        );
    }
}

pub fn init() {}

/// Dump current timerfd and epoll-interest state (for post-mortem diagnosis).
pub fn dump_event_state() {
    let now = monotonic_ns();
    let t = TIMERFD_TABLE.lock();
    log::error!("[event-state] timerfd_table: {} entries (now={}ns)", t.len(), now);
    for (fd, st) in t.iter() {
        let rel = if st.deadline_ns == 0 {
            0i64
        } else {
            st.deadline_ns as i64 - now as i64
        };
        log::error!(
            "[event-state]   tfd={} deadline={}ns ({:+}ns from now) period={}ns pending={}",
            fd, st.deadline_ns, rel, st.period_ns, st.pending
        );
    }
    drop(t);
    let e = EPOLL_TABLE.lock();
    log::error!("[event-state] epoll_table: {} epfd(s)", e.len());
    for (epfd, list) in e.iter() {
        log::error!("[event-state]   epfd={} watching {} fd(s):", epfd, list.len());
        for ent in list.iter() {
            log::error!(
                "[event-state]     fd={} events={:#x} data={:#x}",
                ent.fd, ent.events, ent.data
            );
        }
    }
}

/// Scan the user's stack (from RSP upward) and print any qword that looks
/// like a return address into libflutter_engine.so (loaded at 0x1000000).
/// libflutter has no frame pointers (RBP is used as a GPR), so an RBP walk
/// is impossible — a stack scan is the most we can do without DWARF unwind.
pub fn dump_user_backtrace(depth: usize) {
    let urip = crate::arch::syscall::user_rip();
    let ursp = crate::arch::syscall::user_rsp();
    let urbp = crate::arch::syscall::user_rbp();
    const FLUTTER_BASE: u64 = 0x1000000;
    // .so is ~92MiB; .text typically fits well inside 64MiB. Use a generous
    // window so we don't miss late-loaded sections.
    const FLUTTER_END: u64 = FLUTTER_BASE + 0x0800_0000; // +128MiB
    log::error!(
        "[backtrace] user_rip={:#x} (flutter+{:#x}) user_rsp={:#x} user_rbp={:#x}",
        urip,
        urip.wrapping_sub(FLUTTER_BASE),
        ursp,
        urbp,
    );
    // Sanity: rsp must be a plausible user address.
    if ursp == 0 || ursp & 0x7 != 0 || ursp >= 0x0000_8000_0000_0000 {
        log::error!("[backtrace] bad rsp — aborting scan");
        return;
    }
    let cur_cr3 = crate::arch::memory::read_cr3() & 0x000f_ffff_ffff_f000;
    let mut printed = 0usize;
    // Scan up to 4 KiB of stack (512 qwords).
    let max_words = 512usize;
    for i in 0..max_words {
        let addr = ursp + (i as u64) * 8;
        // Avoid stepping into a guard page near the top.
        if addr >= 0x0000_8000_0000_0000 { break; }
        if crate::mm::paging::translate_user_page(cur_cr3, addr & !0xfff).is_none() {
            break;
        }
        let val = unsafe { core::ptr::read_volatile(addr as *const u64) };
        if val >= FLUTTER_BASE && val < FLUTTER_END {
            log::error!(
                "[backtrace]   stk+{:#06x} = {:#x} (flutter+{:#x})",
                i * 8, val, val - FLUTTER_BASE,
            );
            printed += 1;
            if printed >= depth { break; }
        }
    }
    if printed == 0 {
        log::error!("[backtrace] no libflutter return addresses found in scanned stack");
    }
}

// ── Open-file table ───────────────────────────────────────────────────────────
//
// Simple global open-file table (max 64 entries).  No per-process isolation —
// acceptable for a single-user kernel where one process runs at a time.
// FDs 0..2 = stdin/stdout/stderr (hardwired).  FD 3+ = VFS files.

struct OpenFile {
    data:   &'static [u8],
    offset: u64,
    used:   bool,
    is_dir: bool,
    /// For directory fds: the absolute path of the directory (NUL-padded).
    dir_path: [u8; 192],
    dir_path_len: usize,
    /// For pipe FDs: index into PIPES table (-1 if not a pipe).
    pipe_id: i16,
    pipe_is_write: bool,
}

const MAX_OPEN_FILES: usize = 64;
static OPEN_FILES: spin::Mutex<[OpenFile; MAX_OPEN_FILES]> = spin::Mutex::new(
    [const { OpenFile { data: &[], offset: 0, used: false, is_dir: false, dir_path: [0; 192], dir_path_len: 0, pipe_id: -1, pipe_is_write: false } }; MAX_OPEN_FILES]
);

// ── Pipe buffers ──────────────────────────────────────────────────────────────
// Fixed-size ring buffers shared between paired pipe FDs.  Engine uses pipes
// for thread wakeup / eventfd-like signaling — small buffers are sufficient.

const PIPE_BUF_SIZE: usize = 4096;
const MAX_PIPES: usize = 32;

struct PipeBuf {
    buf: [u8; PIPE_BUF_SIZE],
    head: usize,   // read offset
    tail: usize,   // write offset
    len:  usize,   // bytes currently buffered
    r_refs: u8,    // open read ends
    w_refs: u8,    // open write ends
}

static PIPES: spin::Mutex<[PipeBuf; MAX_PIPES]> = spin::Mutex::new(
    [const { PipeBuf { buf: [0; PIPE_BUF_SIZE], head: 0, tail: 0, len: 0, r_refs: 0, w_refs: 0 } }; MAX_PIPES]
);

/// Allocate an fd (≥3) backed by `data` from the VFS.
fn fd_alloc(data: &'static [u8]) -> Option<u64> {
    let mut tbl = OPEN_FILES.lock();
    for (i, slot) in tbl.iter_mut().enumerate().skip(3) {
        if !slot.used {
            *slot = OpenFile { data, offset: 0, used: true, is_dir: false, dir_path: [0; 192], dir_path_len: 0, pipe_id: -1, pipe_is_write: false };
            return Some(i as u64);
        }
    }
    None
}

/// Allocate an fd that represents an open directory (no data, no read).
/// `path` is the absolute path of the directory, stored for openat() resolution.
fn fd_alloc_dir(path: &str) -> Option<u64> {
    let mut tbl = OPEN_FILES.lock();
    for (i, slot) in tbl.iter_mut().enumerate().skip(3) {
        if !slot.used {
            let bytes = path.as_bytes();
            let n = bytes.len().min(192);
            let mut buf = [0u8; 192];
            buf[..n].copy_from_slice(&bytes[..n]);
            *slot = OpenFile { data: &[], offset: 0, used: true, is_dir: true, dir_path: buf, dir_path_len: n, pipe_id: -1, pipe_is_write: false };
            return Some(i as u64);
        }
    }
    None
}

/// Allocate a pipe pair.  On success writes `[read_fd, write_fd]` (i32) to
/// `pipefd_ptr` and returns 0.  `_flags` accepts O_CLOEXEC / O_NONBLOCK
/// (ignored — pipes are non-blocking).
fn sys_pipe2(pipefd_ptr: u64, _flags: u64) -> i64 {
    if pipefd_ptr == 0 { return -14; } // EFAULT
    // Reserve a pipe buffer slot.
    let pid = {
        let mut pipes = PIPES.lock();
        let mut found = usize::MAX;
        for (i, p) in pipes.iter_mut().enumerate() {
            if p.r_refs == 0 && p.w_refs == 0 {
                p.head = 0; p.tail = 0; p.len = 0;
                p.r_refs = 1; p.w_refs = 1;
                found = i;
                break;
            }
        }
        if found == usize::MAX { return -23; } // ENFILE
        found
    };
    // Allocate two FDs in the open-file table.
    let (read_fd, write_fd) = {
        let mut tbl = OPEN_FILES.lock();
        let mut rfd: i32 = -1;
        let mut wfd: i32 = -1;
        for (i, slot) in tbl.iter_mut().enumerate().skip(3) {
            if !slot.used {
                if rfd < 0 {
                    *slot = OpenFile { data: &[], offset: 0, used: true, is_dir: false, dir_path: [0; 192], dir_path_len: 0, pipe_id: pid as i16, pipe_is_write: false };
                    rfd = i as i32;
                } else {
                    *slot = OpenFile { data: &[], offset: 0, used: true, is_dir: false, dir_path: [0; 192], dir_path_len: 0, pipe_id: pid as i16, pipe_is_write: true };
                    wfd = i as i32;
                    break;
                }
            }
        }
        if rfd < 0 || wfd < 0 {
            // Roll back.
            if rfd >= 0 { tbl[rfd as usize].used = false; tbl[rfd as usize].pipe_id = -1; }
            let mut pipes = PIPES.lock();
            pipes[pid].r_refs = 0; pipes[pid].w_refs = 0;
            return -23;
        }
        (rfd, wfd)
    };
    unsafe {
        let arr = pipefd_ptr as *mut i32;
        core::ptr::write_unaligned(arr,           read_fd);
        core::ptr::write_unaligned(arr.add(1),    write_fd);
    }
    log::debug!("[pipe2] read_fd={} write_fd={} pipe_id={} flags={:#x}", read_fd, write_fd, pid, _flags);
    0
}

/// If `fd` is an open directory fd, return its stored path.
fn fd_dir_path(fd: u64) -> Option<alloc::string::String> {
    let idx = fd as usize;
    if idx >= MAX_OPEN_FILES { return None; }
    let tbl = OPEN_FILES.lock();
    if !tbl[idx].used || !tbl[idx].is_dir { return None; }
    let n = tbl[idx].dir_path_len;
    let s = core::str::from_utf8(&tbl[idx].dir_path[..n]).ok()?;
    Some(alloc::string::String::from(s))
}

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Copy a byte slice from a user virtual address (minimal validation).
/// In a real kernel this would use access_ok + copy_from_user.
/// Here we accept the pointer directly as kernel == user space pre-M13.
unsafe fn read_user_bytes(ptr: u64, len: usize) -> Option<&'static [u8]> {
    if ptr == 0 || len > 0x200_0000 { return None; }
    Some(unsafe { core::slice::from_raw_parts(ptr as *const u8, len) })
}

unsafe fn write_user_bytes(ptr: u64, src: &[u8]) -> bool {
    if ptr == 0 || src.len() > 0x200_0000 {
        return false;
    }
    unsafe {
        core::ptr::copy_nonoverlapping(src.as_ptr(), ptr as *mut u8, src.len());
    }
    true
}

// ── Syscall implementations ───────────────────────────────────────────────────

fn sys_write(fd: u64, buf_ptr: u64, len: u64) -> i64 {
    // Pipe write fast-path.
    let idx = fd as usize;
    if idx >= 3 && idx < MAX_OPEN_FILES {
        let tbl = OPEN_FILES.lock();
        if tbl[idx].used && tbl[idx].pipe_id >= 0 {
            if !tbl[idx].pipe_is_write { return -9; } // EBADF (read end)
            let pid = tbl[idx].pipe_id as usize;
            drop(tbl);
            let src = match unsafe { read_user_bytes(buf_ptr, len as usize) } {
                Some(b) => b,
                None => return -14,
            };
            let mut pipes = PIPES.lock();
            let p = &mut pipes[pid];
            if p.r_refs == 0 { return -32; } // EPIPE
            let free = PIPE_BUF_SIZE - p.len;
            let n = src.len().min(free);
            for i in 0..n {
                p.buf[p.tail] = src[i];
                p.tail = (p.tail + 1) % PIPE_BUF_SIZE;
            }
            p.len += n;
            let written = n;
            {
                static PIPE_WRITE_LOG: core::sync::atomic::AtomicU32 =
                    core::sync::atomic::AtomicU32::new(0);
                // epfd=72 currently watches read_fd=4 on pipe_id=0; the paired
                // write end is fd=5. Trace this path to verify wake source.
                if fd == 5 || pid == 0 {
                    let k = PIPE_WRITE_LOG.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
                    if k < 32 {
                        log::warn!(
                            "[pipe-write] #{} pid={} fd={} pipe_id={} req_len={} wrote={} pipe_len={} free_before={}",
                            k,
                            crate::process::current_pid(),
                            fd,
                            pid,
                            len,
                            written,
                            p.len,
                            free
                        );
                    }
                }
            }
            drop(pipes);

            if written > 0 {
                let mut read_fds = alloc::vec::Vec::new();
                {
                    let open = OPEN_FILES.lock();
                    for r_fd in 0..MAX_OPEN_FILES {
                        if open[r_fd].used && open[r_fd].pipe_id == pid as i16 && !open[r_fd].pipe_is_write {
                            read_fds.push(r_fd as u32);
                        }
                    }
                }
                for r_fd in read_fds {
                    epoll_wake(r_fd);
                }
            }
            return if written == 0 { -11 } else { written as i64 }; // EAGAIN if full
        }
        drop(tbl);
        // Timerfd write: Dart uses write(tfd, &count, 8) to signal wakeup.
        // Treat as eventfd-style: add count to pending, so epoll delivers EPOLLIN.
        {
            let mut tfd_tbl = TIMERFD_TABLE.lock();
            if let Some(ts) = tfd_tbl.get_mut(&(fd as u32)) {
                if len >= 8 && buf_ptr != 0 {
                    let count = unsafe { core::ptr::read_unaligned(buf_ptr as *const u64) };
                    ts.pending = ts.pending.saturating_add(count.max(1));
                } else {
                    ts.pending = ts.pending.saturating_add(1);
                }
                log::warn!("[sys_write] timerfd write fd={} pending={}", fd, ts.pending);
                drop(tfd_tbl);
                epoll_wake(fd as u32);
                return len as i64;
            }
        }
        // Unknown synth fd write — log and return success to avoid breaking callers.
        if idx >= 64 {
            // Check if it's an eventfd before giving up.
            if EVENTFD_TABLE.lock().contains_key(&(fd as u32)) {
                if len >= 8 && buf_ptr != 0 {
                    let val = unsafe { core::ptr::read_unaligned(buf_ptr as *const u64) };
                    eventfd_write(fd as u32, val);
                } else {
                    eventfd_write(fd as u32, 1);
                }
                return len as i64;
            }
            log::warn!("[sys_write] unknown synth fd={} len={}", fd, len);
            return len as i64;
        }
    }
    let bytes = match unsafe { read_user_bytes(buf_ptr, len as usize) } {
        Some(b) => b,
        None => {
            log::warn!("[sys_write] EFAULT fd={} ptr={:#x} len={}", fd, buf_ptr, len);
            return -14; // EFAULT
        }
    };
    if let Ok(s) = core::str::from_utf8(bytes) {
        crate::logger::early_print(s);
    }
    len as i64
}

#[repr(C)]
struct LinuxIovec {
    base: u64,
    len: u64,
}

fn sys_writev(fd: u64, iov_ptr: u64, iov_cnt: u64) -> i64 {
    if iov_ptr == 0 || iov_cnt == 0 {
        return 0;
    }
    if iov_cnt > 1024 {
        return -22; // EINVAL
    }

    let mut total: i64 = 0;
    for i in 0..iov_cnt {
        let ent_ptr = iov_ptr + i * core::mem::size_of::<LinuxIovec>() as u64;
        let ent = unsafe { core::ptr::read_unaligned(ent_ptr as *const LinuxIovec) };
        if ent.len == 0 {
            continue;
        }
        let n = sys_write(fd, ent.base, ent.len);
        if n < 0 {
            return if total > 0 { total } else { n };
        }
        total += n;
        if n as u64 != ent.len {
            // Short write (e.g. pipe full): stop and return bytes written so far.
            break;
        }
    }
    total
}

fn sys_read(fd: u64, buf_ptr: u64, len: u64) -> i64 {
    if len == 0 { return 0; }
    if buf_ptr == 0 { return -14; } // EFAULT

    // Timerfd read: return expiry count (u64, 8 bytes) and drain the timer.
    // This is called by Flutter's TimerDrain() after epoll_wait signals EPOLLIN.
    {
        let mut tfd_tbl = TIMERFD_TABLE.lock();
        if let Some(ts) = tfd_tbl.get_mut(&(fd as u32)) {
            if len < 8 { return -22; } // EINVAL
            let now = monotonic_ns();
            // Accumulate any deadline-based expirations into pending.
            if ts.deadline_ns != 0 && now >= ts.deadline_ns {
                if ts.period_ns != 0 {
                    let elapsed = now.saturating_sub(ts.deadline_ns);
                    let n = 1u64 + elapsed / ts.period_ns;
                    ts.deadline_ns = ts.deadline_ns.saturating_add(n * ts.period_ns);
                    ts.pending = ts.pending.saturating_add(n);
                } else {
                    ts.deadline_ns = 0;
                    ts.pending = ts.pending.saturating_add(1);
                }
            }
            if ts.pending > 0 {
                let count = ts.pending;
                ts.pending = 0;
                drop(tfd_tbl);
                let pid = crate::process::current_pid();
                log::warn!("[tfd-read] pid={} tfd={} count={}", pid, fd, count);
                unsafe { core::ptr::write_unaligned(buf_ptr as *mut u64, count); }
                return 8;
            } else {
                // Not yet fired (or not armed) — EAGAIN.
                return -11;
            }
        }
    }

    // Eventfd read: drain counter atomically, return 8 bytes.
    {
        let mut efd_tbl = EVENTFD_TABLE.lock();
        if let Some(c) = efd_tbl.get_mut(&(fd as u32)) {
            if len < 8 { return -22; } // EINVAL
            if *c == 0 { return -11; } // EAGAIN (non-blocking)
            let val = *c;
            *c = 0;
            drop(efd_tbl);
            unsafe { core::ptr::write_unaligned(buf_ptr as *mut u64, val); }
            return 8;
        }
    }

    let to_read = len as usize;
    let mut tbl = OPEN_FILES.lock();
    let idx = fd as usize;
    if idx >= MAX_OPEN_FILES || !tbl[idx].used {
        return -9; // EBADF
    }
    // Pipe read fast-path.
    if tbl[idx].pipe_id >= 0 {
        if tbl[idx].pipe_is_write { return -9; } // EBADF (write end)
        let pid = tbl[idx].pipe_id as usize;
        drop(tbl);
        let mut pipes = PIPES.lock();
        let p = &mut pipes[pid];
        if p.len == 0 {
            // No data.  If all writers closed → EOF; otherwise EAGAIN.
            return if p.w_refs == 0 { 0 } else { -11 };
        }
        let n = to_read.min(p.len);
        let dst = buf_ptr as *mut u8;
        for i in 0..n {
            unsafe { *dst.add(i) = p.buf[p.head]; }
            p.head = (p.head + 1) % PIPE_BUF_SIZE;
        }
        p.len -= n;
        return n as i64;
    }
    let offset = tbl[idx].offset as usize;
    let data   = tbl[idx].data;
    if offset >= data.len() { return 0; } // EOF
    let available = data.len() - offset;
    let n = to_read.min(available);
    unsafe {
        core::ptr::copy_nonoverlapping(
            data.as_ptr().add(offset),
            buf_ptr as *mut u8,
            n,
        );
    }
    tbl[idx].offset += n as u64;
    n as i64
}

fn sys_open(path_ptr: u64, path_len: u64, _flags: u64) -> i64 {
    let bytes = match unsafe { read_user_bytes(path_ptr, path_len as usize) } {
        Some(b) => b,
        None => return -14, // EFAULT
    };
    let path = match core::str::from_utf8(bytes) {
        Ok(s) => s,
        Err(_) => return -22, // EINVAL
    };
    // Strip null terminator if present.
    let path = path.trim_end_matches('\0');
    match crate::fs::lookup(path) {
        Some(data) => {
            match fd_alloc(data) {
                Some(fd) => {
                    log::debug!("[sys_open] '{}' → fd={} ({} bytes)", path, fd, data.len());
                    fd as i64
                }
                None => -23, // ENFILE (too many open files)
            }
        }
        None => {
            // File not found — maybe it's a directory.
            if crate::fs::is_dir(path) {
                match fd_alloc_dir(path) {
                    Some(fd) => {
                        log::debug!("[sys_open] '{}' → fd={} (dir)", path, fd);
                        return fd as i64;
                    }
                    None => return -23,
                }
            }
            log::debug!("[sys_open] '{}' → ENOENT", path);
            -2 // ENOENT
        }
    }
}

fn sys_close(fd: u64) -> i64 {
    let idx = fd as usize;
    if idx < 3 || idx >= MAX_OPEN_FILES { return 0; }
    let mut tbl = OPEN_FILES.lock();
    if tbl[idx].used {
        // If this is a pipe end, decrement the pipe's refcount.
        if tbl[idx].pipe_id >= 0 {
            let pid = tbl[idx].pipe_id as usize;
            let is_write = tbl[idx].pipe_is_write;
            tbl[idx].pipe_id = -1;
            drop(tbl);
            let mut pipes = PIPES.lock();
            if is_write && pipes[pid].w_refs > 0 { pipes[pid].w_refs -= 1; }
            if !is_write && pipes[pid].r_refs > 0 { pipes[pid].r_refs -= 1; }
            // Re-acquire to clear the file slot.
            drop(pipes);
            let mut tbl = OPEN_FILES.lock();
            tbl[idx].used = false;
            tbl[idx].data = &[];
            tbl[idx].offset = 0;
            return 0;
        }
        tbl[idx].used = false;
        tbl[idx].data = &[];
        tbl[idx].offset = 0;
    }
    0
}

fn sys_lseek(fd: u64, offset: i64, whence: u64) -> i64 {
    let idx = fd as usize;
    if idx < 3 || idx >= MAX_OPEN_FILES { return -9; } // EBADF
    let mut tbl = OPEN_FILES.lock();
    if !tbl[idx].used { return -9; }
    let size = tbl[idx].data.len() as i64;
    let new_off: i64 = match whence {
        0 => offset,                                    // SEEK_SET
        1 => tbl[idx].offset as i64 + offset,           // SEEK_CUR
        2 => size + offset,                             // SEEK_END
        _ => return -22, // EINVAL
    };
    if new_off < 0 { return -22; }
    tbl[idx].offset = new_off as u64;
    new_off
}

fn sys_fstat(fd: u64, stat_ptr: u64) -> i64 {
    // Minimal stat: only st_size matters for the Flutter engine.
    let idx = fd as usize;
    let (size, is_dir): (u64, bool) = if idx < 3 {
        (0, false)
    } else {
        let tbl = OPEN_FILES.lock();
        if !tbl[idx].used { return -9; }
        (tbl[idx].data.len() as u64, tbl[idx].is_dir)
    };
    if stat_ptr != 0 {
        // Write a minimal stat64 struct — st_size is at offset 48 in Linux stat64.
        unsafe {
            let p = stat_ptr as *mut u8;
            // Zero out 144 bytes (sizeof(struct stat64))
            core::ptr::write_bytes(p, 0, 144);
            // st_size at offset 48
            (p.add(48) as *mut u64).write_unaligned(size);
            // st_blksize at offset 56 = 4096
            (p.add(56) as *mut u64).write_unaligned(4096);
            // st_blocks at offset 64 = (size+511)/512
            (p.add(64) as *mut u64).write_unaligned((size + 511) / 512);
            // st_mode at offset 24: S_IFDIR|0755 for dirs, S_IFREG|0644 for files
            let mode: u32 = if is_dir { 0o040755 } else { 0o100644 };
            (p.add(24) as *mut u32).write_unaligned(mode);
        }
    }
    0
}

fn sys_getpid() -> i64 {
    let pid = crate::process::current_pid();
    if pid == 0 {
        return 1;
    }
    crate::process::get_group_leader(pid) as i64
}

fn sys_gettid() -> i64 {
    let pid = crate::process::current_pid();
    if pid == 0 { 1 } else { pid as i64 }
}

/// Linux `stat(const char *path, struct stat *buf)` — path is NUL-terminated.
/// Used by glibc `stat()`/`lstat()` and consequently by Flutter's
/// `fml::IsFile()` to verify `kernel_blob.bin` and similar assets exist.
fn sys_stat_path(path_ptr: u64, stat_ptr: u64) -> i64 {
    if path_ptr == 0 { return -14; } // EFAULT
    // Determine NUL-terminated length (bounded probe).
    let mut len: usize = 0;
    unsafe {
        let p = path_ptr as *const u8;
        while len < 4096 && *p.add(len) != 0 { len += 1; }
    }
    let fd = sys_open(path_ptr, len as u64, 0);
    if fd < 0 { return fd; }
    let r = sys_fstat(fd as u64, stat_ptr);
    let _ = sys_close(fd as u64);
    r
}

/// Linux `access(const char *path, int mode)` — returns 0 if file exists.
fn sys_access_path(path_ptr: u64, _mode: u64) -> i64 {
    if path_ptr == 0 { return -14; }
    let mut len: usize = 0;
    unsafe {
        let p = path_ptr as *const u8;
        while len < 4096 && *p.add(len) != 0 { len += 1; }
    }
    let fd = sys_open(path_ptr, len as u64, 0);
    if fd < 0 { return fd; }
    let _ = sys_close(fd as u64);
    0
}

/// Linux `newfstatat(int dirfd, const char *path, struct stat *buf, int flags)`.
/// We ignore `dirfd` (treat path as absolute) and delegate to `sys_stat_path`.
fn sys_newfstatat(_dirfd: u64, path_ptr: u64, stat_ptr: u64, _flags: u64) -> i64 {
    sys_stat_path(path_ptr, stat_ptr)
}

fn sys_exit(code: u64) -> i64 {
    let pid = crate::process::current_pid();
    if pid != 0 {
        // Read parent BEFORE exit() so we can wake them after.
        let parent_pid = crate::process::get_parent_of(pid);
        crate::process::exit(pid, code as i32);

        if let Some(ppid) = parent_pid {
            // We do NOT wake the parent here. Even if the parent is in the
            // `Blocked` state, that block could be for ANY reason —
            // futex_wait, pthread_cond_wait, etc — not just waitpid().
            // Clobbering the parent's RAX with our exit code makes
            // pthread_cond_wait return -6, which Flutter's libc++ wraps
            // and re-throws as "condition_variable wait failed" → abort.
            // The legitimate wake paths (cond_signal, futex_wake, real
            // waitpid polling) will reap the zombie themselves.
            log::warn!(
                "[trace] sys_exit: pid={} → parent ppid={} left untouched (zombie pending)",
                pid, ppid
            );
        }
        log::warn!("[trace] sys_exit: pid={} no parent — falling through to scheduler", pid);

        // On abnormal exit (negative code), wake all blocked threads so any
        // futex/cond_wait deadlock chain can unwind (e.g. vm-service death).
        if (code as i64) < 0 {
            let n_woken = {
                let mut count: u32 = 0;
                for wake_pid in 1..crate::process::MAX_PROCS as u32 {
                    if crate::process::is_blocked(wake_pid) {
                        crate::process::set_state(wake_pid, crate::process::ProcState::Running);
                        count += 1;
                    }
                }
                count
            };
            if n_woken > 0 {
                log::warn!("[trace] sys_exit: abnormal exit code={} — woke {} blocked threads",
                    code as i64, n_woken);
                FUTEX_WAITERS.lock().clear();
            }
        }

        if let Some(next_pid) = crate::process::next_runnable_pid(0) {
            crate::process::enter_user_by_pid_noreturn(next_pid);
        }
    }
    // No runnable user process left.
    loop {
        unsafe { core::arch::asm!("sti; hlt; cli", options(nomem, nostack)); }
    }
}

fn sys_waitpid(pid: u64, _status_ptr: u64, _options: u64) -> i64 {
    match crate::process::waitpid(pid as u32) {
        Ok(code) => code as i64,
        Err("not exited") => -11,      // EAGAIN
        Err("no such process") => -10, // ECHILD
        Err(_) => -22,                  // EINVAL
    }
}

fn sys_kill(pid: u64, sig: u64) -> i64 {
    if sig != 9 {
        return -22; // EINVAL (only SIGKILL wired today)
    }
    match crate::process::kill(pid as u32) {
        Ok(()) => 0,
        Err(_) => -3, // ESRCH
    }
}

fn sys_arch_prctl(code: u64, addr: u64) -> i64 {
    // x86_64 Linux arch_prctl operations used by libc TLS setup.
    const ARCH_SET_FS: u64 = 0x1002;
    const ARCH_GET_FS: u64 = 0x1003;

    match code {
        ARCH_SET_FS => {
            crate::arch::cpu::set_fs_base(addr);
            let pid = crate::process::current_pid();
            if pid != 0 {
                crate::process::set_proc_fs_base(pid, addr);
            }
            0
        }
        ARCH_GET_FS => {
            if addr == 0 {
                return -14; // EFAULT
            }
            let fs = crate::arch::cpu::get_fs_base();
            unsafe { *(addr as *mut u64) = fs; }
            0
        }
        _ => -22, // EINVAL
    }
}

fn sys_exec(path_ptr: u64, path_len: u64) -> i64 {
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
        None => return -2, // ENOENT
    };
    match crate::process::spawn(elf, path) {
        Ok(pid) => pid as i64,
        Err(_)  => -12, // ENOMEM
    }
}

/// Phase 47 — SYS_EXEC_WAIT (0x39E): spawn a child, block the calling
/// process, context-switch to the child immediately, and return the child's
/// exit code when it calls sys_exit.
///
/// This is a cooperative "exec-and-wait" that works without a preemptive
/// scheduler: the parent blocks, the child runs to completion, then sys_exit
/// restores the parent with RAX = exit_code.
fn sys_exec_wait(path_ptr: u64, path_len: u64) -> i64 {
    let bytes = match unsafe { read_user_bytes(path_ptr, path_len as usize) } {
        Some(b) => b,
        None    => return -14, // EFAULT
    };
    let path = match core::str::from_utf8(bytes) {
        Ok(s)  => s,
        Err(_) => return -22, // EINVAL
    };
    let elf = match crate::fs::lookup(path) {
        Some(data) => data,
        None       => return -2, // ENOENT
    };
    let parent_pid = crate::process::current_pid();
    if parent_pid == 0 { return -1; } // must have an active user process

    let child_pid = match crate::process::spawn(elf, path) {
        Ok(pid) => pid,
        Err(_)  => return -12, // ENOMEM
    };

    // Wire the child back to the parent so sys_exit knows who to wake.
    crate::process::set_child_parent(child_pid, parent_pid);

    // Save the parent's return context (RIP = next instr after syscall, RSP).
    let parent_rip = crate::arch::syscall::user_rip();
    let parent_rsp = crate::arch::syscall::user_rsp();
    crate::process::save_return_context(parent_pid, parent_rip, parent_rsp);
    crate::process::save_full_user_gprs(parent_pid);

    // Block the parent — it will be unblocked by the child's sys_exit.
    crate::process::set_state(parent_pid, crate::process::ProcState::Blocked);

    // Switch the kernel's notion of current PID and SYSRET into the child.
    crate::process::enter_user_by_pid_noreturn(child_pid)
}

fn sys_poweroff() -> i64 {
    log::info!("[syscall] sys_poweroff requested — triggering ACPI S5 shutdown");
    #[cfg(target_arch = "x86_64")]
    crate::arch::acpi_shutdown();
    #[cfg(not(target_arch = "x86_64"))]
    loop { unsafe { core::arch::asm!("hlt", options(nomem, nostack)); } }
}

fn sys_ipc_send(dst_pid: u64, msg_ptr: u64, msg_len: u64) -> i64 {
    let bytes = match unsafe { read_user_bytes(msg_ptr, (msg_len as usize).min(64)) } {
        Some(b) => b,
        None => return -14,
    };
    crate::ipc::send(dst_pid as u32, bytes);
    0
}

fn sys_ipc_recv(buf_ptr: u64, buf_len: u64) -> i64 {
    let pid = crate::process::current_pid();
    if pid == 0 {
        return -11; // no active userspace pid yet
    }
    match crate::ipc::recv(pid) {
        Some(msg) => {
            let copy_len = (buf_len as usize).min(msg.len());
            if buf_ptr == 0 { return -14; }
            unsafe {
                core::ptr::copy_nonoverlapping(
                    msg.as_ptr(),
                    buf_ptr as *mut u8,
                    copy_len,
                );
            }
            copy_len as i64
        }
        None => -11, // EAGAIN
    }
}

fn sys_surface_create(width: u64, height: u64) -> i64 {
    match crate::compositor::create_surface_for(wm_consumer_pid(), width as u32, height as u32) {
        Ok(id) => id as i64,
        Err("invalid size") => -22, // EINVAL
        Err(_) => -12,               // ENOMEM/table full
    }
}

fn sys_surface_move(id: u64, packed_xy: u64, z: u64) -> i64 {
    let x = ((packed_xy >> 32) as u32) as i32;
    let y = (packed_xy as u32) as i32;
    match crate::compositor::move_surface_for(wm_consumer_pid(), id as u32, x, y, z as i32) {
        Ok(()) => 0,
        Err("permission denied") => -1, // EPERM
        Err(_) => -3, // ESRCH
    }
}

fn sys_surface_destroy(id: u64) -> i64 {
    match crate::compositor::destroy_surface_for(wm_consumer_pid(), id as u32) {
        Ok(()) => 0,
        Err("permission denied") => -1, // EPERM
        Err(_) => -3, // ESRCH
    }
}

fn sys_surface_upload(id: u64, buf_ptr: u64, buf_len: u64) -> i64 {
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

fn sys_surface_present(id: u64) -> i64 {
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

fn sys_fb_size_packed() -> i64 {
    crate::compositor::framebuffer_size_packed() as i64
}

fn sys_vsync_counter() -> i64 {
    crate::compositor::frame_counter() as i64
}

fn sys_vsync_wait_nonblock(last_seen: u64) -> i64 {
    match crate::compositor::wait_vsync(last_seen) {
        Some(v) => v as i64,
        None => -11, // EAGAIN
    }
}

fn sys_surface_owner(id: u64) -> i64 {
    match crate::compositor::surface_owner(id as u32) {
        Some(pid) => pid as i64,
        None => -3, // ESRCH
    }
}

fn sys_surface_z_get(id: u64) -> i64 {
    match crate::compositor::surface_z_get(id as u32) {
        Some(z) => z as i64,
        None => -3, // ESRCH
    }
}

fn sys_surface_z_set(id: u64, z: u64) -> i64 {
    match crate::compositor::surface_z_set_for(wm_consumer_pid(), id as u32, z as i32) {
        Ok(()) => 0,
        Err("permission denied") => -1, // EPERM
        Err(_) => -3, // ESRCH
    }
}

fn sys_surface_geometry_get(id: u64) -> i64 {
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

fn sys_surface_geometry_set(id: u64, xy_packed: u64, wh_packed: u64) -> i64 {
    let x = ((xy_packed >> 32) as u32) as i32;
    let y = (xy_packed as u32) as i32;
    let w = ((wh_packed >> 32) as u32) as u32;
    let h = (wh_packed as u32) as u32;
    
    match crate::compositor::surface_geometry_set_for(wm_consumer_pid(), id as u32, x, y, w, h) {
        Ok(()) => 0,
        Err("invalid size") => -22, // EINVAL
        Err("surface too large") => -12, // ENOMEM
        Err("permission denied") => -1, // EPERM
        Err(_) => -3, // ESRCH
    }
}

fn sys_surface_visibility_get(id: u64) -> i64 {
    match crate::compositor::surface_visibility_get(id as u32) {
        Some(visible) => if visible { 1 } else { 0 },
        None => -3, // ESRCH
    }
}

fn sys_surface_visibility_set(id: u64, visible: u64) -> i64 {
    match crate::compositor::surface_visibility_set_for(wm_consumer_pid(), id as u32, visible != 0) {
        Ok(()) => 0,
        Err("permission denied") => -1,
        Err(_) => -3,
    }
}

fn sys_surface_clip_set(id: u64, xy_packed: u64, wh_packed: u64) -> i64 {
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

fn sys_surface_damage_set(id: u64, xy_packed: u64, wh_packed: u64) -> i64 {
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

fn sys_surface_damage_get(id: u64) -> i64 {
    match crate::compositor::surface_damage_get(id as u32) {
        Some((x, y, _w, _h, true)) => {
            // Return packed xy; caller uses separate surface_geometry syscall for dimensions.
            ((x as u32 as u64) << 32 | y as u32 as u64) as i64
        }
        Some((_, _, _, _, false)) => -11, // EAGAIN — no pending damage
        None => -3,                        // ESRCH
    }
}

fn sys_surface_flip(id: u64) -> i64 {
    match crate::compositor::surface_flip_for(wm_consumer_pid(), id as u32) {
        Ok(()) => 0,
        Err("permission denied") => -1,
        Err(_) => -3,
    }
}

fn sys_app_notify(target_pid: u64, subkind: u64, surface_id: u64) -> i64 {
    crate::wm::push_app_event(target_pid as u32, subkind as u32, surface_id as u32);
    0
}

fn sys_proc_surface_count(pid: u64) -> i64 {
    crate::compositor::surface_count_for(pid as u32) as i64
}

fn sys_app_launch_path(path_ptr: u64, path_len: u64, _flags: u64) -> i64 {
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
    match crate::process::spawn(elf, path) {
        Ok(pid) => {
            crate::wm::push_app_event(pid, eabi::APP_LAUNCH, 0);
            let host_pid = ENGINE_HOST_PID.load(Ordering::Acquire);
            if host_pid != 0 {
                // Notify the runtime host about the launched app PID.
                crate::wm::push_event_for(host_pid, eabi::EV_APP, eabi::APP_LAUNCH, pid as u64, 0);
            }
            pid as i64
        }
        Err(_) => -12,
    }
}

fn sys_engine_policy_get() -> i64 {
    // packed: [63:32]=engine target family, [31:0]=loader strategy
    let packed = ((eabi::ENGINE_TARGET_FLUTTER_3_29 as u64) << 32)
        | (eabi::ENGINE_LOADER_DYNAMIC as u64);
    packed as i64
}

fn sys_engine_version_packed() -> i64 {
    // Flutter 3.29.0 baseline.
    let packed = (3u64 << 32) | (29u64 << 16);
    packed as i64
}

fn sys_engine_host_register(_flags: u64) -> i64 {
    let pid = crate::process::current_pid();
    if pid == 0 {
        return -1; // EPERM (kernel context cannot be engine host)
    }
    ENGINE_HOST_PID.store(pid, Ordering::Release);
    pid as i64
}

fn sys_engine_host_pid_get() -> i64 {
    ENGINE_HOST_PID.load(Ordering::Acquire) as i64
}

fn sys_engine_library_path_read(dst_ptr: u64, dst_len: u64) -> i64 {
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
fn wm_consumer_pid() -> u32 {
    let pid = crate::process::current_pid();
    if pid == 0 { 1 } else { pid }
}

fn sys_wm_event_poll() -> i64 {
    crate::wm::pending_count_for(wm_consumer_pid()) as i64
}

fn sys_wm_event_read(ev_ptr: u64, ev_len: u64) -> i64 {
    let need = crate::wm::event_size();
    if (ev_len as usize) < need {
        return -22; // EINVAL
    }

    let ev = match crate::wm::pop_event_for(wm_consumer_pid()) {
        Some(e) => e,
        None => return -11, // EAGAIN
    };

    let bytes = unsafe {
        core::slice::from_raw_parts(
            (&ev as *const crate::wm::WmEvent) as *const u8,
            need,
        )
    };
    if unsafe { write_user_bytes(ev_ptr, bytes) } {
        need as i64
    } else {
        -14 // EFAULT
    }
}

fn sys_wm_event_inject(kind: u64, arg1: u64, arg2: u64) -> i64 {
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

fn sys_embedder_abi_version() -> i64 {
    eabi::EMBEDDER_ABI_VERSION as i64
}

fn sys_wm_event_size() -> i64 {
    eabi::WM_EVENT_SIZE as i64
}

fn sys_wm_event_stats_packed() -> i64 {
    let pending = crate::wm::pending_count_for(wm_consumer_pid()).min(u32::MAX as usize) as u64;
    let dropped = crate::wm::dropped_count().min(u32::MAX as u64);
    // packed: (dropped << 32) | pending
    ((dropped << 32) | pending) as i64
}

fn sys_wm_focus_pid_get() -> i64 {
    crate::wm::focus_pid() as i64
}

fn sys_wm_focus_surface_set(surface_id: u64) -> i64 {
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

fn sys_wm_focus_mirror_get() -> i64 {
    if crate::wm::focus_mirror_enabled() { 1 } else { 0 }
}

fn sys_wm_focus_mirror_set(enabled: u64) -> i64 {
    let on = enabled != 0;
    crate::wm::set_focus_mirror_enabled(on);
    if on { 1 } else { 0 }
}

fn sys_wm_event_wait(ev_ptr: u64, ev_len: u64, _max_halts: u64) -> i64 {
    // Fast path: events already queued.
    let n = sys_wm_event_read(ev_ptr, ev_len);
    if n >= 0 {
        return n;
    }

    // Yield to another runnable thread so Dart/engine threads can progress.
    // The embedder loops on EAGAIN, so we return -11 and let the caller retry.
    let cur = crate::process::current_pid();
    if cur != 0 {
        if let Some(next) = crate::process::next_runnable_pid(cur) {
            if next != cur {
                let urip = crate::arch::syscall::user_rip();
                let ursp = crate::arch::syscall::user_rsp();
                crate::process::save_return_context(cur, urip, ursp);
                crate::process::save_full_user_gprs(cur);
                crate::process::set_errno_to_deliver(cur, 11); // EAGAIN
                crate::process::set_rax(cur, (-11i64) as u64);
                crate::process::save_xstate(cur);
                crate::process::enter_user_by_pid_noreturn(next);
            }
        }
    }

    // No sibling — spin briefly then return EAGAIN.
    for _ in 0..10_000 {
        core::hint::spin_loop();
    }
    let n2 = sys_wm_event_read(ev_ptr, ev_len);
    if n2 >= 0 { n2 } else { -11 }
}

// ── Phase 30 Slice 3: kernel dynamic linker + anonymous mmap ─────────────────

fn sys_dlopen(path_ptr: u64, path_len: u64, _flags: u64) -> i64 {
    let bytes = match unsafe { read_user_bytes(path_ptr, path_len as usize) } {
        Some(b) => b,
        None => return -14, // EFAULT
    };
    let path = match core::str::from_utf8(bytes) {
        Ok(s) => s,
        Err(_) => return -22, // EINVAL
    };
    let pid = crate::process::current_pid();
    let pml4_phys = match crate::process::get_user_context(pid) {
        Some(ctx) => ctx.pml4_phys,
        None => return -9, // EBADF
    };
    // Serve libflutter_engine.so from the Limine module if available.
    let engine_guard;
    let elf: &[u8] = if path.contains("libflutter_engine") {
        engine_guard = crate::FLUTTER_ENGINE_BYTES.lock();
        if let Some(data) = engine_guard.as_ref() {
            log::info!("[dlopen] Serving libflutter_engine.so from Limine module ({} bytes)", data.len());
            data
        } else {
            log::warn!("[dlopen] libflutter_engine.so requested but not available as Limine module");
            return -2; // ENOENT
        }
    } else {
        drop({
            // Need to satisfy borrow checker — create a temporary guard that we immediately drop.
            // We don't actually use it here.
            let _unused = crate::FLUTTER_ENGINE_BYTES.lock();
            _unused
        });
        match crate::fs::lookup(path) {
            Some(data) => data,
            None => return -2, // ENOENT
        }
    };
    match crate::process::dl::dlopen(pid, pml4_phys, elf) {
        Ok(h)  => h as i64,
        Err(e) => {
            log::warn!("[syscall] dlopen '{}' failed: {}", path, e);
            -12 // ENOMEM / generic load failure
        }
    }
}

fn sys_dlsym(handle: u64, name_ptr: u64, name_len: u64) -> i64 {
    if name_ptr == 0 { return 0; } // POSIX: NULL on bad args
    // The libc-ABI `dlsym(handle, name)` trampoline only passes 2 args, so
    // `name_len` here is garbage from the caller's rdx. If it's zero or
    // unreasonable, fall back to strlen on the C-string. dlsym MUST return
    // NULL (0) on any failure — never a kernel errno like -14, which the
    // engine would mistake for a valid pointer.
    let effective_len = if name_len == 0 || name_len > 0x1000 {
        posix::sys_strlen(name_ptr) as usize
    } else {
        name_len as usize
    };
    let name = match unsafe { read_user_bytes(name_ptr, effective_len) } {
        Some(b) => b,
        None => return 0, // not found
    };
    match crate::process::dl::dlsym(handle as u32, name) {
        Some(addr) => addr as i64,
        None       => 0, // POSIX: NULL (0) means not found
    }
}

fn sys_dlclose(handle: u64) -> i64 {
    let pid = crate::process::current_pid();
    crate::process::dl::dlclose(handle as u32, pid);
    0
}

/// Return init function info for a dynamically loaded library so the embedder
/// can call C++ constructors from user space.
///
/// Writes to the three out-pointers (any of which may be null to skip):
///   `*out_init_fn`   ← DT_INIT function VA  (0 if none)
///   `*out_array_va`  ← VA of first DT_INIT_ARRAY entry  (0 if none)
///   `*out_count`     ← number of entries in the array  (0 if none)
fn sys_dl_get_init_array(handle: u64, out_init_fn: u64, out_array_va: u64, out_count: u64) -> i64 {
    let pid = crate::process::current_pid();
    match crate::process::dl::get_init_fns(handle as u32, pid) {
        Some((init_fn, array_va, count)) => {
            // SAFETY: pointers are in user address space while CR3 = user PML4.
            unsafe {
                if out_init_fn  != 0 { core::ptr::write_unaligned(out_init_fn  as *mut u64, init_fn);       }
                if out_array_va != 0 { core::ptr::write_unaligned(out_array_va as *mut u64, array_va);      }
                if out_count    != 0 { core::ptr::write_unaligned(out_count    as *mut u64, count as u64);  }
            }
            0
        }
        None => -9, // EBADF — unknown handle
    }
}

fn sys_mmap(hint_va: u64, size: u64, prot: u64) -> i64 {
    if size == 0 || size > 0x1000_0000 {
        return -22; // EINVAL, max 256 MiB
    }
    let pid = crate::process::current_pid();
    if pid == 0 { return -1; } // EPERM
    let pml4_phys = match crate::process::get_user_context(pid) {
        Some(ctx) => ctx.pml4_phys,
        None => return -9, // EBADF
    };
    let pages = ((size as usize) + 4095) / 4096;
    let va = crate::process::dl::mmap_anon(pid, pml4_phys, hint_va, pages, prot);

    if va == u64::MAX { -12 } else { va as i64 } // ENOMEM or success
}

fn sys_munmap(_va: u64, _size: u64) -> i64 {
    // Stub — frames remain mapped until process exit.
    // Full unmapping requires reverse page-table mappings (future milestone).
    0
}

fn sys_mprotect(_va: u64, _size: u64, _prot: u64) -> i64 {
    // Stub — permissions are set at map time for now.
    // A full implementation would update PTE flags and flush the TLB range.
    0
}

// ── Phase 31 Slice A: FlutterEngineProcTable bridge ──────────────────────────

/// Store the user-space VA of the embedder's `FlutterEngineProcTable` so any
/// kernel subsystem (or a second process via `sys_engine_proctable_ptr_get`)
/// can resolve engine entry points without a second dlsym round-trip.
fn sys_engine_proctable_set(ptr: u64, size: u64) -> i64 {
    // Basic sanity: pointer must be non-null and struct must be large enough.
    if ptr == 0 || (size as usize) < core::mem::size_of::<eabi::FlutterEngineProcTable>() {
        return -22; // EINVAL
    }
    ENGINE_PROC_TABLE_PTR.store(ptr, Ordering::Release);
    0
}

/// Return the previously registered proc-table VA, or 0 if none.
fn sys_engine_proctable_ptr_get() -> i64 {
    ENGINE_PROC_TABLE_PTR.load(Ordering::Acquire) as i64
}

/// Record a vsync baton posted by the embedder.
///
/// Flow: engine calls `vsync_callback(user_data, baton)` → embedder calls
/// `sys_engine_vsync_baton_post(baton)` → kernel stores it → on next vsync
/// the EV_VSYNC event carries `b = baton` → embedder reads it and calls
/// `FlutterEngineOnVsync(engine, baton, start_ns, target_ns)`.
fn sys_engine_vsync_baton_post(baton: u64) -> i64 {
    crate::wm::set_vsync_baton(baton);
    0
}

// ── Phase 31 Slice B: GPU / software-rasterizer fast path ────────────────────

/// Upload RGBA32 pixel data and present a surface in a single call.
///
/// This is the kernel fast-path for the Flutter engine's software-renderer
/// `present_callback(user_data, allocation, row_bytes, height)`.  The caller
/// passes the full tightly-packed (row_bytes == width*4) RGBA32 buffer.
///
/// `arg0` = surface_id, `arg1` = pixel_ptr, `arg2` = pixel_len (bytes)
fn sys_gpu_submit(surface_id: u64, pixel_ptr: u64, pixel_len: u64) -> i64 {
    static GPU_SUBMIT_LOG: core::sync::atomic::AtomicU32 =
        core::sync::atomic::AtomicU32::new(0);
    let n = GPU_SUBMIT_LOG.fetch_add(1, Ordering::Relaxed);
    if n < 128 {
        log::warn!(
            "[frame-boundary] gpu_submit #{} pid={} sid={} ptr={:#x} len={}",
            n,
            crate::process::current_pid(),
            surface_id,
            pixel_ptr,
            pixel_len
        );
    }
    let bytes = match unsafe { read_user_bytes(pixel_ptr, pixel_len as usize) } {
        Some(b) => b,
        None => return -14, // EFAULT
    };
    let pid = wm_consumer_pid();
    match crate::compositor::gpu_submit_for(pid, surface_id as u32, bytes) {
        Ok(()) => 0,
        Err("bad payload size") => -22, // EINVAL
        Err("permission denied") => -1, // EPERM
        Err("no such surface") => -3,   // ESRCH
        Err(_) => -12,
    }
}

// ── Phase 32-C: Platform channel syscalls ────────────────────────────────────

/// Post a message from a native caller to the Flutter platform channel bridge.
/// `arg0` = channel_ptr, `arg1` = channel_len, `arg2` = data_ptr, `arg3` = data_len
/// Returns the `u64` sequence number on success, or a negative errno.
fn sys_platform_msg_post(ch_ptr: u64, ch_len: u64, data_ptr: u64, data_len: u64) -> i64 {
    static PLATFORM_POST_LOG: core::sync::atomic::AtomicU32 =
        core::sync::atomic::AtomicU32::new(0);
    let channel = match unsafe { read_user_bytes(ch_ptr, ch_len as usize) } {
        Some(b) => b,
        None => return -14, // EFAULT
    };
    let payload = match unsafe { read_user_bytes(data_ptr, data_len as usize) } {
        Some(b) => b,
        None => return -14,
    };
    let pid = crate::process::current_pid();
    let k = PLATFORM_POST_LOG.fetch_add(1, Ordering::Relaxed);
    if k < 16 {
        log::info!(
            "[platform-post] #{} pid={} ch_len={} payload_len={}",
            k,
            pid,
            channel.len(),
            payload.len()
        );
    }
    match crate::platform_channel::post(pid, channel, payload) {
        Ok(seq) => {
            // Compute a quick FNV-1a hash of the channel name for the WM event.
            let hash = channel.iter().fold(0x811c9dc5u32, |h, &b| {
                h.wrapping_mul(0x01000193) ^ b as u32
            });
            crate::platform_channel::notify_engine_host(seq, hash);
            seq as i64
        }
        Err("payload too large") => -27,  // EFBIG
        Err("platform message queue full") => -11, // EAGAIN
        Err(_) => -1,
    }
}

/// Copy the oldest pending platform-channel message into a user buffer.
/// `arg0` = buf_ptr, `arg1` = buf_len
/// Returns the number of bytes written, or 0 if no message is pending.
fn sys_platform_msg_recv(buf_ptr: u64, buf_len: u64) -> i64 {
    static PLATFORM_RECV_LOG: core::sync::atomic::AtomicU32 =
        core::sync::atomic::AtomicU32::new(0);
    if buf_ptr == 0 || buf_len == 0 {
        return -14; // EFAULT
    }
    // Allocate a kernel-side buffer, fill via platform_channel, then copy out.
    let len = buf_len as usize;
    let mut kbuf: alloc::vec::Vec<u8> = alloc::vec![0u8; len];
    let written = crate::platform_channel::recv_into(&mut kbuf);
    let k = PLATFORM_RECV_LOG.fetch_add(1, Ordering::Relaxed);
    if k < 16 {
        log::info!(
            "[platform-recv] #{} pid={} req_buf_len={} wrote={}",
            k,
            crate::process::current_pid(),
            len,
            written
        );
    }
    if written == 0 {
        return 0;
    }
    // SAFETY: caller guarantees buf_ptr is valid in their VA space.
    unsafe {
        core::ptr::copy_nonoverlapping(kbuf.as_ptr(), buf_ptr as *mut u8, written);
    }
    written as i64
}

/// Record the engine's reply for a platform-channel message.
/// `arg0` = seq, `arg1` = data_ptr, `arg2` = data_len
fn sys_platform_msg_reply(seq: u64, data_ptr: u64, data_len: u64) -> i64 {
    static PLATFORM_REPLY_LOG: core::sync::atomic::AtomicU32 =
        core::sync::atomic::AtomicU32::new(0);
    let payload = match unsafe { read_user_bytes(data_ptr, data_len as usize) } {
        Some(b) => b,
        None => return -14,
    };
    let k = PLATFORM_REPLY_LOG.fetch_add(1, Ordering::Relaxed);
    if k < 16 {
        log::info!(
            "[platform-reply] #{} pid={} seq={} payload_len={}",
            k,
            crate::process::current_pid(),
            seq,
            payload.len()
        );
    }
    match crate::platform_channel::reply(seq, payload) {
        Ok(()) => 0,
        Err(_) => -3, // ESRCH
    }
}

/// Copy a reply (previously set via sys_platform_msg_reply) into buf and
/// remove the message from the queue.  Returns byte count or 0 if not ready.
/// `arg0` = seq, `arg1` = buf_ptr, `arg2` = buf_len
fn sys_platform_msg_ack(seq: u64, buf_ptr: u64, buf_len: u64) -> i64 {
    static PLATFORM_ACK_LOG: core::sync::atomic::AtomicU32 =
        core::sync::atomic::AtomicU32::new(0);
    if buf_ptr == 0 || buf_len == 0 {
        return -14;
    }
    let len = buf_len as usize;
    let mut kbuf: alloc::vec::Vec<u8> = alloc::vec![0u8; len];
    let n = crate::platform_channel::ack(seq, &mut kbuf);
    let k = PLATFORM_ACK_LOG.fetch_add(1, Ordering::Relaxed);
    if k < 16 {
        log::info!(
            "[platform-ack] #{} pid={} seq={} req_buf_len={} wrote={}",
            k,
            crate::process::current_pid(),
            seq,
            len,
            n
        );
    }
    if n == 0 {
        return 0;
    }
    unsafe {
        core::ptr::copy_nonoverlapping(kbuf.as_ptr(), buf_ptr as *mut u8, n);
    }
    n as i64
}

// ── Phase 32-D: Stride-aware GPU blit ────────────────────────────────────────

/// Stride-aware GPU submit. `arg0` = surface_id, `arg1` = pixel_ptr,
/// `arg2` = row_bytes (bytes per scanline; 0 = tight-packed).
fn sys_gpu_submit_strided(surface_id: u64, pixel_ptr: u64, row_bytes: u64) -> i64 {
    static GPU_SUBMIT_STRIDED_LOG: core::sync::atomic::AtomicU32 =
        core::sync::atomic::AtomicU32::new(0);
    let n = GPU_SUBMIT_STRIDED_LOG.fetch_add(1, Ordering::Relaxed);
    if n < 128 {
        log::warn!(
            "[frame-boundary] gpu_submit_strided #{} pid={} sid={} ptr={:#x} row_bytes={}",
            n,
            crate::process::current_pid(),
            surface_id,
            pixel_ptr,
            row_bytes
        );
    }
    // Derive total byte length from the surface dimensions.
    let (width, height) = {
        let packed = crate::compositor::framebuffer_size_packed();
        // Surface dimensions are not in the framebuffer size; use compositor.
        // We pass row_bytes=0 to compute length inside gpu_submit_strided_for.
        let _ = packed;
        (0usize, 0usize) // sentinel: let compositor query its own table
    };
    let _ = (width, height); // compositor will validate internally
    // Estimate buffer length: if row_bytes == 0 we don't know, so read conservatively.
    // The compositor validates the exact size against the surface dimensions.
    let buf_len = if row_bytes == 0 {
        0x800000usize // 8 MiB upper bound — compositor will reject if wrong
    } else {
        // We need height from compositor; pass a generous estimate.
        (row_bytes as usize).saturating_mul(4096) // up to 4096 rows
    };
    let bytes = match unsafe { read_user_bytes(pixel_ptr, buf_len) } {
        Some(b) => b,
        None => return -14,
    };
    let pid = wm_consumer_pid();
    match crate::compositor::gpu_submit_strided_for(pid, surface_id as u32, bytes, row_bytes as usize) {
        Ok(()) => 0,
        Err("bad payload size") => -22,
        Err("permission denied") => -1,
        Err("no such surface") => -3,
        Err(_) => -12,
    }
}

// ── Phase 33-A: vsync rate control ───────────────────────────────────────────

/// Set the hardware vsync rate.  `hz` must be 1–240.
/// Accepted values: 30, 60, 90, 120, 144, 240.  Any value in range is stored.
fn sys_vsync_set_hz(hz: u64) -> i64 {
    let hz = (hz as u32).clamp(1, 240);
    crate::arch::apic::set_vsync_hz(hz);
    0
}

// ── Phase 34-C: Dart AOT snapshot loader ─────────────────────────────────────

/// Load a Dart AOT snapshot from the VFS into the calling process's address
/// space and return its mapped VA via `out_va_ptr`.
///
/// Accepts ELF-wrapped AOT snapshots (`\x7fELF`) and raw Dart snapshots
/// (magic `\xDC\xDC\xDC\xDC`).  The data is copied into fresh anonymous
/// pages mapped PROT_READ|PROT_EXEC.
///
/// `arg0` = path_ptr, `arg1` = path_len,
/// `arg2` = out_va_ptr (`*mut u64`), `arg3` = out_size_ptr (`*mut u64`)
fn sys_aot_snapshot_load(
    path_ptr:     u64,
    path_len:     u64,
    out_va_ptr:   u64,
    out_size_ptr: u64,
) -> i64 {
    let bytes = match unsafe { read_user_bytes(path_ptr, path_len as usize) } {
        Some(b) => b,
        None => return -14, // EFAULT
    };
    let path = match core::str::from_utf8(bytes) {
        Ok(s) => s,
        Err(_) => return -22, // EINVAL
    };
    let data = match crate::fs::lookup(path) {
        Some(d) => d,
        None => return -2, // ENOENT
    };
    // Validate: accept ELF magic (AOT shared objects) or raw Dart snapshot
    // magic. Flutter JIT VM/isolate snapshots start with F5 F5 DC DC (the
    // little-endian Snapshot::kMagicValue = 0xDCDCF5F5); some older variants
    // use DC DC DC DC. Accept either to keep this loader format-agnostic.
    let valid = data.len() >= 4
        && (&data[..4] == b"\x7fELF"
            || data[..4] == [0xF5, 0xF5, 0xDC, 0xDC]
            || data[..4] == [0xDC, 0xDC, 0xDC, 0xDC]);
    if !valid {
        return -22; // EINVAL — not a recognised snapshot format
    }
    let pid = crate::process::current_pid();
    if pid == 0 { return -1; } // EPERM
    let pml4_phys = match crate::process::get_user_context(pid) {
        Some(ctx) => ctx.pml4_phys,
        None => return -9, // EBADF
    };
    let pages = (data.len() + 4095) / 4096;
    // Map R|W|X so we can copy the file bytes in; Dart AOT instructions need
    // execute, and BSS-style sections within the AOT image may need write
    // access at engine init time. Userspace cannot exploit this — the mapping
    // is owned by the process and contains only the AOT blob.
    let va = crate::process::dl::mmap_anon(pid, pml4_phys, 0, pages, 7); // R|W|X
    if va == u64::MAX { return -12; } // ENOMEM
    unsafe {
        core::ptr::copy_nonoverlapping(data.as_ptr(), va as *mut u8, data.len());
    }
    if out_va_ptr != 0 {
        unsafe { core::ptr::write(out_va_ptr as *mut u64, va); }
    }
    if out_size_ptr != 0 {
        unsafe { core::ptr::write(out_size_ptr as *mut u64, data.len() as u64); }
    }
    0
}

// ── Phase 35: Dart isolate lifecycle ─────────────────────────────────────────

/// Spawn a Dart isolate in the calling process's AOT snapshot.
///
/// `arg0` = aot_va, `arg1` = aot_size, `arg2` = entry_offset (0 = root isolate),
/// `arg3` = stack_size (0 → default 128 KiB).
/// Returns new isolate ID (>0) on success, or negative errno.
fn sys_isolate_spawn(aot_va: u64, aot_size: u64, entry_offset: u64, stack_size: u64) -> i64 {
    let pid = crate::process::current_pid();
    if pid == 0 { return -1; } // EPERM
    match crate::isolate::spawn(pid, aot_va, aot_size, entry_offset, stack_size as usize) {
        Ok(id) => id as i64,
        Err("spawn_thread failed") => -12, // ENOMEM
        Err("isolate table full")  => -11, // EAGAIN
        Err("invalid aot_va")      => -22, // EINVAL
        Err(_)                     => -22,
    }
}

/// Kill a Dart isolate and free its table slot.
/// `arg0` = isolate_id.
fn sys_isolate_kill(id: u64) -> i64 {
    match crate::isolate::kill(id as u32) {
        Ok(())               => 0,
        Err("no such isolate") => -3, // ESRCH
        Err(_)               => -1,
    }
}

/// Query or change an isolate's state.
///
/// `arg0` = isolate_id, `arg1` = op (0=get, 1=pause, 2=resume).
/// Returns state value (1/2/3) for op=0, or 0 for op=1/2.
fn sys_isolate_ctrl(id: u64, op: u64) -> i64 {
    match crate::isolate::ctrl(id as u32, op as u32) {
        Ok(v)                  => v as i64,
        Err("no such isolate") => -3,  // ESRCH
        Err("unknown op")      => -22, // EINVAL
        Err(_)                 => -1,
    }
}

// ── Phase 36: Dart isolate message passing ────────────────────────────────────────

/// Send a message to a destination isolate's inbox.
///
/// `arg0` = dst_isolate_id, `arg1` = data_ptr, `arg2` = data_len.
/// Returns 0 on success or negative errno.
fn sys_isolate_msg_send(dst_id: u64, data_ptr: u64, data_len: u64) -> i64 {
    if data_len as usize > crate::isolate_msg::MAX_MSG_SIZE {
        return -27; // EFBIG
    }
    let data = match unsafe { read_user_bytes(data_ptr, data_len as usize) } {
        Some(b) => b,
        None => return -14, // EFAULT
    };
    match crate::isolate_msg::send(dst_id as u32, data) {
        Ok(())                     => 0,
        Err("message too large")   => -27, // EFBIG
        Err("no inbox for isolate") => -3,  // ESRCH
        Err(_)                     => -1,
    }
}

/// Copy the next message for `isolate_id` from its inbox into a user buffer.
///
/// `arg0` = isolate_id, `arg1` = buf_ptr, `arg2` = buf_len.
/// Returns bytes written (>0), 0 if inbox empty, or negative errno.
fn sys_isolate_msg_recv(isolate_id: u64, buf_ptr: u64, buf_len: u64) -> i64 {
    if buf_ptr == 0 || buf_len == 0 {
        return -14; // EFAULT
    }
    let cap = (buf_len as usize).min(crate::isolate_msg::MAX_MSG_SIZE);
    let mut kbuf: alloc::vec::Vec<u8> = alloc::vec![0u8; cap];
    let n = crate::isolate_msg::recv(isolate_id as u32, &mut kbuf);
    if n == 0 {
        return 0; // EAGAIN: no messages
    }
    if unsafe { write_user_bytes(buf_ptr, &kbuf[..n]) } {
        n as i64
    } else {
        -14 // EFAULT
    }
}

/// Return the number of pending messages in `isolate_id`'s inbox.
/// `arg0` = isolate_id.  Returns count (≥0) or -3 (ESRCH) if unknown isolate.
fn sys_isolate_msg_pending(isolate_id: u64) -> i64 {
    crate::isolate_msg::pending(isolate_id as u32) as i64
}

// ── Phase 37 — PS/2 input device query ────────────────────────────────────────

/// Return the number of detected input devices (keyboard + mouse).
fn sys_input_dev_count() -> i64 {
    crate::drivers::ps2::device_count() as i64
}

/// Return packed device descriptor for device index `n` (0-based).
/// Bit layout: bits[3:0]=type(1=kbd,2=mouse), bits[11:4]=IRQ, bits[15:12]=iface(0=PS/2).
/// Returns 0 if `n` is out of range.
fn sys_input_dev_info(n: u64) -> i64 {
    crate::drivers::ps2::device_info_packed(n as u32) as i64
}

// ── Phase 38 — .oscapp app registry ──────────────────────────────────────────

/// Install a `.oscapp` bundle provided by the caller.
/// `arg0` = bundle_ptr, `arg1` = bundle_len, `arg2` = id_out_ptr (u32le).
fn sys_app_install(bundle_ptr: u64, bundle_len: u64, id_out_ptr: u64) -> i64 {
    let bytes = match unsafe { read_user_bytes(bundle_ptr, bundle_len as usize) } {
        Some(b) => b,
        None => return -14, // EFAULT
    };
    match crate::app_registry::install(bytes) {
        Some(app_id) => {
            if id_out_ptr != 0 {
                unsafe {
                    core::ptr::write_unaligned(id_out_ptr as *mut u32, app_id);
                }
            }
            0
        }
        None => -22, // EINVAL — bad bundle or table full
    }
}

/// Serialise the installed app list into a user buffer.
/// Each entry is 88 bytes: [id: u32le][name: u8×64][version: u8×16][aot_len: u32le].
/// Returns the total number of installed apps (may exceed buf capacity).
fn sys_app_list(buf_ptr: u64, buf_len: u64) -> i64 {
    if buf_ptr == 0 || buf_len == 0 {
        return crate::app_registry::count() as i64;
    }
    if buf_len > 0x20_0000 { return -22; } // EINVAL
    let buf = unsafe { core::slice::from_raw_parts_mut(buf_ptr as *mut u8, buf_len as usize) };
    crate::app_registry::list(buf) as i64
}

/// Launch an installed app.
/// `arg0` = app_id, `arg1` = flags (reserved, pass 0).
/// Returns the new isolate_id, or -ERRNO.
fn sys_app_launch(app_id: u64, flags: u64) -> i64 {
    crate::app_registry::launch(app_id as u32, flags as u32)
}

/// Uninstall an installed app by `app_id`.
fn sys_app_uninstall(app_id: u64) -> i64 {
    if crate::app_registry::uninstall(app_id as u32) { 0 } else { -2 } // ENOENT
}

// ── Phase 39 — Named port IPC namespace ──────────────────────────────────────

/// Bind the calling process's isolate under `name`.
/// `arg0` = name_ptr, `arg1` = name_len, `arg2` = isolate_id.
fn sys_port_bind(name_ptr: u64, name_len: u64, isolate_id: u64) -> i64 {
    let name = match unsafe { read_user_bytes(name_ptr, name_len as usize) } {
        Some(b) => b,
        None => return -14, // EFAULT
    };
    let pid = crate::process::current_pid();
    crate::port_ns::bind(name, isolate_id as u32, pid)
}

/// Look up a named port.
/// `arg0` = name_ptr, `arg1` = name_len, `arg2` = iso_id_out_ptr (u32le), `arg3` = pid_out_ptr (u32le).
fn sys_port_lookup(name_ptr: u64, name_len: u64, iso_out: u64, pid_out: u64) -> i64 {
    let name = match unsafe { read_user_bytes(name_ptr, name_len as usize) } {
        Some(b) => b,
        None => return -14, // EFAULT
    };
    let mut iso = 0u32;
    let mut pid = 0u32;
    let rc = crate::port_ns::lookup(name, &mut iso, &mut pid);
    if rc == 0 {
        if iso_out != 0 {
            unsafe { core::ptr::write_unaligned(iso_out as *mut u32, iso); }
        }
        if pid_out != 0 {
            unsafe { core::ptr::write_unaligned(pid_out as *mut u32, pid); }
        }
    }
    rc
}

/// Unbind a named port.
fn sys_port_unbind(name_ptr: u64, name_len: u64) -> i64 {
    let name = match unsafe { read_user_bytes(name_ptr, name_len as usize) } {
        Some(b) => b,
        None => return -14, // EFAULT
    };
    crate::port_ns::unbind(name)
}

// ── Phase 41 — USB host-controller query ─────────────────────────────────────

fn sys_usb_controller_count() -> i64 {
    crate::drivers::usb::xhci_count() as i64
}

// ── Phase 42 — Userspace framebuffer map + WM event dequeue ─────────────────

/// Map the framebuffer into the calling process's address space.
///
/// Writes a 24-byte FbInfo struct to `info_out_ptr`:
///   u64 addr        – user VA of the mapped framebuffer
///   u32 width       – pixels wide
///   u32 height      – pixels tall
///   u32 pitch       – bytes per row
///   u32 bpp         – bits per pixel (always 32)
fn sys_fb_map(info_out_ptr: u64) -> i64 {
    let (hhdm_va, width, height, pitch_bytes) = match crate::drivers::fb::fb_info() {
        Some(i) => i,
        None    => {
            log::warn!("[sys_fb_map] fb_info() returned None");
            return -6; // ENXIO — framebuffer not ready
        }
    };

    let hhdm_off = crate::mm::frame_allocator::hhdm_offset();
    let fb_phys  = hhdm_va - hhdm_off;

    let pid = crate::process::current_pid();
    let pml4_phys = match crate::process::get_user_context(pid) {
        Some(ctx) => ctx.pml4_phys,
        None      => {
            log::warn!("[sys_fb_map] get_user_context({}) returned None", pid);
            return -9; // EBADF
        }
    };
    log::debug!("[sys_fb_map] pid={} fb={}x{} pitch={} phys={:#x}", pid, width, height, pitch_bytes, fb_phys);

    // Fixed user VA for the framebuffer (1 GiB mark — above typical ELF load).
    const FB_USER_BASE: u64 = 0x4000_0000;
    let fb_size    = height as u64 * pitch_bytes as u64;
    let page_count = (fb_size + 0xFFF) / 0x1000;

    for i in 0..page_count {
        let phys_page = fb_phys + i * 0x1000;
        let user_va   = FB_USER_BASE + i * 0x1000;
        if unsafe { crate::mm::paging::map_user_page(pml4_phys, user_va, phys_page) }.is_err() {
            return -12; // ENOMEM
        }
    }

    // Write FbInfo to user buffer (byte-by-byte to avoid alignment UB).
    unsafe {
        let dst = info_out_ptr as *mut u8;
        let user_addr: u64 = FB_USER_BASE;
        dst.add( 0).cast::<[u8;8]>().write_unaligned(user_addr.to_le_bytes());
        dst.add( 8).cast::<[u8;4]>().write_unaligned(width.to_le_bytes());
        dst.add(12).cast::<[u8;4]>().write_unaligned(height.to_le_bytes());
        dst.add(16).cast::<[u8;4]>().write_unaligned(pitch_bytes.to_le_bytes());
        dst.add(20).cast::<[u8;4]>().write_unaligned(32u32.to_le_bytes());
    }
    // Disable early boot framebuffer logging during userspace display ownership
    crate::drivers::fb::disable_fb_logging();
    // Signal compositor that a process now owns the FB directly.
    crate::compositor::set_fb_bypass(true);
    0
}

/// Pop the next WM event for the calling process.
///
/// Writes a 32-byte WmEvent to `event_out_ptr` on success (returns 0).
/// Returns -11 (EAGAIN) if no events are pending.
fn sys_wm_next_event(event_out_ptr: u64) -> i64 {
    let pid = crate::process::current_pid();
    match crate::wm::pop_event_for(pid) {
        Some(ev) => {
            // WmEvent is #[repr(C)] — copy 32 bytes to user buffer.
            unsafe {
                let src = &ev as *const crate::embedder::abi::WmEvent as *const u8;
                core::ptr::copy_nonoverlapping(src, event_out_ptr as *mut u8, 32);
            }
            0
        }
        None => -11, // EAGAIN
    }
}

// ── Phase 43 — VFS query API (no open-fd) ────────────────────────────────────

/// List files whose path starts with `path`, writing "name\n" per entry.
/// `arg0` = path_ptr, `arg1` = path_len, `arg2` = buf_ptr, `arg3` = buf_len.
/// Returns total bytes written.
fn sys_vfs_list(path_ptr: u64, path_len: u64, buf_ptr: u64, buf_len: u64) -> i64 {
    let pb = match unsafe { read_user_bytes(path_ptr, path_len as usize) } {
        Some(b) => b,
        None => return -14,
    };
    let path = match core::str::from_utf8(pb) {
        Ok(s) => s,
        Err(_) => return -22,
    };
    if buf_ptr == 0 || buf_len == 0 { return -14; }
    let n = crate::fs::list_prefix(path, buf_ptr as *mut u8, buf_len as usize);
    n as i64
}

/// One-shot VFS file read without an fd.
/// `arg0` = path_ptr, `arg1` = path_len, `arg2` = buf_ptr, `arg3` = buf_len.
/// Returns bytes copied.
fn sys_vfs_read(path_ptr: u64, path_len: u64, buf_ptr: u64, buf_len: u64) -> i64 {
    let pb = match unsafe { read_user_bytes(path_ptr, path_len as usize) } {
        Some(b) => b,
        None => return -14,
    };
    let path = match core::str::from_utf8(pb) {
        Ok(s) => s,
        Err(_) => return -22,
    };
    let data = match crate::fs::lookup(path) {
        Some(d) => d,
        None => return -2, // ENOENT
    };
    if buf_ptr == 0 { return -14; }
    let copy_len = (buf_len as usize).min(data.len());
    unsafe { core::ptr::copy_nonoverlapping(data.as_ptr(), buf_ptr as *mut u8, copy_len); }
    copy_len as i64
}

// ── Phase 44 — writable ramdisk (/tmp/) ──────────────────────────────────────

/// Write data to `/tmp/<name>`.
/// `arg0` = path_ptr, `arg1` = path_len, `arg2` = data_ptr, `arg3` = data_len.
fn sys_vfs_write(path_ptr: u64, path_len: u64, data_ptr: u64, data_len: u64) -> i64 {
    let pb = match unsafe { read_user_bytes(path_ptr, path_len as usize) } {
        Some(b) => b,
        None => return -14,
    };
    let path = match core::str::from_utf8(pb) {
        Ok(s) => s,
        Err(_) => return -22,
    };
    let data = match unsafe { read_user_bytes(data_ptr, data_len as usize) } {
        Some(b) => b,
        None => return -14,
    };
    match crate::fs::write(path, data) {
        Ok(()) => 0,
        Err("not under /tmp/")        => -13, // EACCES
        Err("ramdisk full")           => -28, // ENOSPC
        Err("ramdisk slot table full")=> -28,
        Err("path too long")          => -36, // ENAMETOOLONG
        Err(_)                        => -1,
    }
}

/// Stat a VFS path — writes `u64` file size to `size_out_ptr`.
/// Returns 0 on success, -ENOENT if not found.
fn sys_vfs_stat(path_ptr: u64, path_len: u64, size_out_ptr: u64) -> i64 {
    let pb = match unsafe { read_user_bytes(path_ptr, path_len as usize) } {
        Some(b) => b,
        None => return -14,
    };
    let path = match core::str::from_utf8(pb) {
        Ok(s) => s,
        Err(_) => return -22,
    };
    match crate::fs::stat(path) {
        Some(sz) => {
            if size_out_ptr != 0 {
                unsafe { core::ptr::write_unaligned(size_out_ptr as *mut u64, sz); }
            }
            0
        }
        None => -2, // ENOENT
    }
}

// ── Phase 45 — virtio-net networking ─────────────────────────────────────────

/// Write virtio-net info (MAC, IP) as text into user buffer.
/// Returns bytes written.
fn sys_net_info(buf_ptr: u64, buf_len: u64) -> i64 {
    if buf_ptr == 0 || buf_len == 0 { return -14; }
    let len = (buf_len as usize).min(256);
    let mut kbuf = [0u8; 256];
    let n = crate::drivers::virtio_net::info_text(&mut kbuf[..len]);
    unsafe { core::ptr::copy_nonoverlapping(kbuf.as_ptr(), buf_ptr as *mut u8, n); }
    n as i64
}

/// Enqueue a raw UDP-like payload as an Ethernet frame (stub).
/// `arg0` = dst_ip (u32 BE), `arg1` = dst_port (u16), `arg2` = data_ptr, `arg3` = data_len.
fn sys_net_send(dst_ip: u64, _dst_port: u64, data_ptr: u64, data_len: u64) -> i64 {
    if !crate::drivers::virtio_net::is_ready() { return -6; } // ENXIO
    let data = match unsafe { read_user_bytes(data_ptr, (data_len as usize).min(1514)) } {
        Some(b) => b,
        None => return -14,
    };
    // Build a minimal Ethernet frame (broadcast dst, random src).
    let mut frame = [0u8; 1514 + 14];
    frame[0..6].copy_from_slice(&[0xFF; 6]); // dst MAC: broadcast
    frame[6..12].copy_from_slice(&[0x52, 0x54, 0x00, 0x12, 0x34, 0x56]); // src MAC: QEMU default
    frame[12] = 0x08; frame[13] = 0x00; // EtherType IPv4
    let payload_end = 14 + data.len().min(1500);
    frame[14..payload_end].copy_from_slice(&data[..data.len().min(1500)]);
    match crate::drivers::virtio_net::send_raw(&frame[..payload_end]) {
        Ok(()) => 0,
        Err(_) => -11, // EAGAIN
    }
}

/// Poll for a received raw packet.
/// `arg0` = buf_ptr, `arg1` = buf_len, `arg2` = src_ip_out (*mut u32), `arg3` = src_port_out (*mut u16).
fn sys_net_recv(buf_ptr: u64, buf_len: u64, _src_ip_out: u64, _src_port_out: u64) -> i64 {
    if buf_ptr == 0 || buf_len == 0 { return -14; }
    let mut kbuf = [0u8; 1514 + 14];
    match crate::drivers::virtio_net::recv_raw(&mut kbuf) {
        Some(n) => {
            let copy = n.min(buf_len as usize);
            unsafe { core::ptr::copy_nonoverlapping(kbuf.as_ptr(), buf_ptr as *mut u8, copy); }
            copy as i64
        }
        None => -11, // EAGAIN
    }
}

// ── Phase 46 — compositor bypass + full-screen surface ───────────────────────

/// Release the direct-FB mapping so the compositor can resume rendering.
fn sys_fb_release() -> i64 {
    crate::compositor::set_fb_bypass(false);
    0
}

/// Create a surface that exactly covers the framebuffer.
/// Returns the new surface id, or negative errno.
fn sys_surface_fullscreen() -> i64 {
    let (w, h) = crate::drivers::fb::size_px().unwrap_or((0, 0));
    if w == 0 { return -6; } // ENXIO
    let pid = wm_consumer_pid();
    match crate::compositor::create_surface_for(pid, w, h) {
        Ok(id) => {
            let _ = crate::compositor::move_surface_for(pid, id, 0, 0, 0);
            id as i64
        }
        Err(_) => -12, // ENOMEM
    }
}

// ── Phase 48 — UART 16550 serial console ─────────────────────────────────────

/// Read up to `buf_len` bytes from COM1 into user buffer.
/// Returns bytes read, or -11 (EAGAIN) if no data is available.
fn sys_serial_read(buf_ptr: u64, buf_len: u64) -> i64 {
    if buf_ptr == 0 { return -14; } // EFAULT
    let cap = (buf_len as usize).min(256);
    let mut count = 0usize;
    while count < cap {
        match crate::drivers::uart::read_byte_nonblocking() {
            Some(b) => {
                unsafe { (buf_ptr as *mut u8).add(count).write_volatile(b); }
                count += 1;
            }
            None => break,
        }
    }
    if count == 0 { -11 } else { count as i64 }
}

/// Write `buf_len` bytes from user buffer to COM1.
fn sys_serial_write(buf_ptr: u64, buf_len: u64) -> i64 {
    let bytes = match unsafe { read_user_bytes(buf_ptr, (buf_len as usize).min(4096)) } {
        Some(b) => b,
        None => return -14,
    };
    crate::drivers::uart::write_bytes(bytes);
    bytes.len() as i64
}

// ── Phase 49 — virtio-blk block device ───────────────────────────────────────

/// Write device info into user buffer.
/// Returns bytes written.  `arg0` = buf_ptr (64-byte minimum recommended).
fn sys_blk_info(buf_ptr: u64) -> i64 {
    if buf_ptr == 0 { return -14; }
    let mut tmp = [0u8; 128];
    let n = crate::drivers::virtio_blk::info_text(&mut tmp);
    if n == 0 { return 0; }
    unsafe {
        core::ptr::copy_nonoverlapping(tmp.as_ptr(), buf_ptr as *mut u8, n);
    }
    n as i64
}

/// Read sectors from the block device into a user buffer.
/// arg0=sector, arg1=count, arg2=buf_ptr, arg3=buf_len
fn sys_blk_read(sector: u64, count: u64, buf_ptr: u64, buf_len: u64) -> i64 {
    if buf_ptr == 0 { return -14; }
    let needed = (count * 512) as usize;
    if buf_len < needed as u64 { return -22; } // EINVAL
    if needed > 1 << 20 { return -22; } // cap at 1 MiB
    let buf = unsafe { core::slice::from_raw_parts_mut(buf_ptr as *mut u8, needed) };
    match crate::drivers::virtio_blk::read_sectors(sector, count, buf) {
        Ok(()) => needed as i64,
        Err(_)  => -5, // EIO
    }
}

/// Write sectors to the block device from a user buffer.
fn sys_blk_write(sector: u64, count: u64, buf_ptr: u64, buf_len: u64) -> i64 {
    let needed = (count * 512) as usize;
    if buf_len < needed as u64 { return -22; }
    let data = match unsafe { read_user_bytes(buf_ptr, needed) } {
        Some(b) => b,
        None => return -14,
    };
    match crate::drivers::virtio_blk::write_sectors(sector, count, data) {
        Ok(()) => needed as i64,
        Err(_)  => -5,
    }
}

// ── Phase 50 — smoltcp TCP/IP ────────────────────────────────────────────────

/// Open a TCP connection.  arg0 = dst_ip (BE u32), arg1 = dst_port.
/// Returns socket fd ≥ 0 on success, or negative errno.
fn sys_tcp_connect(dst_ip: u64, dst_port: u64) -> i64 {
    match crate::net::tcp::tcp_connect(dst_ip as u32, dst_port as u16) {
        Ok(fd)   => fd as i64,
        Err(e)   => e,
    }
}

/// Write to a TCP socket.  arg0=fd, arg1=buf_ptr, arg2=buf_len.
fn sys_tcp_write(fd: u64, buf_ptr: u64, buf_len: u64) -> i64 {
    let data = match unsafe { read_user_bytes(buf_ptr, (buf_len as usize).min(65536)) } {
        Some(b) => b,
        None => return -14,
    };
    match crate::net::tcp::tcp_write(fd as usize, data) {
        Ok(n)  => n as i64,
        Err(e) => e,
    }
}

/// Read from a TCP socket.  arg0=fd, arg1=buf_ptr, arg2=buf_len.
fn sys_tcp_read(fd: u64, buf_ptr: u64, buf_len: u64) -> i64 {
    if buf_ptr == 0 { return -14; }
    let cap = (buf_len as usize).min(65536);
    let buf = unsafe { core::slice::from_raw_parts_mut(buf_ptr as *mut u8, cap) };
    match crate::net::tcp::tcp_read(fd as usize, buf) {
        Ok(n)  => n as i64,
        Err(e) => e,
    }
}

/// Close a TCP socket.
fn sys_tcp_close(fd: u64) -> i64 {
    match crate::net::tcp::tcp_close(fd as usize) {
        Ok(())  => 0,
        Err(e)  => e,
    }
}

/// Trigger DHCP DISCOVER.  Returns assigned IP as BE u32, or 0 on failure.
fn sys_dhcp_discover() -> i64 {
    crate::net::tcp::dhcp_discover() as i64
}

// ── Phase 51: ext2 read-only filesystem ──────────────────────────────────────

fn sys_ext2_mount() -> i64 {
    match crate::fs::ext2::mount() { Ok(()) => 0, Err(_) => -5 }
}

fn sys_ext2_ls(path_ptr: u64, path_len: u64, out_ptr: u64, out_len: u64) -> i64 {
    if path_ptr == 0 || out_ptr == 0 { return -14; }
    let path_bytes = match unsafe { read_user_bytes(path_ptr, path_len as usize) } {
        Some(b) => b, None => return -14,
    };
    let out = unsafe { core::slice::from_raw_parts_mut(out_ptr as *mut u8, out_len as usize) };
    match crate::fs::ext2::ls(path_bytes, out) { Ok(n) => n as i64, Err(_) => -2 }
}

fn sys_ext2_read(path_ptr: u64, path_len: u64, out_ptr: u64, out_len: u64) -> i64 {
    if path_ptr == 0 || out_ptr == 0 { return -14; }
    let path_bytes = match unsafe { read_user_bytes(path_ptr, path_len as usize) } {
        Some(b) => b, None => return -14,
    };
    let out = unsafe { core::slice::from_raw_parts_mut(out_ptr as *mut u8, out_len as usize) };
    match crate::fs::ext2::read_file(path_bytes, out) { Ok(n) => n as i64, Err(_) => -2 }
}

// ── Phase 53: scheduler extras ───────────────────────────────────────────────

fn sys_sched_yield() -> i64 {
    let cur = crate::process::current_pid();
    if let Some(next) = crate::process::next_runnable_pid(cur) {
        if next != cur {
            crate::process::set_state(cur, crate::process::ProcState::Running);
            crate::process::set_current_pid(next);
            // Context switch will happen on next APIC timer or we do it inline.
        }
    }
    0
}

fn sys_get_cpu_time(pid: u64) -> i64 {
    let target = if pid == 0 { crate::process::current_pid() } else { pid as u32 };
    crate::process::get_cpu_ticks(target) as i64
}

// ── Phase 54: fork ────────────────────────────────────────────────────────────

fn sys_fork() -> i64 {
    match crate::process::fork_current() {
        Ok(child_pid) => child_pid as i64,
        Err(_) => -12, // ENOMEM
    }
}

// ── Phase 55: signals ─────────────────────────────────────────────────────────

fn sys_kill_signal(target_pid: u64, sig: u64) -> i64 {
    if sig > 31 { return -22; } // EINVAL
    crate::process::raise_signal(target_pid as u32, sig as u8);
    0
}

fn sys_sigaction(sig: u64, handler_ptr: u64) -> i64 {
    if sig == 0 || sig > 31 { return -22; }
    let pid = crate::process::current_pid();
    crate::process::set_signal_handler(pid, sig as u8, handler_ptr);
    0
}

fn sys_sigreturn() -> i64 {
    // Signal return — the user stack should have saved context; for now just
    // set running state and return 0 (real implementation needs the signal
    // frame layout to match the signal delivery code).
    0
}

// ── Phase 59: NVMe driver ─────────────────────────────────────────────────────

fn sys_nvme_info(buf_ptr: u64, buf_len: u64) -> i64 {
    if buf_ptr == 0 { return -14; }
    let out = unsafe { core::slice::from_raw_parts_mut(buf_ptr as *mut u8, buf_len as usize) };
    crate::drivers::nvme::info_text(out) as i64
}

fn sys_nvme_read(lba: u64, count: u64, buf_ptr: u64, buf_len: u64) -> i64 {
    if buf_ptr == 0 { return -14; }
    if buf_len < count * 512 { return -22; }
    let buf = unsafe { core::slice::from_raw_parts_mut(buf_ptr as *mut u8, buf_len as usize) };
    match crate::drivers::nvme::read_sectors(lba, count as u32, buf) {
        Ok(()) => count as i64 * 512,
        Err(_) => -5,
    }
}

fn sys_nvme_write(lba: u64, count: u64, buf_ptr: u64, buf_len: u64) -> i64 {
    if buf_ptr == 0 { return -14; }
    if buf_len < count * 512 { return -22; }
    let buf = unsafe { core::slice::from_raw_parts(buf_ptr as *const u8, buf_len as usize) };
    match crate::drivers::nvme::write_sectors(lba, count as u32, buf) {
        Ok(()) => count as i64 * 512,
        Err(_) => -5,
    }
}

// ── Phase 62: futex ───────────────────────────────────────────────────────────
//
// Minimal Linux-compatible futex(2) subset for pthread / Dart VM support.
// Supported ops:
//   FUTEX_WAIT (0): sleep until *uaddr changes from val or a wake arrives.
//   FUTEX_WAKE (1): wake up to val waiters on uaddr.
// Returns 0 on success, negative errno on error.
// This keeps a tiny in-kernel waiter map so pthread condition variables and
// mutexes can park instead of burning CPU in a busy loop.

const FUTEX_WAIT: u32 = 0;
const FUTEX_WAKE: u32 = 1;

const FUTEX_ADDR_PID1_WAIT: u64 = 0x0000_7fff_fffe_c8f0;
const FUTEX_ADDR_WORKER_WAIT: u64 = 0x0000_0004_5700_0040;
const FUTEX_ADDR_HANDOFF: u64 = 0x0000_0003_3800_0070;

#[inline]
fn futex_addr_is_target(addr: u64) -> bool {
    matches!(
        addr,
        FUTEX_ADDR_PID1_WAIT | FUTEX_ADDR_WORKER_WAIT | FUTEX_ADDR_HANDOFF
    )
}

#[inline]
fn futex_target_waiter_count(addr: u64) -> usize {
    let table = FUTEX_WAITERS.lock();
    table.get(&addr).map_or(0, |w| w.len())
}

#[inline]
fn futex_trace_targets(tag: &str, caller: u32, addr: u64, val: u32) {
    static FUTEX_TARGET_LOG: core::sync::atomic::AtomicU32 =
        core::sync::atomic::AtomicU32::new(0);
    let n = FUTEX_TARGET_LOG.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
    if n >= 256 { return; }

    let c_pid1 = futex_target_waiter_count(FUTEX_ADDR_PID1_WAIT);
    let c_worker = futex_target_waiter_count(FUTEX_ADDR_WORKER_WAIT);
    let c_handoff = futex_target_waiter_count(FUTEX_ADDR_HANDOFF);
    log::warn!(
        "[futex-target] #{} {} caller={} addr={:#x} val={:#x} waiters(pid1={};worker={};handoff={})",
        n,
        tag,
        caller,
        addr,
        val,
        c_pid1,
        c_worker,
        c_handoff
    );
}

#[inline]
fn cond_miss_bridge(cond: u64, wake_count: u32) -> u32 {
    let mut total = 0u32;
    // Phase A: try the hardcoded "well-known" pid1/worker/handoff futex
    // addresses (kept for back-compat with earlier surgical fixes).
    for &addr in &[FUTEX_ADDR_PID1_WAIT, FUTEX_ADDR_WORKER_WAIT, FUTEX_ADDR_HANDOFF] {
        if addr == cond { continue; }
        let waiters = futex_target_waiter_count(addr);
        if waiters == 0 { continue; }
        let bridged = futex_wake_waiters(addr, wake_count);
        if bridged > 0 {
            total = total.saturating_add(bridged as u32);
            log::warn!(
                "[cond-bridge] cond={:#x} -> futex={:#x} wake_count={} bridged={}",
                cond,
                addr,
                wake_count,
                bridged
            );
        }
    }

    // Phase B (sibling fallback): if no hardcoded target had waiters, wake
    // every futex address that has at least one waiter belonging to the same
    // address-space group as the broadcaster. Flutter's actual cond addrs
    // (e.g. 0x56b000048) aren't in the hardcoded set, and its workers wait on
    // arbitrary mutex-internal seq futexes (0x338000070, 0x457000040, ...).
    // Without this fallback the engine deadlocks: pid 1 broadcasts forever
    // with woke=0 while pids 5/6/8 sit in futex_wait on unrelated addrs.
    if total == 0 {
        let pid = crate::process::current_pid();
        let siblings = crate::process::sibling_pids(pid);
        if siblings.len() > 1 {
            // Snapshot waiter table addresses.
            let addrs: alloc::vec::Vec<u64> = {
                let t = FUTEX_WAITERS.lock();
                t.iter()
                    .filter_map(|(addr, waiters)| {
                        if *addr == cond { return None; }
                        // Any waiter that is a sibling?
                        if waiters.iter().any(|w| siblings.contains(w)) {
                            Some(*addr)
                        } else {
                            None
                        }
                    })
                    .collect()
            };
            for addr in addrs {
                let bridged = futex_wake_waiters(addr, wake_count);
                if bridged > 0 {
                    total = total.saturating_add(bridged as u32);
                    log::info!(
                        "[cond-bridge-sib] cond={:#x} -> futex={:#x} wake_count={} bridged={}",
                        cond, addr, wake_count, bridged
                    );
                }
            }
        }
    }

    total
}

#[inline]
fn futex_pid1_postrun_bypass(uaddr: u64, val: u32) -> bool {
    if uaddr != FUTEX_ADDR_PID1_WAIT || val != 0 {
        return false;
    }
    let pid = crate::process::current_pid();
    if pid != 1 {
        return false;
    }

    // Force a generous number of pid1 wait-loop turns to fall through so the
    // engine startup path can reach at least one render tick. The original
    // 12-pass budget exhausted before frame submission; bump to 256 and only
    // log periodically.
    static PID1_WAIT_LOOP_BYPASS: core::sync::atomic::AtomicU32 =
        core::sync::atomic::AtomicU32::new(0);
    let n = PID1_WAIT_LOOP_BYPASS.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
    if n < 256 {
        if n < 16 || n % 32 == 0 {
            log::warn!(
                "[futex-postrun-bypass] pid=1 uaddr={:#x} val={} pass={}/256",
                uaddr, val, n + 1
            );
        }
        return true;
    }
    false
}

fn futex_waiter_add(addr: u64, pid: u32) {
    let mut table = FUTEX_WAITERS.lock();
    let waiters = table.entry(addr).or_insert_with(Vec::new);
    if !waiters.contains(&pid) {
        waiters.push(pid);
    }
}

fn futex_waiter_remove(addr: u64, pid: u32) {
    let mut table = FUTEX_WAITERS.lock();
    if let Some(waiters) = table.get_mut(&addr) {
        waiters.retain(|&waiter| waiter != pid);
        if waiters.is_empty() {
            table.remove(&addr);
        }
    }
}

fn futex_waiter_present(addr: u64, pid: u32) -> bool {
    let table = FUTEX_WAITERS.lock();
    table.get(&addr).map_or(false, |waiters| waiters.contains(&pid))
}

/// Brute-force unstick: wake one waiter on every recorded futex address.
/// Used by [pid1-wait-kick] to give every parked thread a chance to re-check
/// its predicate when pid 1 is about to park on its own cond. Returns the
/// total number of wakes performed.
fn futex_wake_all_known_waiters() -> u32 {
    let addrs: Vec<u64> = {
        let table = FUTEX_WAITERS.lock();
        table.keys().copied().collect()
    };
    let mut total = 0u32;
    for addr in addrs {
        let n = futex_wake_waiters(addr, u32::MAX);
        if n > 0 {
            total = total.saturating_add(n as u32);
        }
    }
    total
}

/// Record `count` pending wakes for `addr`. Capped at 64 so a stuck producer
/// can't unbounded-grow the table.
fn futex_pending_post(addr: u64, count: u32) {
    if count == 0 { return; }
    let mut t = FUTEX_PENDING_WAKES.lock();
    let e = t.entry(addr).or_insert(0);
    *e = e.saturating_add(count).min(64);
}

/// Consume one pending wake on `addr`. Returns true if a wake was pending.
fn futex_pending_take(addr: u64) -> bool {
    let mut t = FUTEX_PENDING_WAKES.lock();
    if let Some(e) = t.get_mut(&addr) {
        if *e > 0 {
            *e -= 1;
            if *e == 0 { t.remove(&addr); }
            return true;
        }
    }
    false
}

fn futex_wake_waiters(addr: u64, count: u32) -> i64 {
    let wake_list = {
        let mut table = FUTEX_WAITERS.lock();
        let Some(waiters) = table.get_mut(&addr) else {
            // No waiter parked. If this is a target address, remember the
            // wake so the next waiter can consume it (fixes wake-before-wait
            // race in cond-bridge path).
            if futex_addr_is_target(addr) {
                drop(table);
                futex_pending_post(addr, count);
            }
            return 0;
        };
        let wake_count = waiters.len().min(count as usize);
        let mut woke = Vec::with_capacity(wake_count);
        for _ in 0..wake_count {
            woke.push(waiters.remove(0));
        }
        if waiters.is_empty() {
            table.remove(&addr);
        }
        woke
    };

    let n = wake_list.len();
    if n > 0 {
        let wpid = crate::process::current_pid();
        log::trace!("[futex-wake] caller={} addr={:#x} woke={}", wpid, addr, n);
        if futex_addr_is_target(addr) {
            static FUTEX_WAKE_TARGET_LOG: core::sync::atomic::AtomicU32 =
                core::sync::atomic::AtomicU32::new(0);
            let k = FUTEX_WAKE_TARGET_LOG.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
            if k < 128 {
                for pid in wake_list.iter() {
                    log::warn!(
                        "[futex-target-wake] #{} caller={} addr={:#x} waking_pid={}",
                        k,
                        wpid,
                        addr,
                        pid
                    );
                }
            }
        }
    }

    for pid in &wake_list {
        // Best-effort: ensure waker target is marked Running so a scheduler
        // that has parked it can pick it up again.
        crate::process::set_state(*pid, crate::process::ProcState::Running);
    }

    n as i64
}

fn sys_futex(uaddr: u64, op: u32, val: u32) -> i64 {
    let op_base = op & 0x7F; // strip FUTEX_PRIVATE_FLAG etc.
    match op_base {
        FUTEX_WAIT => {
            if uaddr < 0x1000 || uaddr & 3 != 0 { return -22; } // EINVAL: invalid/unaligned
            let cur = unsafe { core::ptr::read_volatile(uaddr as *const u32) };
            if cur != val {
                return -11; // EAGAIN: value changed before we could sleep
            }

            let pid = crate::process::current_pid();
            if pid == 0 {
                return 0;
            }

            if futex_pid1_postrun_bypass(uaddr, val) {
                return 0;
            }

            // Consume any pending wake that arrived before we parked. This is
            // the proper fix for the cond-bridge wake-before-wait race. If a
            // wake is pending, return success immediately so the user code
            // can re-test its predicate.
            if futex_addr_is_target(uaddr) && futex_pending_take(uaddr) {
                static FUTEX_PENDING_CONSUME_LOG: core::sync::atomic::AtomicU32 =
                    core::sync::atomic::AtomicU32::new(0);
                let k = FUTEX_PENDING_CONSUME_LOG.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
                if k < 32 || k % 64 == 0 {
                    log::warn!(
                        "[futex-pending-consumed] pid={} addr={:#x} (wake arrived before wait)",
                        pid, uaddr
                    );
                }
                // DEADLOCK BREAKER: when a worker/Dart isolate is spinning
                // through pending-wake / re-test cycles, the task runners
                // (pid 2/3/4/7) may be parked in epoll_wait with no one to
                // arm their timerfds. Every 16 pending consumes, force-wake
                // every task runner so they get a chance to discover new
                // work the engine wants to post.
                if k % 16 == 0 {
                    let _ = force_wake_all_task_runners("pending-consume-pulse");
                }
                return 0;
            }

            // NOTE: previously we had a "deadlock-break" branch for both
            // PID1_WAIT and WORKER_WAIT that did yield+return-0 when the
            // counterparty was parked. That was a hot infinite spin: the
            // wait NEVER actually slept, user-space re-tested predicate
            // (still false), syscalled again, infinitely. The pending-wake
            // mechanism above + cond_miss_bridge handle this properly: when
            // pid 1 broadcasts a cond, the bridge posts pending wakes on
            // PID1_WAIT/WORKER_WAIT/HANDOFF so the next wait returns 0
            // immediately. Workers must park for real to consume those.

            if futex_addr_is_target(uaddr) {
                futex_trace_targets("wait-enter", pid, uaddr, val);
            }

            // Log which thread is blocking on which futex (helps find ServiceIsolate::StartUp)
            {
                let urip = crate::arch::syscall::user_rip();
                log::warn!("[futex-wait] pid={} uaddr={:#x} val={:#x} rip={:#x}", pid, uaddr, val, urip);
            }

            futex_waiter_add(uaddr, pid);
            if futex_addr_is_target(uaddr) {
                futex_trace_targets("wait-queued", pid, uaddr, val);
                if uaddr == FUTEX_ADDR_PID1_WAIT {
                    static PID1_QUEUE_SNAPSHOT_LOG: core::sync::atomic::AtomicU32 =
                        core::sync::atomic::AtomicU32::new(0);
                    let k = PID1_QUEUE_SNAPSHOT_LOG.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
                    if k < 8 {
                        log::warn!("[pid1-wait] queued on {:#x}; taking scheduler/syscall snapshot", uaddr);
                        crate::process::debug_dump_core_threads();
                        dump_recent_syscalls(12);
                    }
                }
            }

            // Try to cooperatively hand the CPU to a sibling user thread that
            // is runnable. This is the only path by which other threads of
            // PID 1 (e.g. the Flutter engine worker) can actually execute,
            // since the APIC timer ISR does not preempt user mode during
            // bring-up. We save our user return context (with RAX=0 so the
            // resumed syscall reports success), mark ourselves Blocked, and
            // SYSRET into the sibling. A subsequent FUTEX_WAKE will mark us
            // Running again and the next yielding thread will pick us up.
            if let Some(next) = crate::process::next_runnable_pid(pid) {
                if next != pid {
                    let urip = crate::arch::syscall::user_rip();
                    let ursp = crate::arch::syscall::user_rsp();
                    crate::process::save_return_context(pid, urip, ursp);
                    crate::process::save_full_user_gprs(pid);
                    crate::process::set_rax(pid, 0);
                    crate::process::save_xstate(pid);
                    crate::process::set_state(pid, crate::process::ProcState::Blocked);
                    crate::process::enter_user_by_pid_noreturn(next);
                }
            }

            // No sibling to switch into — sleep loop using sti; hlt; cli
            while unsafe { core::ptr::read_volatile(uaddr as *const u32) } == val
                && futex_waiter_present(uaddr, pid)
            {
                #[cfg(target_arch = "x86_64")]
                unsafe {
                    core::arch::asm!("sti; hlt; cli", options(nomem, nostack));
                }
            }

            futex_waiter_remove(uaddr, pid);
            if futex_addr_is_target(uaddr) {
                futex_trace_targets("wait-exit", pid, uaddr, val);
            }
            0
        }
        FUTEX_WAKE => {
            if futex_addr_is_target(uaddr) {
                let pid = crate::process::current_pid();
                futex_trace_targets("wake-enter", pid, uaddr, val);
            }
            let n = futex_wake_waiters(uaddr, val);
            let wpid = crate::process::current_pid();
            if wpid != 0 {
                let should_yield = if n > 0 {
                    true
                } else {
                    static WAKE_YIELD_COUNTER: core::sync::atomic::AtomicU32 =
                        core::sync::atomic::AtomicU32::new(0);
                    let cnt = WAKE_YIELD_COUNTER.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
                    cnt % 16 == 15
                };
                if should_yield {
                    if let Some(next) = crate::process::next_runnable_pid(wpid) {
                        if next != wpid {
                            let urip = crate::arch::syscall::user_rip();
                            let ursp = crate::arch::syscall::user_rsp();
                            crate::process::save_return_context(wpid, urip, ursp);
                            crate::process::save_full_user_gprs(wpid);
                            // Return value of futex_wake (n) will be in rax after resume.
                            crate::process::set_rax(wpid, n as u64);
                            crate::process::save_xstate(wpid);
                            crate::process::enter_user_by_pid_noreturn(next);
                        }
                    }
                }
            }
            if futex_addr_is_target(uaddr) {
                futex_trace_targets("wake-exit", wpid, uaddr, val);
            }
            n
        }
        9 => {
            // FUTEX_WAIT_BITSET: timed wait with bitset.  libc++ uses this op
            // for std::future::get() / wait_for().  We ignore the bitset and
            // timeout arguments and block identically to FUTEX_WAIT, which is
            // safe under cooperative scheduling because the sibling will call
            // set_value / FUTEX_WAKE before the parent ever needs to time out.
            if uaddr < 0x1000 || uaddr & 3 != 0 { return -22; }
            let cur = unsafe { core::ptr::read_volatile(uaddr as *const u32) };
            if cur != val {
                let pid = crate::process::current_pid();
                log::warn!("[futex-bitset] EAGAIN pid={} uaddr={:#x} cur={} expected={}", pid, uaddr, cur, val);
                return -11; // EAGAIN
            }
            let pid = crate::process::current_pid();
            if pid == 0 { return 0; }
            if futex_pid1_postrun_bypass(uaddr, val) {
                return 0;
            }
            log::warn!("[futex-bitset] WAIT pid={} uaddr={:#x} val={}", pid, uaddr, val);
            futex_waiter_add(uaddr, pid);
            if let Some(next) = crate::process::next_runnable_pid(pid) {
                if next != pid {
                    let urip = crate::arch::syscall::user_rip();
                    let ursp = crate::arch::syscall::user_rsp();
                    log::warn!(
                        "[futex-bitset] yield pid={} -> pid={} uaddr={:#x} val={} rip={:#x}",
                        pid, next, uaddr, val, urip
                    );
                    crate::process::save_return_context(pid, urip, ursp);
                    crate::process::save_full_user_gprs(pid);
                    crate::process::set_rax(pid, 0);
                    crate::process::save_xstate(pid);
                    crate::process::set_state(pid, crate::process::ProcState::Blocked);
                    crate::process::enter_user_by_pid_noreturn(next);
                }
            }
            while unsafe { core::ptr::read_volatile(uaddr as *const u32) } == val
                && futex_waiter_present(uaddr, pid)
            {
                #[cfg(target_arch = "x86_64")]
                unsafe {
                    core::arch::asm!("sti; hlt; cli", options(nomem, nostack));
                }
            }
            futex_waiter_remove(uaddr, pid);
            0
        }
        _ => -22, // EINVAL: unsupported op
    }
}

// ── Phase 60: pty/tty ─────────────────────────────────────────────────────────

fn sys_pty_open(flags: u64) -> i64 {
    let _ = flags;
    match crate::drivers::pty::open() {
        Ok((master, slave)) => ((master as i64) << 32) | (slave as i64),
        Err(_) => -24, // ENFILE
    }
}

fn sys_pty_read(fd: u64, buf_ptr: u64, count: u64) -> i64 {
    if buf_ptr == 0 { return -14; }
    let buf = unsafe { core::slice::from_raw_parts_mut(buf_ptr as *mut u8, count as usize) };
    match crate::drivers::pty::read(fd as u32, buf) {
        Ok(n) => n as i64,
        Err(_) => -5,
    }
}

fn sys_pty_write(fd: u64, buf_ptr: u64, count: u64) -> i64 {
    if buf_ptr == 0 { return -14; }
    let buf = unsafe { core::slice::from_raw_parts(buf_ptr as *const u8, count as usize) };
    match crate::drivers::pty::write(fd as u32, buf) {
        Ok(n) => n as i64,
        Err(_) => -5,
    }
}

fn sys_pty_ioctl(fd: u64, cmd: u64, arg: u64) -> i64 {
    crate::drivers::pty::ioctl(fd as u32, cmd, arg)
}

// ── Phase 31 Slice C: shared-address-space threads ───────────────────────────

/// Create a thread in the current process's address space.
///
/// Supports two calling conventions on syscall 0x35A:
/// - Raw OSCortex ABI: `(entry_fn, arg, stack_size)` -> returns `tid`.
/// - POSIX pthread ABI: `(pthread_t* out, attr*, start_routine, arg)` ->
///   writes `*out = tid`, returns `0` on success.
fn sys_thread_create(arg0: u64, arg1: u64, arg2: u64, arg3: u64) -> i64 {
    let parent_pid = crate::process::current_pid();
    if parent_pid == 0 {
        return -1; // EPERM
    }

    let spawn = |entry_fn: u64, arg: u64, stack_size: u64| -> Result<u32, i64> {
        let sz = if stack_size == 0 { 65536 } else { stack_size as usize };
        crate::process::spawn_thread(parent_pid, entry_fn, arg, sz).map_err(|_| -12)
    };

    // Heuristic for pthread_create-style call shape:
    // arg0 = pointer to pthread_t out, arg2 = start routine, arg3 = arg.
    // Raw ABI uses arg2 as stack_size, which is typically small; pthread
    // start-routine pointers are code VAs and thus much larger.
    if arg2 >= 0x10_0000 {
        let mut stack_size = 0u64;
        if arg1 != 0 {
            // Best-effort read of attr->stack_size at +8 (glibc/musl-compatible).
            if let Some(raw) = unsafe { read_user_bytes(arg1 + 8, 8) } {
                let mut tmp = [0u8; 8];
                tmp.copy_from_slice(raw);
                stack_size = u64::from_le_bytes(tmp);
            }
        }
        let (r, child_pid_opt) = match spawn(arg2, arg3, stack_size) {
            Ok(tid) => {
                let fs_base = crate::process::get_fs_base(tid);
                unsafe { *(arg0 as *mut u64) = fs_base; }
                (0_i64, Some(tid))
            }
            Err(errno) => (-errno, None),
        };
        log::trace!("[trace] sys_thread_create POSIX out={:#x} attr={:#x} entry={:#x} arg={:#x} stk={:#x} -> {}",
            arg0, arg1, arg2, arg3, stack_size, r);
        if let Some(child_pid) = child_pid_opt {
            log::warn!("[thread-create] spawned child={} entry={:#x}", child_pid, arg2);

            // Give the newborn thread one immediate slice. Without this, the
            // creator can monopolize userspace and delay child startup long
            // enough to starve wake-source setup (pipe/timer arming).
            let parent = crate::process::current_pid();
            if parent != 0 && child_pid != parent {
                let urip = crate::arch::syscall::user_rip();
                let ursp = crate::arch::syscall::user_rsp();
                crate::process::save_return_context(parent, urip, ursp);
                crate::process::save_full_user_gprs(parent);
                crate::process::set_rax(parent, 0);
                crate::process::save_xstate(parent);
                crate::process::enter_user_by_pid_noreturn(child_pid);
            }
        }
        r
    } else {
        // Raw OSCortex ABI.
        match spawn(arg0, arg1, arg2) {
            Ok(tid) => tid as i64,
            Err(errno) => errno,
        }
    }
}

/// Exit the current thread (or process if not a thread).
fn sys_thread_exit(code: u64) -> i64 {
    let pid = crate::process::current_pid();
    let user_rip = crate::arch::syscall::user_rip();
    let user_rsp = crate::arch::syscall::user_rsp();
    log::warn!("[trace] sys_thread_exit pid={} code={:#x} rip={:#x} rsp={:#x}",
        pid, code, user_rip, user_rsp);
    // Dump full syscall ring buffer so we can see EVERY syscall this thread
    // made before exiting — critical for diagnosing the Flutter fml::Thread
    // start_routine bail-out (only pthread_setname_np visible otherwise).
    dump_recent_syscalls(SYSCALL_TRACE_DEPTH);
    // Activate post-exit syscall trace window so we can see what pid=1 does
    // after it resumes from pthread_cond_wait.
    POSTEXIT_TRACE_COUNT.store(0, Ordering::Relaxed);
    // Disabled: engine is healthy past pid-2 exit; trace just spams logs.
    // POSTEXIT_TRACE_ACTIVE.store(true, Ordering::Relaxed);
    sys_exit(code)
}

/// Wait for a thread to finish and return its exit code.
fn sys_thread_join(thread_handle: u64) -> i64 {
    let tid = if thread_handle > 0x100000 {
        match crate::process::find_tid_by_fs_base(thread_handle) {
            Some(t) => t,
            None => return -10, // ECHILD
        }
    } else {
        thread_handle as u32
    };

    match crate::process::waitpid(tid) {
        Ok(code) => code as i64,
        Err("not exited") => -11,  // EAGAIN
        Err(_) => -10,             // ECHILD
    }
}

/// Linux-compat `clone(2)` — only thread-creation mode (CLONE_VM) supported.
///
/// Registers at syscall entry (SysV SYSCALL convention):
///   RAX=56, RDI=flags, RSI=stack_top, RDX=parent_tidptr (ignored),
///   R10=child_tidptr (ignored), R8=tls (ignored)
///
/// When CLONE_VM (0x100) is set the child shares the parent's address space.
/// The child's RIP is the SYSCALL return address (user_rip), the child's RSP
/// is `stack_top` (arg1).  Parent receives child TID; child receives 0.
fn sys_clone(flags: u64, stack_top: u64) -> i64 {
    const CLONE_VM: u64 = 0x0000_0100;
    if flags & CLONE_VM == 0 {
        return -38; // ENOSYS — only thread-clone supported
    }
    let parent_pid = crate::process::current_pid();
    if parent_pid == 0 {
        return -1; // EPERM
    }
    // Use the SYSCALL return address as the child's starting RIP.
    let child_rip = crate::arch::syscall::user_rip();
    if child_rip == 0 {
        return -22; // EINVAL
    }
    // stack_top == 0 means "copy parent stack" — not supported; reject.
    if stack_top == 0 {
        return -22; // EINVAL
    }
    match crate::process::clone_thread(parent_pid, child_rip, stack_top) {
        Ok(tid) => {
            // The child slot already has RAX=0 set by clone_thread.
            tid as i64 // parent sees child TID
        }
        Err(_) => -12, // ENOMEM
    }
}

// ── POSIX compatibility shim implementations ──────────────────────────────────

mod posix {
    //! Implementations of POSIX/glibc functions dispatched via syscall stubs
    //! from the trampoline page mapped by posix_trampolines.rs.

    use super::{read_user_bytes, write_user_bytes, monotonic_ns};
    use crate::process::{self, dl::mmap_anon};
    use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};
    use spin::Mutex;
    use alloc::vec::Vec;
    use alloc::collections::BTreeMap;

    // ── Per-process TLS area ──────────────────────────────────────────────────
    //
    // Each process gets a 128 KiB TLS area starting at a bumped user VA.
    // We store (pid → tls_base) here; the area is allocated on first use.

    static TLS_TABLE: Mutex<BTreeMap<u32, u64>> = Mutex::new(BTreeMap::new());

    // Per-thread emulated-TLS storage: (pid, obj_va) -> storage_va.
    //
    // CRITICAL: The "ptr" cell inside the __emutls_object (at obj+16) is shared
    // across all threads because the object lives in libflutter_engine.so's
    // .data. Caching the allocation there means thread A's storage becomes
    // visible to thread B, which is the OPPOSITE of what TLS means. Threads
    // then read each other's uninitialized slots (frequently NULL function
    // pointers) and crash via call-thru-NULL. We must keep a per-thread cache
    // here and NEVER write back into obj+16.
    static EMUTLS_TABLE: Mutex<BTreeMap<(u32, u64), u64>> = Mutex::new(BTreeMap::new());

    fn get_or_alloc_tls(pid: u32, pml4_phys: u64) -> u64 {
        let mut table = TLS_TABLE.lock();
        if let Some(&base) = table.get(&pid) {
            return base;
        }
        // Allocate 128 KiB for TLS in the current process.
        let pages = 32; // 32 × 4 KiB = 128 KiB
        let base = mmap_anon(pid, pml4_phys, 0, pages, 3);
        if base != u64::MAX {
            table.insert(pid, base);
        }
        base
    }

    // ── Per-process key-value store (pthread_key_*) ───────────────────────────

    // key: (pid, thread_id, key_id) → value (u64)
    static KEY_TABLE: Mutex<BTreeMap<(u32, u32, u32), u64>> = Mutex::new(BTreeMap::new());
    static NEXT_KEY: AtomicU32 = AtomicU32::new(1);

    // ── Per-thread pthread_self object ─────────────────────────────────────
    //
    // glibc/libstdc++ often treat pthread_t as a pointer to thread state and
    // dereference fixed offsets (for example +0x68). Returning a small integer
    // TID causes null-ish derefs inside C++ constructors.
    //
    // key: tid(pid) -> user VA of synthetic pthread object (256 bytes)
    static PTHREAD_SELF_TABLE: Mutex<BTreeMap<u32, u64>> = Mutex::new(BTreeMap::new());

    // ── Per-process malloc tracking ───────────────────────────────────────────
    // (pid, user_va) → pages_allocated
    // Used so free/realloc can skip already-allocated regions.
    // Since munmap is a stub (no actual unmapping), free is a no-op here.

    // ── Random state ─────────────────────────────────────────────────────────

    static RAND_STATE: AtomicU64 = AtomicU64::new(1234567890);
    static LOCALE_OBJ: AtomicU64 = AtomicU64::new(0);
    static LOCALE_CURRENT: AtomicU64 = AtomicU64::new(0);

    // ── Helpers ───────────────────────────────────────────────────────────────

    fn pid_and_pml4() -> (u32, u64) {
        let pid = process::current_pid();
        let pml4 = process::get_user_context(pid)
            .map(|ctx| ctx.pml4_phys)
            .unwrap_or(0);
        (pid, pml4)
    }

    /// Read a null-terminated C string from user memory (max 4096 bytes).
    unsafe fn read_cstr(ptr: u64) -> Option<&'static [u8]> {
        if ptr < 0x10000 || ptr >= 0x0000_8000_0000_0000 { return None; }
        let p = ptr as *const u8;
        let mut len = 0usize;
        unsafe {
            while len < 4096 && *p.add(len) != 0 { len += 1; }
        }
        Some(unsafe { core::slice::from_raw_parts(p, len) })
    }

    /// Write a value to a user pointer (8-byte atomic write).
    unsafe fn write_u64_user(ptr: u64, v: u64) {
        if user_ptr_ok(ptr) { unsafe { *(ptr as *mut u64) = v; } }
    }
    unsafe fn write_u32_user(ptr: u64, v: u32) {
        if user_ptr_ok(ptr) { unsafe { *(ptr as *mut u32) = v; } }
    }

    // ── Memory allocation ─────────────────────────────────────────────────────

    pub fn sys_malloc(size: u64) -> i64 {
        if size == 0 { return 8; } // non-null sentinel
        let (pid, pml4) = pid_and_pml4();
        if pid == 0 {
            log::error!("[malloc] pid=0 — no user context; size={:#x}", size);
            return 0;
        }
        // Progress counter — prints every 100K mallocs to show life signs
        static MALLOC_COUNT: core::sync::atomic::AtomicU64 =
            core::sync::atomic::AtomicU64::new(0);
        let n = MALLOC_COUNT.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
        if n % 100_000 == 99_999 {
            log::warn!("[malloc] progress: {} mallocs pid={}", n + 1, pid);
        }
        // Allocate size + 16 bytes (header stores size for realloc).
        let alloc_size = size as usize + 16;
        let pages = alloc_size.div_ceil(4096);
        let va = mmap_anon(pid, pml4, 0, pages, 3);
        if va == u64::MAX {
            let used = crate::mm::frame_allocator::frames_used();
            let total = crate::mm::frame_allocator::frames_total();
            log::error!("[malloc] OOM: size={:#x} pages={} pid={} pml4={:#x} frames={}/{}", size, pages, pid, pml4, used, total);
            return 0;
        }
        // Write allocation size into header (kernel can write since user VA is
        // directly accessible in our single-space model).
        unsafe { write_u64_user(va, size); }
        (va + 16) as i64  // return ptr past header
    }

    pub fn sys_free(_ptr: u64) -> i64 {
        // munmap is a stub → leak is acceptable for demo.
        0
    }

    pub fn sys_calloc(n: u64, size: u64) -> i64 {
        // mmap_anon zeroes pages, so malloc already gives zeroed memory.
        sys_malloc(n.saturating_mul(size))
    }

    pub fn sys_realloc(ptr: u64, size: u64) -> i64 {
        if ptr == 0 { return sys_malloc(size); }
        if size == 0 { sys_free(ptr); return 0; }
        // Get old size from header (ptr - 16).
        let old_size = if ptr >= 16 {
            unsafe { *((ptr - 16) as *const u64) }
        } else { 0 };
        if old_size >= size { return ptr as i64; } // already big enough
        let new_ptr = sys_malloc(size);
        if new_ptr == 0 || ptr == 0 { return new_ptr; }
        // Copy old data.
        let copy_len = old_size.min(size) as usize;
        unsafe {
            core::ptr::copy_nonoverlapping(ptr as *const u8, new_ptr as *mut u8, copy_len);
        }
        new_ptr
    }

    pub fn sys_aligned_alloc(_align: u64, size: u64) -> i64 {
        // mmap always returns page-aligned memory.
        sys_malloc(size)
    }

    pub fn sys_posix_memalign(pptr: u64, _align: u64, size: u64) -> i64 {
        let ptr = sys_malloc(size);
        unsafe { write_u64_user(pptr, ptr as u64); }
        if ptr == 0 { 12 } else { 0 } // 12 = ENOMEM
    }

    pub fn sys_malloc_usable_size(ptr: u64) -> i64 {
        if ptr < 16 { return 0; }
        let size = unsafe { *((ptr - 16) as *const u64) };
        size as i64
    }

    pub fn sys_strdup(s: u64) -> i64 {
        let len = sys_strlen(s) as u64;
        let new_ptr = sys_malloc(len + 1);
        if new_ptr == 0 { return 0; }
        sys_memcpy(new_ptr as u64, s, len + 1);
        new_ptr
    }

    pub fn sys_strndup(s: u64, n: u64) -> i64 {
        let slen = sys_strlen(s) as u64;
        let len = slen.min(n);
        let new_ptr = sys_malloc(len + 1);
        if new_ptr == 0 { return 0; }
        sys_memcpy(new_ptr as u64, s, len);
        // null-terminate
        unsafe { *((new_ptr as u64 + len) as *mut u8) = 0; }
        new_ptr
    }

    // ── String operations ─────────────────────────────────────────────────────

    pub fn sys_strlen(s: u64) -> i64 {
        if s < 0x10000 || s >= 0x0000_8000_0000_0000 { return 0; }  // guard: only userspace addresses
        let mut len = 0i64;
        unsafe {
            let mut p = s as *const u8;
            while *p != 0 { p = p.add(1); len += 1; if len > 1024*1024 { break; } }
        }
        len
    }

    #[inline(always)]
    fn user_ptr_ok(p: u64) -> bool {
        p >= 0x1000 && p < 0x0000_8000_0000_0000
    }

    pub fn sys_memcpy(dst: u64, src: u64, n: u64) -> i64 {
        if n == 0 || !user_ptr_ok(dst) || !user_ptr_ok(src) { return dst as i64; }
        unsafe { core::ptr::copy_nonoverlapping(src as *const u8, dst as *mut u8, n as usize); }
        dst as i64
    }

    pub fn sys_memset(dst: u64, val: u64, n: u64) -> i64 {
        if n == 0 || !user_ptr_ok(dst) { return dst as i64; }
        unsafe { core::ptr::write_bytes(dst as *mut u8, val as u8, n as usize); }
        dst as i64
    }

    pub fn sys_memmove(dst: u64, src: u64, n: u64) -> i64 {
        if n == 0 || !user_ptr_ok(dst) || !user_ptr_ok(src) { return dst as i64; }
        unsafe { core::ptr::copy(src as *const u8, dst as *mut u8, n as usize); }
        dst as i64
    }

    pub fn sys_memcmp(a: u64, b: u64, n: u64) -> i64 {
        if n == 0 { return 0; }
        if !user_ptr_ok(a) || !user_ptr_ok(b) { return 0; }
        let sa = unsafe { core::slice::from_raw_parts(a as *const u8, n as usize) };
        let sb = unsafe { core::slice::from_raw_parts(b as *const u8, n as usize) };
        for i in 0..n as usize {
            let diff = sa[i] as i32 - sb[i] as i32;
            if diff != 0 { return diff as i64; }
        }
        0
    }

    pub fn sys_memchr(s: u64, c: u64, n: u64) -> i64 {
        if n == 0 || !user_ptr_ok(s) { return 0; }
        let slice = unsafe { core::slice::from_raw_parts(s as *const u8, n as usize) };
        for (i, &b) in slice.iter().enumerate() {
            if b == c as u8 { return (s + i as u64) as i64; }
        }
        0
    }

    pub fn sys_bzero(dst: u64, n: u64) -> i64 {
        sys_memset(dst, 0, n)
    }

    pub fn sys_strcmp(a: u64, b: u64) -> i64 {
        if a == 0 || b == 0 { return if a == b { 0 } else { 1 }; }
        let mut pa = a as *const u8;
        let mut pb = b as *const u8;
        loop {
            let ca = unsafe { *pa };
            let cb = unsafe { *pb };
            if ca != cb { return (ca as i64) - (cb as i64); }
            if ca == 0 { return 0; }
            pa = unsafe { pa.add(1) };
            pb = unsafe { pb.add(1) };
        }
    }

    pub fn sys_strncmp(a: u64, b: u64, n: u64) -> i64 {
        if n == 0 { return 0; }
        let mut pa = a as *const u8;
        let mut pb = b as *const u8;
        for _ in 0..n {
            let ca = unsafe { *pa };
            let cb = unsafe { *pb };
            if ca != cb { return (ca as i64) - (cb as i64); }
            if ca == 0 { return 0; }
            pa = unsafe { pa.add(1) };
            pb = unsafe { pb.add(1) };
        }
        0
    }

    pub fn sys_strcpy(dst: u64, src: u64) -> i64 {
        if dst == 0 || src == 0 { return dst as i64; }
        let mut ps = src as *const u8;
        let mut pd = dst as *mut u8;
        loop {
            let c = unsafe { *ps };
            unsafe { *pd = c; }
            if c == 0 { break; }
            ps = unsafe { ps.add(1) };
            pd = unsafe { pd.add(1) };
        }
        dst as i64
    }

    pub fn sys_strncpy(dst: u64, src: u64, n: u64) -> i64 {
        if dst == 0 { return 0; }
        let mut ps = src as *const u8;
        let mut pd = dst as *mut u8;
        for _ in 0..n {
            let c = if src != 0 { unsafe { let v = *ps; ps = ps.add(1); v } } else { 0 };
            unsafe { *pd = c; pd = pd.add(1); }
        }
        dst as i64
    }

    pub fn sys_strcat(dst: u64, src: u64) -> i64 {
        let dlen = sys_strlen(dst) as u64;
        sys_strcpy(dst + dlen, src);
        dst as i64
    }

    pub fn sys_strncat(dst: u64, src: u64, n: u64) -> i64 {
        let dlen = sys_strlen(dst) as u64;
        sys_strncpy(dst + dlen, src, n);
        // ensure null terminator
        let total = dlen + n;
        unsafe { *((dst + total) as *mut u8) = 0; }
        dst as i64
    }

    pub fn sys_strstr(hay: u64, needle: u64) -> i64 {
        if hay == 0 || needle == 0 { return 0; }
        let nlen = sys_strlen(needle) as usize;
        if nlen == 0 { return hay as i64; }
        let hlen = sys_strlen(hay) as usize;
        if nlen > hlen { return 0; }
        let hs = unsafe { core::slice::from_raw_parts(hay as *const u8, hlen) };
        let ns = unsafe { core::slice::from_raw_parts(needle as *const u8, nlen) };
        for i in 0..=(hlen - nlen) {
            if hs[i..i+nlen] == *ns { return (hay + i as u64) as i64; }
        }
        0
    }

    pub fn sys_strchr(s: u64, c: u64) -> i64 {
        if s == 0 { return 0; }
        let mut p = s as *const u8;
        loop {
            let b = unsafe { *p };
            if b == c as u8 { return p as i64; }
            if b == 0 { return 0; }
            p = unsafe { p.add(1) };
        }
    }

    pub fn sys_strrchr(s: u64, c: u64) -> i64 {
        if s == 0 { return 0; }
        let mut p = s as *const u8;
        let mut last = 0i64;
        loop {
            let b = unsafe { *p };
            if b == c as u8 { last = p as i64; }
            if b == 0 { break; }
            p = unsafe { p.add(1) };
        }
        last
    }

    pub fn sys_strnlen(s: u64, n: u64) -> i64 {
        if s == 0 { return 0; }
        let mut len = 0u64;
        let mut p = s as *const u8;
        while len < n {
            if unsafe { *p } == 0 { break; }
            p = unsafe { p.add(1) };
            len += 1;
        }
        len as i64
    }

    pub fn sys_strcspn(s: u64, reject: u64) -> i64 {
        if s == 0 { return 0; }
        let rlen = sys_strlen(reject) as usize;
        let rs = if reject != 0 { unsafe { core::slice::from_raw_parts(reject as *const u8, rlen) } } else { &[] };
        let mut len = 0i64;
        let mut p = s as *const u8;
        loop {
            let c = unsafe { *p };
            if c == 0 { break; }
            if rs.contains(&c) { break; }
            p = unsafe { p.add(1) };
            len += 1;
        }
        len
    }

    pub fn sys_strspn(s: u64, accept: u64) -> i64 {
        if s == 0 { return 0; }
        let alen = sys_strlen(accept) as usize;
        let acc = if accept != 0 { unsafe { core::slice::from_raw_parts(accept as *const u8, alen) } } else { &[] };
        let mut len = 0i64;
        let mut p = s as *const u8;
        loop {
            let c = unsafe { *p };
            if c == 0 || !acc.contains(&c) { break; }
            p = unsafe { p.add(1) };
            len += 1;
        }
        len
    }

    pub fn sys_strcasestr(hay: u64, needle: u64) -> i64 {
        // Simple case-insensitive substring search.
        if hay == 0 || needle == 0 { return 0; }
        let hlen = sys_strlen(hay) as usize;
        let nlen = sys_strlen(needle) as usize;
        if nlen == 0 { return hay as i64; }
        if nlen > hlen { return 0; }
        let hs = unsafe { core::slice::from_raw_parts(hay as *const u8, hlen) };
        let ns = unsafe { core::slice::from_raw_parts(needle as *const u8, nlen) };
        'outer: for i in 0..=(hlen - nlen) {
            for j in 0..nlen {
                if hs[i+j].to_ascii_lowercase() != ns[j].to_ascii_lowercase() {
                    continue 'outer;
                }
            }
            return (hay + i as u64) as i64;
        }
        0
    }

    pub fn sys_strtol(s: u64, endptr: u64, base: u64) -> i64 {
        if s == 0 { return 0; }
        let bytes = unsafe { read_cstr(s) }.unwrap_or(&[]);
        let s = core::str::from_utf8(bytes).unwrap_or("").trim_start();
        let (neg, s) = if s.starts_with('-') { (true, &s[1..]) } else { (false, s) };
        let base = if base == 0 { 10 } else { base };
        let mut val: i64 = 0;
        let mut consumed = 0usize;
        for c in s.bytes() {
            let d = match c {
                b'0'..=b'9' => (c - b'0') as i64,
                b'a'..=b'f' => (c - b'a' + 10) as i64,
                b'A'..=b'F' => (c - b'A' + 10) as i64,
                _ => break,
            };
            if d >= base as i64 { break; }
            val = val * base as i64 + d;
            consumed += 1;
        }
        if endptr != 0 {
            unsafe { *(endptr as *mut u64) = s.as_ptr() as u64 + consumed as u64 + if neg { 1 } else { 0 }; }
        }
        if neg { -val } else { val }
    }

    pub fn sys_strtoul(s: u64, endptr: u64, base: u64) -> i64 {
        sys_strtol(s, endptr, base)
    }

    pub fn sys_strtoll(s: u64, endptr: u64, base: u64) -> i64 {
        sys_strtol(s, endptr, base)
    }

    pub fn sys_strtoull(s: u64, endptr: u64, base: u64) -> i64 {
        sys_strtol(s, endptr, base)
    }

    pub fn sys_atoi(s: u64) -> i64 {
        sys_strtol(s, 0, 10)
    }

    pub fn sys_rand() -> i64 {
        // Xorshift64
        let mut x = RAND_STATE.load(Ordering::Relaxed);
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        RAND_STATE.store(x, Ordering::Relaxed);
        (x & 0x7FFFFFFF) as i64
    }

    pub fn sys_srand(seed: u32) {
        RAND_STATE.store(seed as u64 | 1, Ordering::Relaxed);
    }

    // ── Threading ─────────────────────────────────────────────────────────────

    pub fn sys_pthread_self() -> i64 {
        let tid = process::current_pid();
        if tid == 0 { return 0; }

        let fs_base = crate::process::get_fs_base(tid);
        if fs_base != 0 {
            return fs_base as i64;
        }

        // Fast path: already allocated for this tid.
        if let Some(&va) = PTHREAD_SELF_TABLE.lock().get(&tid) {
            return va as i64;
        }

        // Allocate a synthetic thread object in user VA and publish it.
        // Keep it larger than common pthread header offsets used by runtimes.
        let obj = sys_malloc(256) as u64;
        if obj == 0 {
            // OOM fallback: return a non-null tagged value rather than 0.
            return ((tid as u64) << 12 | 1) as i64;
        }

        unsafe {
            // Clear object and set a few self-referential fields expected by
            // pointer-based pthread_t implementations.
            core::ptr::write_bytes(obj as *mut u8, 0, 256);
            *(obj as *mut u64) = obj;              // self pointer
            *((obj + 8) as *mut u64) = tid as u64; // tid
            *((obj + 0x68) as *mut u64) = obj;     // non-null guard field
        }

        PTHREAD_SELF_TABLE.lock().insert(tid, obj);
        obj as i64
    }

    pub fn sys_pthread_mutex_init(mutex: u64) -> i64 {
        // A mutex is a 64-bit value: 0 = unlocked.
        if mutex != 0 { unsafe { *(mutex as *mut u64) = 0; } }
        0
    }

    pub fn sys_pthread_mutex_lock(mutex: u64, sys_nr: u64) -> i64 {
        if mutex < 0x1000 {
            log::warn!("[mutex] pthread_mutex_lock bogus addr={:#x}, returning 0", mutex);
            return 0;
        }
        let atom = unsafe { &*(mutex as *const core::sync::atomic::AtomicU32) };
        let pid = crate::process::current_pid();

        loop {
            if atom.compare_exchange(0, 1, Ordering::Acquire, Ordering::Acquire).is_ok() {
                return 0;
            }

            if pid == 0 {
                return 0;
            }

            // Add the process to the futex waiters list
            super::futex_waiter_add(mutex, pid);

            // Yield if there is a sibling runnable thread
            if let Some(next) = crate::process::next_runnable_pid(pid) {
                if next != pid {
                    let urip = crate::arch::syscall::user_rip();
                    let ursp = crate::arch::syscall::user_rsp();
                    // Save context pointing to the syscall instruction itself so we retry on re-entry.
                    crate::process::save_return_context(pid, urip - 2, ursp);
                    crate::process::save_full_user_gprs(pid);
                    crate::process::set_rax(pid, sys_nr); // Restore the original syscall number
                    crate::process::save_xstate(pid);
                    crate::process::set_state(pid, crate::process::ProcState::Blocked);
                    crate::process::enter_user_by_pid_noreturn(next);
                }
            }

            // No sibling thread — loop in kernel space using hlt
            while atom.load(Ordering::Acquire) != 0 && super::futex_waiter_present(mutex, pid) {
                #[cfg(target_arch = "x86_64")]
                unsafe {
                    core::arch::asm!("sti; hlt; cli", options(nomem, nostack));
                }
            }

            super::futex_waiter_remove(mutex, pid);
        }
    }

    pub fn sys_pthread_mutex_unlock(mutex: u64) -> i64 {
        if mutex < 0x1000 { return 0; }
        let atom = unsafe { &*(mutex as *const core::sync::atomic::AtomicU32) };
        atom.store(0, Ordering::Release);
        let n = super::futex_wake_waiters(mutex, 1);
        let wpid = crate::process::current_pid();
        if wpid != 0 && n > 0 {
            if let Some(next) = crate::process::next_runnable_pid(wpid) {
                if next != wpid {
                    let urip = crate::arch::syscall::user_rip();
                    let ursp = crate::arch::syscall::user_rsp();
                    crate::process::save_return_context(wpid, urip, ursp);
                    crate::process::save_full_user_gprs(wpid);
                    crate::process::set_rax(wpid, 0);
                    crate::process::save_xstate(wpid);
                    crate::process::enter_user_by_pid_noreturn(next);
                }
            }
        }
        0
    }

    pub fn sys_pthread_mutex_trylock(mutex: u64) -> i64 {
        if mutex < 0x1000 { return 0; }
        let atom = unsafe { &*(mutex as *const core::sync::atomic::AtomicU32) };
        if atom.compare_exchange(0, 1, Ordering::Acquire, Ordering::Acquire).is_ok() { 0 } else { 16 }
    }

    pub fn sys_pthread_once(once: u64, func: u64) -> i64 {
        if once < 0x1000 || func == 0 {
            log::error!("[pthread-err] once EINVAL pid={} once={:#x} func={:#x} rip={:#x}",
                crate::process::current_pid(), once, func, crate::arch::syscall::user_rip());
            return 22;
        }
        let atom = unsafe { &*(once as *const core::sync::atomic::AtomicU32) };
        let pid = crate::process::current_pid();

        loop {
            // State: 0=uninit, 1=in-progress, 2=done
            let state = atom.load(Ordering::Acquire);
            if state == 2 {
                return 0;
            }
            if state == 0 {
                if atom.compare_exchange(0, 1, Ordering::Acquire, Ordering::Acquire).is_ok() {
                    // We won the race — call func.
                    let f: extern "C" fn() = unsafe { core::mem::transmute(func) };
                    f();
                    atom.store(2, Ordering::Release);
                    // Wake all threads blocked waiting for the init to complete.
                    let n = super::futex_wake_waiters(once, u32::MAX);
                    let wpid = crate::process::current_pid();
                    if wpid != 0 && n > 0 {
                        if let Some(next) = crate::process::next_runnable_pid(wpid) {
                            if next != wpid {
                                let urip = crate::arch::syscall::user_rip();
                                let ursp = crate::arch::syscall::user_rsp();
                                crate::process::save_return_context(wpid, urip, ursp);
                                crate::process::save_full_user_gprs(wpid);
                                crate::process::set_rax(wpid, 0);
                                crate::process::save_xstate(wpid);
                                crate::process::enter_user_by_pid_noreturn(next);
                            }
                        }
                    }
                    return 0;
                }
            }

            // If state == 1, block until it becomes 2.
            if pid == 0 {
                return 0;
            }

            super::futex_waiter_add(once, pid);

            if let Some(next) = crate::process::next_runnable_pid(pid) {
                if next != pid {
                    let urip = crate::arch::syscall::user_rip();
                    let ursp = crate::arch::syscall::user_rsp();
                    // Save context pointing to the syscall instruction itself so we retry on re-entry.
                    crate::process::save_return_context(pid, urip - 2, ursp);
                    crate::process::save_full_user_gprs(pid);
                    crate::process::set_rax(pid, 0x3d7); // sys_pthread_once syscall number
                    crate::process::save_xstate(pid);
                    crate::process::set_state(pid, crate::process::ProcState::Blocked);
                    crate::process::enter_user_by_pid_noreturn(next);
                }
            }

            // No sibling runnable thread — loop using hlt in the kernel until state is 2.
            while atom.load(Ordering::Acquire) != 2 && super::futex_waiter_present(once, pid) {
                #[cfg(target_arch = "x86_64")]
                unsafe {
                    core::arch::asm!("sti; hlt; cli", options(nomem, nostack));
                }
            }

            super::futex_waiter_remove(once, pid);
        }
    }

    pub fn sys_pthread_key_create(key_ptr: u64, _dtor: u64) -> i64 {
        let key = NEXT_KEY.fetch_add(1, Ordering::Relaxed);
        unsafe { write_u32_user(key_ptr, key); }
        0
    }

    pub fn sys_pthread_setspecific(key: u64, value: u64) -> i64 {
        let pid = process::current_pid();
        let tid = pid; // simplified: single thread per process
        KEY_TABLE.lock().insert((pid, tid, key as u32), value);
        0
    }

    pub fn sys_pthread_getspecific(key: u64) -> i64 {
        let pid = process::current_pid();
        let tid = pid;
        KEY_TABLE.lock().get(&(pid, tid, key as u32)).copied().unwrap_or(0) as i64
    }

    pub fn sys_pthread_cond_wait_timeout(cond: u64, mutex: u64, timeout_ns: u64, sys_nr: u64) -> i64 {
        if cond < 0x1000 || mutex < 0x1000 {
            log::error!("[pthread-err] cond_wait EINVAL pid={} cond={:#x} mutex={:#x} rip={:#x}",
                crate::process::current_pid(), cond, mutex, crate::arch::syscall::user_rip());
            return 22;
        }

        let pid = crate::process::current_pid();
        let atom = unsafe { &*(cond as *const core::sync::atomic::AtomicU32) };

        // Check if we have an active wait state
        let mut state = {
            let table = super::COND_WAIT_STATE.lock();
            table.get(&pid).copied()
        };

        if state.is_none() {
            // First time: unlock mutex and enter Waiting state.
            let seq = atom.load(Ordering::Acquire);
            sys_pthread_mutex_unlock(mutex);
            let next_state = super::CondWaitState::Waiting { cond, mutex, seq, timeout_ns };
            super::COND_WAIT_STATE.lock().insert(pid, next_state);
            state = Some(next_state);
        }

        // Now process the state machine
        loop {
            match state.unwrap() {
                super::CondWaitState::Waiting { cond, mutex, seq, timeout_ns } => {
                    let cur_seq = atom.load(Ordering::Acquire);
                    if cur_seq != seq {
                        // The condvar was signaled!
                        super::futex_waiter_remove(cond, pid);
                        let next_state = super::CondWaitState::AcquiringMutex { mutex, timed_out: false };
                        super::COND_WAIT_STATE.lock().insert(pid, next_state);
                        state = Some(next_state);
                        // Continue to try and acquire the mutex
                        continue;
                    }

                    // Check for timeout
                    if timeout_ns != 0 && monotonic_ns() >= timeout_ns {
                        super::futex_waiter_remove(cond, pid);
                        let next_state = super::CondWaitState::AcquiringMutex { mutex, timed_out: true };
                        super::COND_WAIT_STATE.lock().insert(pid, next_state);
                        state = Some(next_state);
                        continue;
                    }

                    // Otherwise, we must wait (yield or hlt)
                    if pid != 0 {
                        super::futex_waiter_add(cond, pid);
                        if let Some(next) = crate::process::next_runnable_pid(pid) {
                            if next != pid {
                                let urip = crate::arch::syscall::user_rip();
                                let ursp = crate::arch::syscall::user_rsp();
                                crate::process::save_return_context(pid, urip - 2, ursp);
                                crate::process::save_full_user_gprs(pid);
                                crate::process::set_rax(pid, sys_nr); // Restore the original syscall number
                                crate::process::save_xstate(pid);
                                crate::process::set_state(pid, crate::process::ProcState::Blocked);
                                crate::process::enter_user_by_pid_noreturn(next);
                            }
                        }
                    }

                    // No sibling runnable thread — loop using hlt in the kernel until sequence changes or timeout
                    while atom.load(Ordering::Acquire) == seq 
                        && (timeout_ns == 0 || monotonic_ns() < timeout_ns)
                        && super::futex_waiter_present(cond, pid) 
                    {
                        #[cfg(target_arch = "x86_64")]
                        unsafe {
                            core::arch::asm!("sti; hlt; cli", options(nomem, nostack));
                        }
                    }

                    // Re-evaluate condition after waking up
                    let cur_seq = atom.load(Ordering::Acquire);
                    if cur_seq != seq {
                        super::futex_waiter_remove(cond, pid);
                        let next_state = super::CondWaitState::AcquiringMutex { mutex, timed_out: false };
                        super::COND_WAIT_STATE.lock().insert(pid, next_state);
                        state = Some(next_state);
                        continue;
                    }

                    if timeout_ns != 0 && monotonic_ns() >= timeout_ns {
                        super::futex_waiter_remove(cond, pid);
                        let next_state = super::CondWaitState::AcquiringMutex { mutex, timed_out: true };
                        super::COND_WAIT_STATE.lock().insert(pid, next_state);
                        state = Some(next_state);
                        continue;
                    }
                }

                super::CondWaitState::AcquiringMutex { mutex, timed_out } => {
                    // Try to acquire the mutex (using the new sys_pthread_mutex_lock logic, retrying sys_nr on yield)
                    let rc = sys_pthread_mutex_lock(mutex, sys_nr);
                    if rc == 0 {
                        // Successfully acquired the mutex! Clear wait state and return
                        super::COND_WAIT_STATE.lock().remove(&pid);
                        return if timed_out { 110 } else { 0 }; // 110 = ETIMEDOUT
                    } else {
                        // yielded inside sys_pthread_mutex_lock
                        return rc;
                    }
                }
            }
        }
    }

    pub fn sys_pthread_cond_wait(cond: u64, mutex: u64, sys_nr: u64) -> i64 {
        sys_pthread_cond_wait_timeout(cond, mutex, 0, sys_nr)
    }

    pub fn sys_pthread_cond_timedwait(cond: u64, mutex: u64, timeout: u64, sys_nr: u64) -> i64 {
        let (sec, nsec) = if timeout != 0 {
            unsafe {
                (
                    core::ptr::read_unaligned(timeout as *const i64),
                    core::ptr::read_unaligned((timeout + 8) as *const i64),
                )
            }
        } else {
            (0, 0)
        };
        let timeout_ns = if timeout != 0 {
            (sec.max(0) as u64).saturating_mul(1_000_000_000)
                .saturating_add(nsec.max(0) as u64)
        } else {
            0
        };

        static TIMEDWAIT_LOG: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
        let n = TIMEDWAIT_LOG.fetch_add(1, Ordering::Relaxed);
        if n < 32 {
            log::warn!("[timedwait] #{} pid={} cond={:#x} mutex={:#x} timeout_ns={} now={}",
                n, crate::process::current_pid(), cond, mutex, timeout_ns, monotonic_ns());
        }

        sys_pthread_cond_wait_timeout(cond, mutex, timeout_ns, sys_nr)
    }

    #[inline]
    fn cond_broadcast_loop_handoff(_pid: u32, _cond: u64, _n: i64, _bridged: u32) {}

    pub fn sys_pthread_cond_signal(cond: u64) -> i64 {
        if cond == 0 { return 22; }
        let pid = crate::process::current_pid();
        let atom = unsafe { &*(cond as *const core::sync::atomic::AtomicU32) };
        let old_seq = atom.fetch_add(1, Ordering::Release);
        let n = super::futex_wake_waiters(cond, 1);
        let mut bridged = 0u32;
        if n == 0 {
            bridged = super::cond_miss_bridge(cond, 1);
            if bridged > 0 {
                log::warn!(
                    "[cond-signal-bridged] pid={} cond={:#x} woke={} bridged={}",
                    pid,
                    cond,
                    n,
                    bridged
                );
            }
        }
        log::trace!("[cond-signal] pid={} cond={:#x} seq {}→{} woke={}", pid, cond, old_seq, old_seq+1, n);
        let wpid = crate::process::current_pid();
        if wpid != 0 && (n > 0 || bridged > 0) {
            if let Some(next) = crate::process::next_runnable_pid(wpid) {
                if next != wpid {
                    let urip = crate::arch::syscall::user_rip();
                    let ursp = crate::arch::syscall::user_rsp();
                    crate::process::save_return_context(wpid, urip, ursp);
                    crate::process::save_full_user_gprs(wpid);
                    crate::process::set_rax(wpid, 0);
                    crate::process::save_xstate(wpid);
                    crate::process::enter_user_by_pid_noreturn(next);
                }
            }
        }
        0
    }

    pub fn sys_pthread_cond_broadcast(cond: u64) -> i64 {
        if cond == 0 { return 22; }
        let pid = crate::process::current_pid();
        let atom = unsafe { &*(cond as *const core::sync::atomic::AtomicU32) };
        let old_seq = atom.fetch_add(1, Ordering::Release);
        let n = super::futex_wake_waiters(cond, i32::MAX as u32);
        let mut bridged = 0u32;
        let mut skip_bridge = false;
        if n == 0 && pid == 2 {
            let ursp = crate::arch::syscall::user_rsp();
            let cur_cr3 = crate::arch::memory::read_cr3() & 0x000f_ffff_ffff_f000;
            if ursp != 0 && crate::mm::paging::translate_user_page(cur_cr3, ursp & !0xfff).is_some() {
                let caller = unsafe { core::ptr::read_unaligned(ursp as *const u64) };
                // Flutter engine Dart VM `ConditionVariable::NotifyAll` callsite
                // after `pthread_cond_broadcast@plt` (see mapped addr 0x326d61e).
                // Bridging this path causes a synthetic wake storm.
                if caller == 0x326d61e {
                    skip_bridge = true;
                }
            }
        }

        if n == 0 && !skip_bridge {
            bridged = super::cond_miss_bridge(cond, i32::MAX as u32);
            if bridged > 0 {
                log::warn!(
                    "[cond-broadcast-bridged] pid={} cond={:#x} woke={} bridged={}",
                    pid,
                    cond,
                    n,
                    bridged
                );
            }
        }
        cond_broadcast_loop_handoff(pid, cond, n, bridged);

        log::trace!(
            "[cond-broadcast] pid={} cond={:#x} seq {}→{} woke={}",
            pid,
            cond,
            old_seq,
            old_seq + 1,
            n
        );
        
        let wpid = crate::process::current_pid();
        if wpid != 0 && (n > 0 || bridged > 0) {
            if let Some(next) = crate::process::next_runnable_pid(wpid) {
                if next != wpid {
                    let urip = crate::arch::syscall::user_rip();
                    let ursp = crate::arch::syscall::user_rsp();
                    crate::process::save_return_context(wpid, urip, ursp);
                    crate::process::save_full_user_gprs(wpid);
                    crate::process::set_rax(wpid, 0);
                    crate::process::save_xstate(wpid);
                    crate::process::enter_user_by_pid_noreturn(next);
                }
            }
        }
        0
    }

    pub fn sys_pthread_attr_init(attr: u64) -> i64 {
        log::trace!("[trace] sys_pthread_attr_init attr={:#x}", attr);
        // pthread_attr_t is typically 56 bytes; zero it out.
        if attr != 0 { unsafe { core::ptr::write_bytes(attr as *mut u8, 0, 56); } }
        0
    }

    pub fn sys_pthread_attr_destroy(attr: u64) -> i64 {
        log::trace!("[trace] sys_pthread_attr_destroy attr={:#x}", attr);
        0
    }

    pub fn sys_pthread_attr_setstacksize(attr: u64, stacksize: u64) -> i64 {
        log::trace!("[trace] sys_pthread_attr_setstacksize attr={:#x} size={:#x}", attr, stacksize);
        // Store stacksize at offset 8 of the attr struct (glibc layout).
        if attr != 0 { unsafe { *((attr + 8) as *mut u64) = stacksize; } }
        0
    }

    pub fn sys_pthread_attr_setdetachstate(attr: u64, state: u64) -> i64 {
        log::trace!("[trace] sys_pthread_attr_setdetachstate attr={:#x} state={}", attr, state);
        // Store detach state at offset 0.
        if attr != 0 { unsafe { *(attr as *mut u64) = state; } }
        0
    }

    pub fn sys_pthread_attr_getstack(attr: u64, base_out: u64, size_out: u64) -> i64 {
        // The Dart VM calls pthread_getattr_np(self, &attr) then
        // pthread_attr_getstack(&attr, &base, &size) to validate stack bounds.
        // If we return 0,0 the VM aborts with "GetAndValidateThreadStackBounds failed".
        // Look up the current thread's recorded user-space stack bounds.
        let tid = crate::process::current_pid();
        let (base, size) = crate::process::get_user_stack_bounds(tid);

        // Also check if the attr struct was pre-filled by pthread_getattr_np
        // (stored at attr+0x10 = base, attr+0x18 = size).
        let (eff_base, eff_size) = if base != 0 {
            (base, size)
        } else if attr >= 0x1000 {
            // Try reading what pthread_getattr_np may have written into attr.
            let a_base = unsafe { *(attr.wrapping_add(0x10) as *const u64) };
            let a_size = unsafe { *(attr.wrapping_add(0x18) as *const u64) };
            if a_base != 0 && a_size != 0 { (a_base, a_size) } else { (0, 0) }
        } else {
            (0, 0)
        };

        log::debug!("[pthread_attr_getstack] tid={} base={:#x} size={:#x}", tid, eff_base, eff_size);
        unsafe {
            if base_out != 0 { *(base_out as *mut u64) = eff_base; }
            if size_out != 0 { *(size_out as *mut u64) = eff_size; }
        }
        0
    }

    pub fn sys_pthread_setname_np(thread: u64, name: u64) -> i64 {
        let tid = if thread > 0x100000 {
            crate::process::find_tid_by_fs_base(thread).unwrap_or(0)
        } else {
            thread as u32
        };
        let name_str = if name > 0x1000 {
            let bytes = unsafe {
                let p = name as *const u8;
                let mut end = 0usize;
                while end < 64 && *p.add(end) != 0 { end += 1; }
                core::slice::from_raw_parts(p, end)
            };
            core::str::from_utf8(bytes).unwrap_or("?")
        } else { "?" };
        log::trace!("[trace] sys_pthread_setname_np thread={:#x} (tid={}) name=\"{}\"", thread, tid, name_str);
        0
    }

    pub fn sys_pthread_attr_getter_noop(nr: u64, a0: u64, a1: u64, a2: u64) -> i64 {
        log::trace!("[trace] pthread_attr/sched noop nr={:#x} a0={:#x} a1={:#x} a2={:#x}", nr, a0, a1, a2);
        0
    }

    // ── Semaphores ────────────────────────────────────────────────────────────

    pub fn sys_sem_init(sem: u64, _pshared: u64, value: u64) -> i64 {
        if sem == 0 {
            log::error!("[pthread-err] sem_init EINVAL pid={} rip={:#x}",
                crate::process::current_pid(), crate::arch::syscall::user_rip());
            return 22;
        }
        unsafe { *(sem as *mut u32) = value as u32; }
        0
    }

    pub fn sys_sem_wait(sem: u64) -> i64 {
        if sem < 0x1000 {
            log::error!("[pthread-err] sem_wait EINVAL pid={} sem={:#x} rip={:#x}",
                crate::process::current_pid(), sem, crate::arch::syscall::user_rip());
            return 22;
        }
        let atom = unsafe { &*(sem as *const core::sync::atomic::AtomicU32) };
        loop {
            let v = atom.load(Ordering::Acquire);
            if v > 0 && atom.compare_exchange(v, v - 1, Ordering::Acquire, Ordering::Acquire).is_ok() {
                break;
            }
            unsafe { core::arch::asm!("pause", options(nomem, nostack, preserves_flags)); }
        }
        0
    }

    pub fn sys_sem_trywait(sem: u64) -> i64 {
        if sem < 0x1000 { return -11; }
        let atom = unsafe { &*(sem as *const core::sync::atomic::AtomicU32) };
        let v = atom.load(Ordering::Acquire);
        if v == 0 { return -11; }
        if atom.compare_exchange(v, v - 1, Ordering::Acquire, Ordering::Acquire).is_ok() { 0 } else { -11 }
    }

    pub fn sys_sem_post(sem: u64) -> i64 {
        if sem == 0 {
            log::error!("[pthread-err] sem_post EINVAL pid={} rip={:#x}",
                crate::process::current_pid(), crate::arch::syscall::user_rip());
            return 22;
        }
        let atom = unsafe { &*(sem as *const core::sync::atomic::AtomicU32) };
        atom.fetch_add(1, Ordering::Release);
        0
    }

    // ── TLS ───────────────────────────────────────────────────────────────────

    pub fn sys_tls_get_addr(ti_ptr: u64) -> i64 {
        // `ti_ptr` points to a TLS descriptor: [module_id: u64, offset: u64].
        let offset = if ti_ptr != 0 {
            unsafe { *((ti_ptr + 8) as *const u64) }
        } else { 0 };
        let (pid, pml4) = pid_and_pml4();
        let tls_base = get_or_alloc_tls(pid, pml4);
        if tls_base == u64::MAX { return 0; }
        // Return base + (offset & 0x1FFFF) — keep within 128 KiB.
        (tls_base + (offset & 0x1_FFFF)) as i64
    }

    // ── GNU emulated TLS ──────────────────────────────────────────────────────
    //
    // Flutter's libflutter_engine.so is commonly built without a native TLS
    // segment (no PT_TLS).  The compiler emits calls to __emutls_get_address
    // instead of the ELF TLS descriptor protocol.
    //
    // __emutls_object layout (all fields pointer-sized, i.e. 8 bytes):
    //   +0  size   – size of the variable's storage
    //   +8  align  – required alignment (unused here, we always 8-align via malloc)
    //  +16  ptr    – pointer to allocated storage (0 if not yet allocated)
    //  +24  templ  – pointer to initializer template (NULL → zero-init)

    pub fn sys_emutls_get_address(obj: u64) -> i64 {
        if obj == 0 { return 0; }
        let (pid, _pml4) = pid_and_pml4();

        // Per-thread cache lookup. DO NOT consult obj+16 — that cell is in the
        // engine's .data and shared across all threads, which would alias TLS
        // between threads and cause call-thru-NULL crashes.
        {
            let table = EMUTLS_TABLE.lock();
            if let Some(&va) = table.get(&(pid, obj)) {
                return va as i64;
            }
        }

        // Allocate storage for this thread's copy of the TLS variable.
        let size = unsafe { *(obj as *const u64) };
        let sz = size.max(8);
        let ptr_va = sys_malloc(sz) as u64;
        if ptr_va == 0 { return 0; }
        // Copy template or zero-init.
        let templ = unsafe { *((obj + 24) as *const u64) };
        if templ != 0 {
            unsafe {
                core::ptr::copy_nonoverlapping(
                    templ as *const u8,
                    ptr_va as *mut u8,
                    sz as usize,
                );
            }
        } else {
            unsafe { core::ptr::write_bytes(ptr_va as *mut u8, 0, sz as usize); }
        }
        // Insert into per-thread cache. Intentionally DO NOT write obj+16.
        EMUTLS_TABLE.lock().insert((pid, obj), ptr_va);
        ptr_va as i64
    }

    pub fn sys_emutls_register_common(obj: u64, size: u64, align: u64, templ: u64) -> i64 {
        if obj == 0 { return 0; }
        unsafe {
            *(obj as *mut u64)          = size;
            *((obj + 8)  as *mut u64)   = align;
            *((obj + 16) as *mut u64)   = 0;    // legacy: clear cached ptr (unused now)
            *((obj + 24) as *mut u64)   = templ;
        }
        0
    }

    // ── System ────────────────────────────────────────────────────────────────

    pub fn sys_abort() -> ! {
        // Delegate to sys_exit so we never return into the dead process's
        // user code path. sys_exit performs the context switch to the next
        // runnable process (or halts if none), and is `-> i64` only because
        // it threads through the normal dispatch table; in practice it never
        // returns.
        let pid = crate::process::current_pid();
        log::error!("[sys_abort] pid={} aborting — dumping recent syscalls:", pid);
        super::dump_recent_syscalls(32);
        // Dump timerfd + epoll state to help diagnose event-loop starvation.
        super::dump_event_state();
        // Walk user-mode RBP frames to identify the abort call site.
        super::dump_user_backtrace(16);
        super::sys_exit((-6i64) as u64);
        loop {
            unsafe { core::arch::asm!("sti; hlt; cli", options(nomem, nostack)); }
        }
    }

    pub fn sys_sysconf(name: i32) -> i64 {
        match name {
            // _SC_PAGESIZE / _SC_PAGE_SIZE
            30 | 47 => 4096,
            // _SC_NPROCESSORS_ONLN / _SC_NPROCESSORS_CONF
            // Report 1 CPU: the Dart VM scales JIT worker thread count by CPU
            // count. With 2 CPUs it spawns 2 parallel JIT workers (pids 8 and 9)
            // that race on the class table, triggering ASSERT(previous_cid <
            // current_cid) in il.cc. Under our single-CPU cooperative scheduler
            // with a single BSP, 1 worker is both correct and sufficient.
            84 | 83 => 1,
            // _SC_PHYS_PAGES
            85 => 131072, // 512 MiB / 4096
            // _SC_CLK_TCK
            2 => 100,
            // _SC_OPEN_MAX
            4 => 256,
            // _SC_GETPW_R_SIZE_MAX
            70 => 1024,
            // _SC_GETGR_R_SIZE_MAX
            69 => 1024,
            _ => -1,
        }
    }

    pub fn sys_nanosleep(req: u64, _rem: u64) -> i64 {
        if req == 0 { return 0; }
        // Read tv_sec from req (offset 0) and tv_nsec (offset 8).
        let secs = unsafe { *(req as *const u64) };
        // Yield for approximately secs * 1000 iterations (very rough).
        for _ in 0..(secs * 1000).min(10000) {
            unsafe { core::arch::asm!("pause", options(nomem, nostack, preserves_flags)); }
        }
        0
    }

    /// Fake monotonic time: use TSC / 1000 as nanoseconds.
    fn read_tsc() -> u64 {
        let lo: u32;
        let hi: u32;
        unsafe { core::arch::asm!("rdtsc", out("eax") lo, out("edx") hi, options(nomem, nostack)); }
        ((hi as u64) << 32) | lo as u64
    }

    pub fn sys_gettimeofday(tv: u64, _tz: u64) -> i64 {
        if tv == 0 { return 0; }
        // tv_sec at offset 0, tv_usec at offset 8.
        let tsc = read_tsc() / 3_000; // ~3 GHz TSC → microseconds
        let sec  = 1_700_000_000u64 + tsc / 1_000_000;
        let usec = tsc % 1_000_000;
        unsafe {
            *(tv as *mut u64)       = sec;
            *((tv + 8) as *mut u64) = usec;
        }
        0
    }

    pub fn sys_clock_gettime(clock_id: i32, tp: u64) -> i64 {
        if tp == 0 { return -22; }
        let tsc_ns = read_tsc() / 3; // ~3 GHz → nanoseconds
        let (sec, nsec) = match clock_id {
            0 | 1 => { // CLOCK_REALTIME or CLOCK_MONOTONIC
                let secs = 1_700_000_000u64 + tsc_ns / 1_000_000_000;
                let ns   = tsc_ns % 1_000_000_000;
                (secs, ns)
            }
            _ => (0, tsc_ns),
        };
        unsafe {
            *(tp as *mut u64)       = sec;
            *((tp + 8) as *mut u64) = nsec;
        }
        0
    }

    pub fn sys_time(tloc: u64) -> i64 {
        let tsc = read_tsc() / 3_000_000_000; // seconds
        let t = 1_700_000_000u64 + tsc;
        if tloc != 0 { unsafe { *(tloc as *mut u64) = t; } }
        t as i64
    }

    pub fn sys_getrusage(_who: u64, usage: u64) -> i64 {
        if usage != 0 { unsafe { core::ptr::write_bytes(usage as *mut u8, 0, 144); } }
        0
    }

    pub fn sys_getcwd(buf: u64, size: u64) -> i64 {
        let path = b"/\0";
        if buf == 0 || size < 2 { return 0; }
        unsafe { core::ptr::copy_nonoverlapping(path.as_ptr(), buf as *mut u8, 2); }
        buf as i64
    }

    pub fn sys_gethostname(buf: u64, size: u64) -> i64 {
        let name = b"oscortex\0";
        let n = name.len().min(size as usize);
        if buf == 0 { return -1; }
        unsafe { core::ptr::copy_nonoverlapping(name.as_ptr(), buf as *mut u8, n); }
        0
    }

    pub fn sys_uname(buf: u64) -> i64 {
        // struct utsname: 6 fields × 65 bytes each = 390 bytes.
        if buf == 0 { return -1; }
        unsafe { core::ptr::write_bytes(buf as *mut u8, 0, 390); }
        let copy = |off: usize, s: &[u8]| {
            let n = s.len().min(64);
            unsafe { core::ptr::copy_nonoverlapping(s.as_ptr(), (buf + off as u64) as *mut u8, n); }
        };
        copy(0,   b"Linux");
        copy(65,  b"oscortex");
        copy(130, b"6.1.0");
        copy(195, b"#1 SMP");
        copy(260, b"x86_64");
        0
    }

    /// Return a static error string for the given errno.
    pub fn sys_strerror(errnum: i32) -> i64 {
        log::warn!("[strerror] errnum={}", errnum);
        // Embed static strings at a fixed kernel address.
        // The user gets a pointer to read-only kernel text — valid since
        // user processes can read the kernel text in our current model.
        let s: &'static [u8] = match errnum.unsigned_abs() {
            0  => b"Success\0",
            1  => b"Operation not permitted\0",
            2  => b"No such file or directory\0",
            11 => b"Resource temporarily unavailable\0",
            12 => b"Out of memory\0",
            13 => b"Permission denied\0",
            14 => b"Bad address\0",
            16 => b"Device or resource busy\0", // EBUSY
            17 => b"File exists\0",
            19 => b"No such device\0",
            22 => b"Invalid argument\0",
            28 => b"No space left on device\0",
            35 => b"Resource deadlock would occur\0", // EDEADLK
            38 => b"Function not implemented\0",
            _  => b"Unknown error\0",
        };
        s.as_ptr() as i64
    }

    pub fn sys_strerror_r(errnum: i32, buf: u64, n: u64) -> i64 {
        log::warn!("[strerror_r] errnum={} buf={:#x} n={}", errnum, buf, n);
        let s_ptr = sys_strerror(errnum) as u64;
        let s_len = sys_strlen(s_ptr) as u64 + 1;
        let copy = s_len.min(n);
        if buf != 0 && copy > 0 {
            sys_memcpy(buf, s_ptr, copy);
        }
        buf as i64
    }

    pub fn sys_passthrough_syscall(nr: u64, a0: u64, a1: u64, a2: u64) -> i64 {
        // Allow userspace `syscall(NR, ...)` to dispatch to our kernel.
        crate::syscall::dispatch_fast(nr, a0, a1, a2, 0, 0)
    }

    pub fn sys_getenv(_name_ptr: u64, _name_len: u64) -> i64 {
        // No environment variables in our OS.
        0 // NULL
    }

    fn ensure_locale_obj() -> u64 {
        let cur = LOCALE_OBJ.load(Ordering::Acquire);
        if cur != 0 { return cur; }

        let obj = sys_malloc(256) as u64;
        if obj == 0 { return 0; }
        unsafe {
            core::ptr::write_bytes(obj as *mut u8, 0, 256);
            *(obj as *mut u64) = obj;
            *((obj + 0x68) as *mut u64) = obj;
        }

        match LOCALE_OBJ.compare_exchange(0, obj, Ordering::AcqRel, Ordering::Acquire) {
            Ok(_) => {
                LOCALE_CURRENT.store(obj, Ordering::Release);
                obj
            }
            Err(existing) => existing,
        }
    }

    pub fn sys_setlocale(_category: u64, _locale_ptr: u64) -> i64 {
        // Return pointer to a stable locale string "C".
        static C_LOCALE: &[u8] = b"C\0";
        C_LOCALE.as_ptr() as i64
    }

    pub fn sys_uselocale(locale: u64) -> i64 {
        let prev = LOCALE_CURRENT.load(Ordering::Acquire);
        // locale_t special values:
        //   0   => query current locale
        //   -1  => use global locale
        let use_ptr = if locale == 0 || locale == u64::MAX {
            ensure_locale_obj()
        } else {
            locale
        };
        if use_ptr != 0 {
            LOCALE_CURRENT.store(use_ptr, Ordering::Release);
        }
        if prev != 0 { prev as i64 } else { ensure_locale_obj() as i64 }
    }

    pub fn sys_newlocale(_mask: u64, _name_ptr: u64, base: u64) -> i64 {
        // Some runtimes pass small sentinels here that are not real locale_t
        // pointers; only trust clearly pointer-like values.
        if base >= 0x10000 { return base as i64; }
        ensure_locale_obj() as i64
    }

    pub fn sys_freelocale(_locale: u64) -> i64 {
        // Locale objects are process-lifetime for now.
        0
    }

    // ── IO ────────────────────────────────────────────────────────────────────

    pub fn sys_open64(path_ptr: u64, _flags: u64, _mode: u64, _arg3: u64) -> i64 {
        // libc open64(path, flags, mode): arg1 is flags, NOT a path length.
        // Compute strlen ourselves so sys_open can read the user path.
        if path_ptr == 0 { return -14; } // EFAULT
        let path_len = sys_strlen(path_ptr) as u64 + 1; // include NUL
        crate::syscall::dispatch_fast(2, path_ptr, path_len, 0, 0, 0)
    }

    pub fn sys_lseek64(fd: u64, offset: i64, whence: i32) -> i64 {
        crate::syscall::dispatch_fast(8, fd, offset as u64, whence as u64, 0, 0)
    }

    pub fn sys_fopen64(path_ptr: u64, path_len: u64, _mode_ptr: u64, _mode_len: u64) -> i64 {
        // Open the VFS file via sys_open and return a tiny heap-allocated FILE*.
        // FILE layout: [i64 fd][u64 size][u64 pos]  = 24 bytes
        let path_end = if path_len > 0 { path_len } else { sys_strlen(path_ptr) as u64 + 1 };
        let fd = crate::syscall::dispatch_fast(2, path_ptr, path_end, 0, 0, 0);
        if fd < 0 { return 0; } // NULL on error
        // Query file size via fstat.
        let stat_buf = sys_malloc(144 + 16);
        if stat_buf == 0 { crate::syscall::dispatch_fast(3, fd as u64, 0, 0, 0, 0); return 0; }
        crate::syscall::dispatch_fast(5, fd as u64, stat_buf as u64, 0, 0, 0); // fstat
        let size = unsafe { *((stat_buf as u64 + 48) as *const u64) }; // st_size offset=48
        sys_free(stat_buf as u64);
        // Allocate FILE struct: [fd:i64][size:u64][pos:u64]
        let fp = sys_malloc(24);
        if fp == 0 { crate::syscall::dispatch_fast(3, fd as u64, 0, 0, 0, 0); return 0; }
        unsafe {
            *((fp as u64) as *mut i64)      = fd;
            *((fp as u64 + 8) as *mut u64)  = size;
            *((fp as u64 + 16) as *mut u64) = 0; // pos
        }
        fp
    }

    pub fn sys_fclose(fp: u64) -> i64 {
        if fp == 0 { return -1; }
        let fd = unsafe { *(fp as *const i64) };
        sys_free(fp);
        crate::syscall::dispatch_fast(3, fd as u64, 0, 0, 0, 0) // sys_close
    }

    pub fn sys_fread(buf: u64, size: u64, count: u64, fp: u64) -> i64 {
        if fp == 0 || buf == 0 { return 0; }
        let fd = unsafe { *(fp as *const i64) } as u64;
        let total = size * count;
        let r = crate::syscall::dispatch_fast(0, fd, buf, total, 0, 0); // sys_read
        if r <= 0 { return 0; }
        // Update pos in FILE struct.
        unsafe { *((fp + 16) as *mut u64) += r as u64; }
        if size == 0 { 0 } else { r / size as i64 }
    }

    pub fn sys_fwrite(buf: u64, size: u64, count: u64, fp: u64) -> i64 {
        let fd: u64 = if fp == 0 { 1 } else { unsafe { *(fp as *const i64) as u64 } };
        let total = size * count;
        let r = crate::syscall::dispatch_fast(1, fd, buf, total, 0, 0);
        if r < 0 { 0 } else { r / size as i64 }
    }

    pub fn sys_fseek(fp: u64, offset: i64, whence: i32) -> i64 {
        if fp == 0 { return -1; }
        let fd = unsafe { *(fp as *const i64) } as u64;
        let new_pos = crate::syscall::dispatch_fast(8, fd, offset as u64, whence as u64, 0, 0);
        if new_pos < 0 { return -1; }
        unsafe { *((fp + 16) as *mut u64) = new_pos as u64; }
        0
    }

    pub fn sys_ftell(fp: u64) -> i64 {
        if fp == 0 { return -1; }
        let pos = unsafe { *((fp + 16) as *const u64) };
        pos as i64
    }

    pub fn sys_fgets(buf: u64, size: i32, fp: u64) -> i64 {
        if fp == 0 || buf == 0 || size <= 0 { return 0; }
        let fd = unsafe { *(fp as *const i64) } as u64;
        let cap = (size - 1).max(0) as u64;
        // Read byte by byte until newline or EOF.
        let mut n: u64 = 0;
        while n < cap {
            let mut ch: u8 = 0;
            let r = crate::syscall::dispatch_fast(0, fd, &mut ch as *mut u8 as u64, 1, 0, 0);
            if r <= 0 { break; }
            unsafe { *((buf + n) as *mut u8) = ch; }
            n += 1;
            if ch == b'\n' { break; }
        }
        unsafe { *((buf + n) as *mut u8) = 0; }
        if n == 0 { 0 } else { buf as i64 }
    }

    pub fn sys_fileno(fp: u64) -> i64 {
        if fp == 0 { return -1; }
        unsafe { *(fp as *const i64) }
    }

    pub fn sys_feof(fp: u64) -> i64 {
        if fp == 0 { return 1; }
        let fd  = unsafe { *(fp as *const i64) } as usize;
        let tbl = super::OPEN_FILES.lock();
        if fd < super::MAX_OPEN_FILES && tbl[fd].used {
            if tbl[fd].offset >= tbl[fd].data.len() as u64 { 1 } else { 0 }
        } else {
            1
        }
    }

    pub fn sys_ferror(_fp: u64) -> i64 { 0 }

    pub fn sys_clearerr(_fp: u64) -> i64 { 0 }

    pub fn sys_rewind(fp: u64) -> i64 {
        sys_fseek(fp, 0, 0 /* SEEK_SET */)
    }

    pub fn sys_fflush(_fp: u64) -> i64 { 0 }

    pub fn sys_stat(path_ptr: u64, path_len: u64, stat_ptr: u64) -> i64 {
        // The POSIX `stat()` ABI is `stat(const char *path, struct stat *buf)`.
        // When called via the libc trampoline (syscall 0x441), `arg1` is the
        // stat buffer pointer and `arg2` is unused — there is no length. Detect
        // that case by checking whether `stat_ptr` is zero, and treat
        // `path_len` as the real stat buffer.
        let (mut real_path_len, real_stat_ptr) = if stat_ptr == 0 {
            (0u64, path_len)
        } else {
            (path_len, stat_ptr)
        };
        // If no path length was supplied, compute it from the NUL terminator.
        if real_path_len == 0 && path_ptr != 0 {
            let mut len: usize = 0;
            unsafe {
                let p = path_ptr as *const u8;
                while len < 4096 && *p.add(len) != 0 { len += 1; }
            }
            real_path_len = len as u64;
        }
        // Open the file, stat it, close it.
        let fd = crate::syscall::dispatch_fast(2, path_ptr, real_path_len, 0, 0, 0);
        if fd < 0 { return -2; } // ENOENT
        let r = crate::syscall::dispatch_fast(5, fd as u64, real_stat_ptr, 0, 0, 0);
        crate::syscall::dispatch_fast(3, fd as u64, 0, 0, 0, 0);
        r
    }

    fn format_into(
        buf: u64,
        size: u64,
        fmt_ptr: u64,
        mut next_int: impl FnMut() -> u64,
    ) -> i64 {
        let fmt_valid = fmt_ptr >= 0x10000 && fmt_ptr < 0x0000_8000_0000_0000;
        let buf_valid = buf  >= 0x10000 && buf  < 0x0000_8000_0000_0000;
        if !fmt_valid || !buf_valid { return 0; }

        let cur_cr3 = crate::arch::memory::read_cr3() & 0x000f_ffff_ffff_f000;

        // Safe C-string reader from user address space.
        let read_cstr = |p: u64, out: &mut [u8]| -> usize {
            if p < 0x10000 || p >= 0x0000_8000_0000_0000 { return 0; }
            if crate::mm::paging::translate_user_page(cur_cr3, p & !0xfff).is_none() { return 0; }
            let mut n = 0usize;
            unsafe {
                let pp = p as *const u8;
                while n < out.len() {
                    let c = *pp.add(n);
                    if c == 0 { break; }
                    out[n] = c;
                    n += 1;
                }
            }
            n
        };

        // Read the format string.
        let mut fbuf = [0u8; 256];
        let flen = read_cstr(fmt_ptr, &mut fbuf);
        let fmt = &fbuf[..flen];

        let max_out = ((size as usize).saturating_sub(1)).min(511);
        let mut out_buf = [0u8; 512];
        let mut out_len = 0usize;
        let mut i = 0usize;

        while i < flen && out_len < max_out {
            let c = fmt[i];
            if c != b'%' || i + 1 >= flen {
                out_buf[out_len] = c;
                out_len += 1;
                i += 1;
                continue;
            }

            // Skip format spec modifiers
            let mut j = i + 1;
            while j < flen && matches!(fmt[j], b'l'|b'h'|b'z'|b'j'|b't'|b'L'|b'0'..=b'9'|b'.'|b'-'|b'+'|b' '|b'#') {
                j += 1;
            }
            if j >= flen { break; }
            let conv = fmt[j];
            let mut tmp = [0u8; 32];
            match conv {
                b'd' | b'i' => {
                    let v = next_int() as i64;
                    let mut n = 0usize;
                    let (neg, mut u) = if v < 0 { (true, (v.wrapping_neg()) as u64) } else { (false, v as u64) };
                    if u == 0 { tmp[n] = b'0'; n += 1; }
                    while u > 0 { tmp[n] = b'0' + (u % 10) as u8; n += 1; u /= 10; }
                    if neg { tmp[n] = b'-'; n += 1; }
                    tmp[..n].reverse();
                    let limit = (max_out - out_len).min(n);
                    out_buf[out_len..out_len + limit].copy_from_slice(&tmp[..limit]);
                    out_len += limit;
                }
                b'u' => {
                    let mut u = next_int();
                    let mut n = 0usize;
                    if u == 0 { tmp[n] = b'0'; n += 1; }
                    while u > 0 { tmp[n] = b'0' + (u % 10) as u8; n += 1; u /= 10; }
                    tmp[..n].reverse();
                    let limit = (max_out - out_len).min(n);
                    out_buf[out_len..out_len + limit].copy_from_slice(&tmp[..limit]);
                    out_len += limit;
                }
                b'x' | b'X' | b'p' => {
                    let mut u = next_int();
                    if conv == b'p' {
                        if out_len + 2 <= max_out {
                            out_buf[out_len..out_len+2].copy_from_slice(b"0x");
                            out_len += 2;
                        }
                    }
                    let hexd = if conv == b'X' { b"0123456789ABCDEF" } else { b"0123456789abcdef" };
                    let mut n = 0usize;
                    if u == 0 { tmp[n] = b'0'; n += 1; }
                    while u > 0 { tmp[n] = hexd[(u & 0xF) as usize]; n += 1; u >>= 4; }
                    tmp[..n].reverse();
                    let limit = (max_out - out_len).min(n);
                    out_buf[out_len..out_len + limit].copy_from_slice(&tmp[..limit]);
                    out_len += limit;
                }
                b's' => {
                    let p = next_int();
                    let mut sbuf = [0u8; 256];
                    let sn = read_cstr(p, &mut sbuf);
                    let limit = (max_out - out_len).min(sn);
                    out_buf[out_len..out_len + limit].copy_from_slice(&sbuf[..limit]);
                    out_len += limit;
                }
                b'c' => {
                    let v = next_int() as u8;
                    out_buf[out_len] = v;
                    out_len += 1;
                }
                b'%' => {
                    out_buf[out_len] = b'%';
                    out_len += 1;
                }
                _ => {
                    out_buf[out_len] = b'%';
                    out_len += 1;
                    if out_len < max_out {
                        out_buf[out_len] = conv;
                        out_len += 1;
                    }
                }
            }
            i = j + 1;
        }

        out_buf[out_len] = 0;

        let fmt_str = core::str::from_utf8(fmt).unwrap_or("<non-utf8>");
        let is_error = flen >= 5 && (fmt_str.contains("error") || fmt_str.contains("expected") || fmt_str.contains("assert") || fmt_str.contains("fail") || fmt_str.contains("FATAL"));
        if is_error {
            static SNPRINTF_ERR_COUNT: core::sync::atomic::AtomicU32 =
                core::sync::atomic::AtomicU32::new(0);
            let n = SNPRINTF_ERR_COUNT.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
            if n < 16 {
                let msg = core::str::from_utf8(&out_buf[..out_len]).unwrap_or("<non-utf8>");
                log::error!("[snprintf-error] pid={} #{}: msg=\"{}\" fmt=\"{}\"",
                    crate::process::current_pid(), n, msg, fmt_str);
            }
        }

        // Write formatted result to the caller's buffer.
        unsafe {
            core::ptr::copy_nonoverlapping(out_buf.as_ptr(), buf as *mut u8, out_len + 1);
        }
        out_len as i64
    }

    pub fn sys_snprintf(buf: u64, size: u64, fmt_ptr: u64, first_vararg: u64, second_vararg: u64) -> i64 {
        let user_rsp = crate::arch::syscall::user_rsp();
        let third_vararg  = crate::arch::syscall::user_r9();
        let cur_cr3 = crate::arch::memory::read_cr3() & 0x000f_ffff_ffff_f000;
        let safe_qword = |off: u64| -> u64 {
            let addr = user_rsp.wrapping_add(off);
            if crate::mm::paging::translate_user_page(cur_cr3, addr & !0xfff).is_some() {
                unsafe { core::ptr::read_volatile(addr as *const u64) }
            } else { 0 }
        };
        let fourth_vararg = safe_qword(8);
        let fifth_vararg  = safe_qword(16);
        let varargs = [first_vararg, second_vararg, third_vararg, fourth_vararg, fifth_vararg];
        let mut varg_idx = 0usize;
        format_into(buf, size, fmt_ptr, || {
            let val = if varg_idx < varargs.len() { varargs[varg_idx] } else { 0 };
            varg_idx += 1;
            val
        })
    }

    pub fn sys_vsnprintf(buf: u64, size: u64, fmt_ptr: u64, ap: u64) -> i64 {
        let plausible_user = |p: u64| p != 0 && p < 0x0000_8000_0000_0000;
        let (mut gp_off, reg_save, mut ovf): (u32, u64, u64) =
            if plausible_user(ap) {
                unsafe {
                    let g = core::ptr::read_unaligned((ap as *const u32));
                    let r = core::ptr::read_unaligned((ap as *const u64).offset(2));
                    let o = core::ptr::read_unaligned((ap as *const u64).offset(1));
                    (g, r, o)
                }
            } else { (48, 0, 0) };

        let mut next_int = || -> u64 {
            if gp_off < 48 && plausible_user(reg_save) {
                let v = unsafe { core::ptr::read_unaligned((reg_save + gp_off as u64) as *const u64) };
                gp_off += 8;
                v
            } else if plausible_user(ovf) {
                let v = unsafe { core::ptr::read_unaligned(ovf as *const u64) };
                ovf += 8;
                v
            } else { 0 }
        };

        format_into(buf, size, fmt_ptr, next_int)
    }

    pub fn sys_printf(fmt_ptr: u64, _fmt_len: u64) -> i64 {
        let len = sys_strlen(fmt_ptr) as u64;
        crate::syscall::dispatch_fast(1, 1, fmt_ptr, len, 0, 0)
    }

    pub fn sys_puts(s: u64) -> i64 {
        let len = sys_strlen(s) as u64;
        crate::syscall::dispatch_fast(1, 1, s, len, 0, 0);
        crate::syscall::dispatch_fast(1, 1, b"\n".as_ptr() as u64, 1, 0, 0);
        0
    }

    pub fn sys_perror(msg: u64) -> i64 {
        // Log caller's return address (at top of user stack) to identify flutter call site.
        {
            static PERROR_TRACE: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
            let n = PERROR_TRACE.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
            if n < 4 {
                let rsp = crate::arch::syscall::user_rsp();
                let caller_rip = unsafe { *(rsp as *const u64) };
                let epoll_ret_rax = crate::process::get_saved_rax(crate::process::current_pid());
                log::warn!("[sys_perror] #{} pid={} msg={:#x} caller_ret={:#x} prev_epoll_rax={}",
                    n, crate::process::current_pid(), msg, caller_rip, epoll_ret_rax as i64);
            }
        }
        if msg != 0 {
            let mlen = sys_strlen(msg) as u64;
            crate::syscall::dispatch_fast(1, 2, msg, mlen, 0, 0);
            crate::syscall::dispatch_fast(1, 2, b": ".as_ptr() as u64, 2, 0, 0);
        }
        // Write ENOSYS strerror.
        let s = sys_strerror(38) as u64;
        let slen = sys_strlen(s) as u64;
        crate::syscall::dispatch_fast(1, 2, s, slen, 0, 0);
        crate::syscall::dispatch_fast(1, 2, b"\n".as_ptr() as u64, 1, 0, 0);
        0
    }

    pub fn sys_fprintf(fp: u64, fmt_ptr: u64, ap: u64) -> i64 {
        // Simplified vfprintf with minimal %i / %s / %d / %x expansion so
        // diagnostic messages (errno, strerror) are actually readable.
        //
        // C semantics for both fprintf(stream, fmt, ...) and vfprintf(stream,
        // fmt, va_list ap): the format string is NUL-terminated and the third
        // ABI slot is a va_list pointer — never a byte count. Earlier code
        // (mis-)used arg2 as a length, which made vfprintf hand its caller's
        // stack pointer to sys_write as the buffer length, then EFAULT, then
        // abort.
        //
        // SysV x86_64 va_list layout (struct __va_list_tag):
        //   off 0:   u32 gp_offset      (next int-reg byte offset 0..48)
        //   off 4:   u32 fp_offset
        //   off 8:   *mut u8 overflow_arg_area
        //   off 16:  *mut u8 reg_save_area
        if fmt_ptr == 0 { return 0; }
        let fd: u64 = if fp == 0 || fp == 1 { 1 }
                      else if fp == 2 { 2 }
                      else { unsafe { *(fp as *const i64) as u64 } };

        // Read va_list state if pointer is plausibly a user-mode pointer.
        let plausible_user = |p: u64| p != 0 && p < 0x0000_8000_0000_0000;
        let (mut gp_off, reg_save, mut ovf): (u32, u64, u64) =
            if plausible_user(ap) {
                unsafe {
                    let g = core::ptr::read_unaligned((ap as *const u32));
                    let r = core::ptr::read_unaligned((ap as *const u64).offset(2));
                    let o = core::ptr::read_unaligned((ap as *const u64).offset(1));
                    (g, r, o)
                }
            } else { (48, 0, 0) };

        // Pull next u64-sized int arg from the va_list. Integers consume
        // 8 bytes of gp regs first, then overflow.
        let mut next_int = || -> u64 {
            if gp_off < 48 && plausible_user(reg_save) {
                let v = unsafe { core::ptr::read_unaligned((reg_save + gp_off as u64) as *const u64) };
                gp_off += 8;
                v
            } else if plausible_user(ovf) {
                let v = unsafe { core::ptr::read_unaligned(ovf as *const u64) };
                ovf += 8;
                v
            } else { 0 }
        };

        // Format into a stack buffer (cap 512 bytes) then sys_write.
        let mut out = [0u8; 512];
        let mut olen: usize = 0;
        let mut push = |s: &[u8], olen: &mut usize, out: &mut [u8; 512]| {
            let n = s.len().min(out.len() - *olen);
            out[*olen..*olen + n].copy_from_slice(&s[..n]);
            *olen += n;
        };

        let fmt_len = sys_strlen(fmt_ptr) as usize;
        let fmt = unsafe { core::slice::from_raw_parts(fmt_ptr as *const u8, fmt_len) };
        let mut i = 0;
        while i < fmt.len() && olen < out.len() {
            let c = fmt[i];
            if c != b'%' || i + 1 >= fmt.len() {
                push(&[c], &mut olen, &mut out);
                i += 1;
                continue;
            }
            // Skip flags/width — minimal: read a single conversion char,
            // ignoring length modifiers ('l','ll','h','z').
            let mut j = i + 1;
            while j < fmt.len() && matches!(fmt[j], b'l'|b'h'|b'z'|b'j'|b't'|b'L'|b'0'..=b'9'|b'.'|b'-'|b'+'|b' '|b'#') {
                j += 1;
            }
            if j >= fmt.len() { break; }
            let conv = fmt[j];
            let mut tmp = [0u8; 32];
            match conv {
                b'd' | b'i' => {
                    let v = next_int() as i64;
                    let mut n = 0usize;
                    let (neg, mut u) = if v < 0 { (true, (v.wrapping_neg()) as u64) } else { (false, v as u64) };
                    if u == 0 { tmp[n] = b'0'; n += 1; }
                    while u > 0 { tmp[n] = b'0' + (u % 10) as u8; n += 1; u /= 10; }
                    if neg { tmp[n] = b'-'; n += 1; }
                    tmp[..n].reverse();
                    push(&tmp[..n], &mut olen, &mut out);
                }
                b'u' => {
                    let mut u = next_int();
                    let mut n = 0usize;
                    if u == 0 { tmp[n] = b'0'; n += 1; }
                    while u > 0 { tmp[n] = b'0' + (u % 10) as u8; n += 1; u /= 10; }
                    tmp[..n].reverse();
                    push(&tmp[..n], &mut olen, &mut out);
                }
                b'x' | b'X' | b'p' => {
                    let mut u = next_int();
                    if conv == b'p' { push(b"0x", &mut olen, &mut out); }
                    let hexd = if conv == b'X' { b"0123456789ABCDEF" } else { b"0123456789abcdef" };
                    let mut n = 0usize;
                    if u == 0 { tmp[n] = b'0'; n += 1; }
                    while u > 0 { tmp[n] = hexd[(u & 0xF) as usize]; n += 1; u >>= 4; }
                    tmp[..n].reverse();
                    push(&tmp[..n], &mut olen, &mut out);
                }
                b's' => {
                    let p = next_int();
                    if plausible_user(p) {
                        let sl = sys_strlen(p) as usize;
                        let s = unsafe { core::slice::from_raw_parts(p as *const u8, sl) };
                        push(s, &mut olen, &mut out);
                    } else {
                        push(b"(null)", &mut olen, &mut out);
                    }
                }
                b'c' => {
                    let v = next_int() as u8;
                    push(&[v], &mut olen, &mut out);
                }
                b'%' => { push(b"%", &mut olen, &mut out); }
                _ => {
                    // Unknown — emit literally.
                    push(&fmt[i..=j], &mut olen, &mut out);
                }
            }
            i = j + 1;
        }
        if olen == 0 { return 0; }
        crate::syscall::dispatch_fast(1, fd, out.as_ptr() as u64, olen as u64, 0, 0)
    }

    // ── Signals ───────────────────────────────────────────────────────────────

    pub fn sys_sigfillset(set: u64) -> i64 {
        // Fill all bits in sigset_t (128 bytes on Linux x86_64).
        if set != 0 { unsafe { core::ptr::write_bytes(set as *mut u8, 0xFF, 128); } }
        0
    }

    // ── Wide chars ────────────────────────────────────────────────────────────

    pub fn sys_wcslen(s: u64) -> i64 {
        // wchar_t is 4 bytes on Linux.
        if s == 0 { return 0; }
        let mut len = 0i64;
        let mut p = s as *const u32;
        unsafe { while *p != 0 { p = p.add(1); len += 1; } }
        len
    }

    pub fn sys_mbrtowc(pwc: u64, s: u64, n: u64, _ps: u64) -> i64 {
        // C locale: 1-byte encoding.
        if s == 0 { return 0; }
        if n == 0 { return -2; } // incomplete sequence
        let b = unsafe { *(s as *const u8) };
        if b == 0 {
            if pwc != 0 { unsafe { *(pwc as *mut u32) = 0; } }
            return 0;
        }
        if pwc != 0 { unsafe { *(pwc as *mut u32) = b as u32; } }
        1
    }

    pub fn sys_mbsnrtowcs(dst: u64, srcp: u64, nmc: u64, len: u64, _ps: u64) -> i64 {
        if srcp == 0 { return -1; }
        let src_ptr = unsafe { *(srcp as *const u64) };
        if src_ptr == 0 { return 0; }

        let mut src = src_ptr as *const u8;
        let mut converted: u64 = 0;
        let mut remaining = nmc;
        while remaining > 0 {
            let b = unsafe { *src };
            if b == 0 {
                unsafe { *(srcp as *mut u64) = 0; }
                break;
            }
            if dst != 0 {
                if converted >= len { break; }
                unsafe { *((dst as *mut u32).add(converted as usize)) = b as u32; }
            }
            src = unsafe { src.add(1) };
            converted += 1;
            remaining -= 1;
        }

        if unsafe { *src } != 0 {
            unsafe { *(srcp as *mut u64) = src as u64; }
        }
        converted as i64
    }

    pub fn sys_mbsrtowcs(dst: u64, srcp: u64, len: u64, ps: u64) -> i64 {
        // Unbounded input bytes for C locale path.
        sys_mbsnrtowcs(dst, srcp, u64::MAX / 2, len, ps)
    }

    pub fn sys_mbtowc(pwc: u64, s: u64, n: u64) -> i64 {
        if s == 0 { return 0; } // reset shift state
        sys_mbrtowc(pwc, s, n, 0)
    }

    pub fn sys_wcrtomb(s: u64, wc: u64, _ps: u64) -> i64 {
        // C locale: only single-byte characters.
        if s == 0 { return 1; } // state reset query
        let ch = wc as u32;
        if ch > 0xFF { return -1; }
        unsafe { *(s as *mut u8) = ch as u8; }
        1
    }

    pub fn sys_wmemchr(s: u64, wc: u64, n: u64) -> i64 {
        if s == 0 || n == 0 { return 0; }
        let target = wc as u32;
        let p = s as *const u32;
        for i in 0..(n as usize) {
            let cur = unsafe { *p.add(i) };
            if cur == target {
                return (s + (i as u64) * 4) as i64;
            }
        }
        0
    }

    // ── Futex helpers ─────────────────────────────────────────────────────────

    fn sys_futex_wake(addr: u64, count: u32) -> i64 {
        // Dispatch to the existing futex syscall.
        crate::syscall::dispatch_fast(0x39D, addr, 129, count as u64, 0, 0) // FUTEX_WAKE = 1
    }
}

// ── Dispatcher ────────────────────────────────────────────────────────────────

/// Fast SYSCALL entry path (called from assembly stub).
#[unsafe(no_mangle)]
pub extern "C" fn dispatch_fast(number: u64, arg0: u64, arg1: u64, arg2: u64, arg3: u64, arg4: u64) -> i64 {
    let user_rip = crate::arch::syscall::user_rip();
    record_syscall_trace(number, arg0, arg1, arg2, user_rip);

    // Trace the first 32 syscalls made by pid=7 so we can see exactly which
    // call triggers "Poll failed:" and from where.
    {
        static PID7_TRACE: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
        if crate::process::current_pid() == 7 {
            let n = PID7_TRACE.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
            let _ = n;
        }
    }

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
            let user_rsp = crate::arch::syscall::user_rsp();
            // Place FS near current user stack top; keep room for small offsets.
            let bootstrap_fs = user_rsp.saturating_sub(0x200);
            if bootstrap_fs != 0 {
                crate::arch::cpu::set_fs_base(bootstrap_fs);
                crate::process::set_proc_fs_base(cur_pid, bootstrap_fs);
                // Log per-pid bootstrap so we can verify each pthread gets a
                // distinct TLS area. Cap at first 8 to keep logs small.
                static FS_BOOT_COUNT: core::sync::atomic::AtomicU32 =
                    core::sync::atomic::AtomicU32::new(0);
                let n = FS_BOOT_COUNT.fetch_add(1, Ordering::AcqRel);
                if n < 8 {
                    log::warn!(
                        "[syscall] bootstrapped FS base pid={} fs={:#x} rsp={:#x}",
                        cur_pid,
                        bootstrap_fs,
                        user_rsp
                    );
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

    match number {
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
            let hint = if is_fixed { arg0 } else { 0 };
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
        202 => sys_futex(arg0, arg1 as u32, arg2 as u32), // Linux SYS_futex — libc++ std::future uses this directly
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

        // Phase 38 — .oscapp app registry
        eabi::SYS_APP_INSTALL   => sys_app_install(arg0, arg1, arg2),
        eabi::SYS_APP_LIST      => sys_app_list(arg0, arg1),
        eabi::SYS_APP_LAUNCH    => sys_app_launch(arg0, arg1),
        eabi::SYS_APP_UNINSTALL => sys_app_uninstall(arg0),

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
        0x39D => sys_futex(arg0, arg1 as u32, arg2 as u32),

        // Phase 31 Slice C — threads
        eabi::SYS_THREAD_CREATE => sys_thread_create(arg0, arg1, arg2, arg3),
        eabi::SYS_THREAD_EXIT   => sys_thread_exit(arg0),
        eabi::SYS_THREAD_JOIN   => sys_thread_join(arg0),

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
        0x3D7 => posix::sys_pthread_once(arg0, arg1),
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
            let (base, size) = crate::process::get_user_stack_bounds(tid);
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
        0x412 => 0, // madvise noop
        0x413 => 0, // dladdr stub
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
            let mut s: u64 = unsafe { core::arch::x86_64::_rdtsc() } ^ 0xA5A5_F00D_DEAD_BEEFu64;
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
    }
}

/// Legacy INT 0x80 syscall path.
pub fn dispatch_legacy() {
    // TODO: read registers from saved context and dispatch.
}

