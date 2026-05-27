use super::state::{
    CondWaitState, COND_WAIT_STATE, FUTEX_WAITERS, SYSCALL_TRACE_BUF, SYSCALL_TRACE_HEAD,
    SYSCALL_TRACE_DEPTH, SyscallTraceEntry,
};
use super::poll::{monotonic_ns, EPOLL_TABLE, TIMERFD_TABLE};
use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};

pub fn debug_dump_sync_states() {
    let safe_read_u32 = |ptr: u64| -> Option<u32> {
        if ptr < 0x1000 || ptr > 0x7fff_ffff_ffff { return None; }
        unsafe { Some(*(ptr as *const u32)) }
    };

    if let Some(cws) = COND_WAIT_STATE.try_lock() {
        if !cws.is_empty() {
            log::info!("=== Cond Wait States ===");
            for (&pid, state) in cws.iter() {
                match state {
                    CondWaitState::Waiting { cond, mutex, seq, timeout_ns } => {
                        let c_val = safe_read_u32(*cond).map_or(-1, |v| v as i32);
                        let m_val = safe_read_u32(*mutex).map_or(-1, |v| v as i32);
                        log::info!("  pid={}: Waiting {{ cond: {:#x} (val={}), mutex: {:#x} (val={}), seq: {}, timeout_ns: {} }}",
                            pid, cond, c_val, mutex, m_val, seq, timeout_ns);
                    }
                    CondWaitState::AcquiringMutex { mutex, timed_out } => {
                        let m_val = safe_read_u32(*mutex).map_or(-1, |v| v as i32);
                        log::info!("  pid={}: AcquiringMutex {{ mutex: {:#x} (val={}), timed_out: {} }}",
                            pid, mutex, m_val, timed_out);
                    }
                }
            }
        }
    }
    if let Some(fw) = FUTEX_WAITERS.try_lock() {
        if !fw.is_empty() {
            log::info!("=== Futex Waiters ===");
            for (&addr, pids) in fw.iter() {
                let val = safe_read_u32(addr).map_or(-1, |v| v as i32);
                log::info!("  addr={:#x} (val={}): {:?}", addr, val, pids);
            }
        }
    }
}

// ── Post-exit syscall trace window ────────────────────────────────────────────
pub(crate) const POSTEXIT_TRACE_LIMIT: u32 = 5000;
pub(crate) static POSTEXIT_TRACE_ACTIVE: core::sync::atomic::AtomicBool =
    core::sync::atomic::AtomicBool::new(false);
pub(crate) static POSTEXIT_TRACE_COUNT: AtomicU32 = AtomicU32::new(0);

pub(crate) fn record_syscall_trace(nr: u64, a0: u64, a1: u64, a2: u64, rip: u64) {
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
