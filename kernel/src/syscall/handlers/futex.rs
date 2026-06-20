use super::fd::{read_user_bytes, sys_exit};
use crate::syscall::poll::{force_wake_all_task_runners, monotonic_ns};
use crate::syscall::state::{
    CondWaitState, COND_WAIT_STATE, FUTEX_PENDING_WAKES, FUTEX_WAITERS, SYSCALL_TRACE_DEPTH,
    WM_WAITER_DEADLINE, WM_WAITER_PID,
};
use crate::syscall::trace::dump_recent_syscalls;
use crate::syscall::trace::POSTEXIT_TRACE_COUNT;
use crate::syscall::wait::{futex_waiter_present, futex_waiter_remove};
use alloc::collections::BTreeMap;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU32, Ordering};

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
pub(crate) fn cond_miss_bridge(cond: u64, wake_count: u32) -> u32 {
    let mut total = 0u32;
    // Phase A: try the hardcoded "well-known" pid1/worker/handoff futex
    // addresses (kept for back-compat with earlier surgical fixes).
    for &addr in &[FUTEX_ADDR_PID1_WAIT, FUTEX_ADDR_WORKER_WAIT] {
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
            // Keep sibling fallback available for broadcast paths, but wake
            // only one waiter per futex address to avoid wake storms.
            let bridge_wake_count = if wake_count > 1 { 1 } else { wake_count };
            // Snapshot waiter table addresses.
            let addrs: alloc::vec::Vec<(u64, bool)> = {
                let t = FUTEX_WAITERS.lock();
                let cond_states = COND_WAIT_STATE.lock();
                t.iter()
                    .filter_map(|(addr, waiters)| {
                        if *addr == cond || *addr == FUTEX_ADDR_HANDOFF {
                            return None;
                        }
                        // Skip mutex re-lock waiters (cond already signaled).
                        let only_relock = waiters.iter().all(|&wpid| {
                            matches!(
                                cond_states.get(&wpid),
                                Some(&CondWaitState::AcquiringMutex { .. })
                            )
                        });
                        if only_relock {
                            return None;
                        }
                        // Wake sibling cond waiters parked on this futex addr.
                        // Do not bridge to the handoff word (0x338000070): spurious
                        // wakes there inflate seq and stall pids 5/6 without helping paint.
                        let has_sibling = waiters.iter().any(|&wpid| {
                            if !siblings.contains(&wpid) {
                                return false;
                            }
                            match cond_states.get(&wpid) {
                                Some(&CondWaitState::Waiting { cond: wcond, .. }) => wcond == *addr,
                                Some(&CondWaitState::AcquiringMutex { .. }) => false,
                                None => true,
                            }
                        });
                        // Confirmed glibc cond_t iff a sibling is actually `Waiting` on THIS
                        // addr. Raw-futex waiters (the `None` cond-state branch above) still
                        // get woken, but must NOT receive the glibc __wakeup_seq(+16)/
                        // __broadcast_seq(+40) pokes — that addr is not a pthread_cond_t and
                        // the writes stomp its heap (suspected interact-freeze corruption).
                        let is_glibc_cond = waiters.iter().any(|&wpid| {
                            siblings.contains(&wpid)
                                && matches!(cond_states.get(&wpid),
                                    Some(&CondWaitState::Waiting { cond: wcond, .. }) if wcond == *addr)
                        });
                        if has_sibling {
                            Some((*addr, is_glibc_cond))
                        } else {
                            None
                        }
                    })
                    .collect()
            };
            for (addr, is_glibc_cond) in addrs {
                let bridged = futex_wake_waiters(addr, bridge_wake_count);
                if bridged > 0 {
                    total = total.saturating_add(bridged as u32);
                    // Bump the target condvar's seq so the woken thread sees
                    // `cur_seq != seq` and exits cond_wait instead of re-sleeping.
                    // Without this, bridge wakes are spurious and the thread goes
                    // right back to sleep because its saved seq == current seq.
                    //
                    // Also update glibc pthread_cond_t __wakeup_seq (at addr+16)
                    // so threads using glibc's pthread_cond_wait see a proper
                    // signal and exit their re-check loop.
                    if is_glibc_cond && addr >= 0x1000 && addr < 0x0000_8000_0000_0000 {
                        let cur_cr3 = crate::arch::memory::read_cr3() & 0x000f_ffff_ffff_f000;
                        if crate::mm::paging::translate_user_page(cur_cr3, addr & !0xfff).is_some() {
                            // Custom condvar seq bump (our protocol)
                            let atom = unsafe { &*(addr as *const core::sync::atomic::AtomicU32) };
                            atom.fetch_add(1, core::sync::atomic::Ordering::Release);
                            // glibc __wakeup_seq at addr+16 (u64, glibc 2.17 NPTL layout)
                            // glibc __broadcast_seq at addr+40 (u32)
                            if addr + 48 < 0x0000_8000_0000_0000 {
                                if crate::mm::paging::translate_user_page(cur_cr3, (addr+16) & !0xfff).is_some() {
                                    let wakeup_ptr = (addr + 16) as *mut u64;
                                    unsafe { *wakeup_ptr = (*wakeup_ptr).wrapping_add(1); }
                                    let bcast_ptr = (addr + 40) as *mut u32;
                                    unsafe { *bcast_ptr = (*bcast_ptr).wrapping_add(1); }
                                    // Also wake glibc's futex word at addr+4
                                    let _ = futex_wake_waiters(addr + 4, bridge_wake_count);
                                }
                            }
                        }
                    }
                    static COND_BRIDGE_SIB_LOG: core::sync::atomic::AtomicU32 =
                        core::sync::atomic::AtomicU32::new(0);
                    let n = COND_BRIDGE_SIB_LOG.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
                    if n < 64 || n % 512 == 0 {
                        log::info!(
                            "[cond-bridge-sib] #{} cond={:#x} -> futex={:#x} wake_count={} bridged={}",
                            n,
                            cond,
                            addr,
                            bridge_wake_count,
                            bridged
                        );
                    }
                }
            }
        }
    }

    total
}

/// When an engine runner (pid 2–7) spins on zero-wake `NotifyAll`, wake sibling
/// threads parked on *other* cond addresses so init can proceed.
pub(crate) fn engine_broadcast_storm_wake(broadcaster: u32, cond: u64) -> u32 {
    static LAST_COND: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
    static STORM_COUNT: AtomicU32 = AtomicU32::new(0);
    if cond != LAST_COND.load(Ordering::Relaxed) {
        LAST_COND.store(cond, Ordering::Relaxed);
        STORM_COUNT.store(0, Ordering::Relaxed);
        return 0;
    }
    let c = STORM_COUNT.fetch_add(1, Ordering::Relaxed);
    if c < 3 {
        return 0;
    }
    STORM_COUNT.store(0, Ordering::Relaxed);

    let siblings = crate::process::sibling_pids(broadcaster);
    if siblings.len() <= 1 {
        return 0;
    }
    let targets: Vec<(u32, u64)> = {
        let t = COND_WAIT_STATE.lock();
        t.iter()
            .filter_map(|(&pid, st)| {
                if !siblings.contains(&pid) || pid == broadcaster {
                    return None;
                }
                match st {
                    CondWaitState::Waiting { cond: wcond, .. } if *wcond != cond => {
                        Some((pid, *wcond))
                    }
                    _ => None,
                }
            })
            .collect()
    };
    let mut total = 0u32;
    for (pid, wcond) in targets {
        let n = futex_wake_waiters(wcond, i32::MAX as u32);
        if n > 0 {
            total = total.saturating_add(n as u32);
            crate::process::set_state(pid, crate::process::ProcState::Running);
        }
    }
    if total > 0 {
        static STORM_WAKE_LOG: AtomicU32 = AtomicU32::new(0);
        let n = STORM_WAKE_LOG.fetch_add(1, Ordering::Relaxed);
        if n < 8 {
            log::warn!(
                "[engine-bcast-storm] #{} pid={} cond={:#x} storm_wakes={}",
                n,
                broadcaster,
                cond,
                total
            );
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

/// SMP M2: the PHYSICAL address backing a process's futex word at virtual `addr`.
/// Two contexts share a futex iff this matches — the correct, address-space-
/// independent identity (Redox blueprint #4). Returns None if the page isn't
/// mapped in that process (caller then falls back to group-leader scoping).
fn futex_phys_of(pid: u32, addr: u64) -> Option<u64> {
    let p4 = crate::process::pml4_phys_of(pid)?;
    let frame = crate::mm::paging::translate_user_page(p4, addr & !0xFFF)?;
    Some(frame | (addr & 0xFFF))
}

pub(crate) fn futex_wake_waiters(addr: u64, count: u32) -> i64 {
    // CRITICAL: FUTEX_WAITERS is keyed by the userspace futex/mutex ADDRESS only,
    // but that address is process-local. The shell (pid 1) and any launched app
    // (e.g. pid 10) are distinct address spaces that load the same libc / Flutter
    // engine .so at IDENTICAL virtual addresses, so their internal pthread
    // mutex/cond addresses COLLIDE in this table. Waking the first `count` waiters
    // blindly can wake — and remove — the WRONG process's waiter, leaving the
    // intended one parked forever (the post-app-launch render freeze: a render
    // thread stuck in pthread_mutex_lock while the unlock woke a sibling-process
    // waiter instead). Only wake waiters that share the caller's address space.
    // (Kernel/ISR context, pid 0, has no group and wakes everyone — preserves the
    // force-wake bring-up path.)
    let caller = crate::process::current_pid();
    let caller_grp = if caller == 0 { 0 } else { crate::process::get_group_leader(caller) };
    // SMP M2: prefer PHYSICAL-address identity over group-leader scoping. The
    // caller's physical futex address is the authoritative key — two contexts
    // share the futex iff they map `addr` to the same physical page (correct even
    // across cores, and across the shell/app shared-VA collision). Kernel/ISR
    // context (caller==0) still wakes everyone (the bring-up force-wake path).
    let caller_phys = if caller == 0 { None } else { futex_phys_of(caller, addr) };
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
        let mut woke = Vec::with_capacity((count as usize).min(waiters.len()));
        let mut i = 0;
        while i < waiters.len() && woke.len() < count as usize {
            let w = waiters[i];
            // Physical-address identity is authoritative when both resolve; fall
            // back to group-leader scoping only if a translation fails (so we
            // never DROP a wake the old path would have allowed).
            let same_space = if caller_grp == 0 {
                true
            } else if let (Some(cp), Some(wp)) = (caller_phys, futex_phys_of(w, addr)) {
                cp == wp
            } else {
                crate::process::get_group_leader(w) == caller_grp
            };
            if same_space {
                woke.push(w);
                waiters.remove(i);
            } else {
                i += 1;
            }
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
        // Guarded unblock (foundation blueprint Step 3): flip the waiter
        // Blocked→Running ONLY, idempotent. This is the wake side of the atomic
        // WAIT — it must NOT force Running over an unrelated state. Critically,
        // when a wake races a thread that is between its (already-committed)
        // waiter push and its try_block, this leaves the thread Running so its
        // try_block returns false and it skips sleeping (the lost-wakeup fix).
        crate::process::try_unblock(*pid);
    }

    n as i64
}

pub(crate) fn sys_futex(uaddr: u64, op: u32, val: u32, sys_nr: u64) -> i64 {
    let op_base = op & 0x7F; // strip FUTEX_PRIVATE_FLAG etc.
    match op_base {
        FUTEX_WAIT => {
            if uaddr < 0x1000 || uaddr & 3 != 0 { return -22; } // EINVAL: invalid/unaligned

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

            // ── ATOMIC WAIT (foundation blueprint Step 3) ──────────────────────
            // Value-check, enqueue, and the Running→Blocked transition all happen
            // under ONE FUTEX_WAITERS critical section. Previously the value-check
            // and the enqueue were SEPARATE lock acquisitions, so a FUTEX_WAKE
            // landing in the gap found no waiter and was LOST — the thread then
            // parked forever (a face of the post-app-launch render freeze). Now a
            // wake can only be processed either entirely before we hold the lock
            // (→ value changed under the lock → EAGAIN, no park) or entirely after
            // we release it (→ our waiter record is already present, so the wake
            // sees us and flips us Running). `try_block` is the linchpin: if a wake
            // already flipped us out of Running between the push and here, it
            // returns false and we skip sleeping. The lock is DROPPED before any
            // cooperative hand-off / halt below — never held across the yield (that
            // would strand it cross-core; the M6 PTABLE_LOCK-desync class).
            let blocked = {
                let mut table = FUTEX_WAITERS.lock();
                // Re-read the futex word UNDER the lock so the compare and the
                // enqueue are not separated by a wake.
                let cur = unsafe { core::ptr::read_volatile(uaddr as *const u32) };
                if cur != val {
                    // Value changed before we could sleep — no waiter was queued,
                    // so nothing to remove. Drop the lock and report EAGAIN.
                    drop(table);
                    return -11; // EAGAIN
                }
                // Enqueue our waiter record (inline; we already hold the lock that
                // futex_waiter_add would otherwise take).
                let waiters = table.entry(uaddr).or_insert_with(Vec::new);
                if !waiters.contains(&pid) {
                    waiters.push(pid);
                }
                // Running→Blocked-ONLY. False ⇒ a wake already flipped our state
                // (it ran after our push became visible) ⇒ we must NOT sleep.
                let did_block = crate::process::try_block(pid);
                drop(table);
                did_block
            };

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

            // A wake landed between our push and the block (try_block saw us
            // already non-Running). The wake won: do not sleep. Remove our (now
            // stale) waiter record and return success so user code re-tests its
            // predicate. This is the LOAD-BEARING half — without it we would park
            // on a state a wake already cleared and lose the wakeup.
            if !blocked {
                futex_waiter_remove(uaddr, pid);
                if futex_addr_is_target(uaddr) {
                    futex_trace_targets("wait-woke-early", pid, uaddr, val);
                }
                return 0;
            }

            // Try to cooperatively hand the CPU to a sibling user thread that
            // is runnable. This is the only path by which other threads of
            // PID 1 (e.g. the Flutter engine worker) can actually execute,
            // since the APIC timer ISR does not preempt user mode during
            // bring-up. We save our user return context (with RAX=0 so the
            // resumed syscall reports success) and SYSRET into the sibling. We
            // are ALREADY Blocked (try_block above), so no further set_state is
            // needed here. A subsequent FUTEX_WAKE will mark us Running again and
            // the next yielding thread will pick us up.
            if let Some(next) = crate::process::next_runnable_pid(pid) {
                if next != pid {
                    let urip = crate::arch::syscall::user_rip();
                    let ursp = crate::arch::syscall::user_rsp();
                    crate::process::save_return_context(pid, urip, ursp);
                    crate::process::save_full_user_gprs(pid);
                    crate::process::set_rax(pid, 0);
                    crate::process::save_xstate(pid);
                    crate::process::enter_user_by_pid_noreturn(next);
                }
            }

            // No sibling to switch into — sleep loop using sti; hlt; cli
            while unsafe { core::ptr::read_volatile(uaddr as *const u32) } == val
                && futex_waiter_present(uaddr, pid)
            {
                // Sleep with IRQs unmasked so the timer ISR fires + (aarch64) the
                // kernel-mode wake-assist runs. Was x86-only → aarch64 busy-spun.
                unsafe {
                    { crate::arch::enable_and_halt(); }
                }
                if let Some(next) = crate::process::next_runnable_pid(pid) {
                    if next != pid {
                        let urip = crate::arch::syscall::user_rip();
                        let ursp = crate::arch::syscall::user_rsp();
                        crate::process::save_return_context_reexec(pid, urip, ursp);
                        crate::process::save_full_user_gprs(pid);
                        crate::process::set_rax(pid, sys_nr);
                        crate::process::save_xstate(pid);
                        crate::process::set_state(pid, crate::process::ProcState::Blocked);
                        crate::process::enter_user_by_pid_noreturn(next);
                    }
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
            let pid = crate::process::current_pid();
            if pid == 0 { return 0; }
            if futex_pid1_postrun_bypass(uaddr, val) {
                return 0;
            }
            log::warn!("[futex-bitset] WAIT pid={} uaddr={:#x} val={}", pid, uaddr, val);

            // ── ATOMIC WAIT (foundation blueprint Step 3) ──────────────────────
            // Same register-under-lock-then-block ordering as FUTEX_WAIT above:
            // value-check + enqueue + Running→Blocked under ONE FUTEX_WAITERS
            // critical section, lock dropped before any hand-off. Bitset and
            // absolute-timeout semantics are intentionally LEFT unchanged here
            // (still ignored, block like plain FUTEX_WAIT) — those land in
            // Step 5; this step only closes the lost-wakeup window.
            let blocked = {
                let mut table = FUTEX_WAITERS.lock();
                let cur = unsafe { core::ptr::read_volatile(uaddr as *const u32) };
                if cur != val {
                    drop(table);
                    log::warn!("[futex-bitset] EAGAIN pid={} uaddr={:#x} cur={} expected={}", pid, uaddr, cur, val);
                    return -11; // EAGAIN
                }
                let waiters = table.entry(uaddr).or_insert_with(Vec::new);
                if !waiters.contains(&pid) {
                    waiters.push(pid);
                }
                let did_block = crate::process::try_block(pid);
                drop(table);
                did_block
            };

            // A wake won the race between our push and try_block — skip sleeping.
            if !blocked {
                futex_waiter_remove(uaddr, pid);
                return 0;
            }

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
                    // Already Blocked (try_block); no redundant set_state.
                    crate::process::enter_user_by_pid_noreturn(next);
                }
            }
            while unsafe { core::ptr::read_volatile(uaddr as *const u32) } == val
                && futex_waiter_present(uaddr, pid)
            {
                // Sleep with IRQs unmasked so the timer ISR fires + (aarch64) the
                // kernel-mode wake-assist runs. Was x86-only → aarch64 busy-spun.
                unsafe {
                    { crate::arch::enable_and_halt(); }
                }
                if let Some(next) = crate::process::next_runnable_pid(pid) {
                    if next != pid {
                        let urip = crate::arch::syscall::user_rip();
                        let ursp = crate::arch::syscall::user_rsp();
                        crate::process::save_return_context_reexec(pid, urip, ursp);
                        crate::process::save_full_user_gprs(pid);
                        crate::process::set_rax(pid, sys_nr);
                        crate::process::save_xstate(pid);
                        crate::process::set_state(pid, crate::process::ProcState::Blocked);
                        crate::process::enter_user_by_pid_noreturn(next);
                    }
                }
            }
            futex_waiter_remove(uaddr, pid);
            0
        }
        _ => -22, // EINVAL: unsupported op
    }
}

// ── Phase 60: pty/tty ─────────────────────────────────────────────────────────

pub(crate) fn sys_pty_open(flags: u64) -> i64 {
    let _ = flags;
    match crate::drivers::pty::open() {
        Ok((master, slave)) => ((master as i64) << 32) | (slave as i64),
        Err(_) => -24, // ENFILE
    }
}

pub(crate) fn sys_pty_read(fd: u64, buf_ptr: u64, count: u64) -> i64 {
    if buf_ptr == 0 { return -14; }
    let buf = unsafe { core::slice::from_raw_parts_mut(buf_ptr as *mut u8, count as usize) };
    match crate::drivers::pty::read(fd as u32, buf) {
        Ok(n) => n as i64,
        Err(_) => -5,
    }
}

pub(crate) fn sys_pty_write(fd: u64, buf_ptr: u64, count: u64) -> i64 {
    if buf_ptr == 0 { return -14; }
    let buf = unsafe { core::slice::from_raw_parts(buf_ptr as *const u8, count as usize) };
    match crate::drivers::pty::write(fd as u32, buf) {
        Ok(n) => n as i64,
        Err(_) => -5,
    }
}

pub(crate) fn sys_pty_ioctl(fd: u64, cmd: u64, arg: u64) -> i64 {
    crate::drivers::pty::ioctl(fd as u32, cmd, arg)
}

// ── Phase 31 Slice C: shared-address-space threads ───────────────────────────

/// Create a thread in the current process's address space.
///
/// Supports two calling conventions on syscall 0x35A:
/// - Raw OSCortex ABI: `(entry_fn, arg, stack_size)` -> returns `tid`.
/// - POSIX pthread ABI: `(pthread_t* out, attr*, start_routine, arg)` ->
///   writes `*out = tid`, returns `0` on success.
pub(crate) fn sys_thread_create(arg0: u64, arg1: u64, arg2: u64, arg3: u64) -> i64 {
    let parent_pid = crate::process::current_pid();
    log::error!("[thread-create] ENTER pid={} a0={:#x} a1={:#x} a2={:#x} a3={:#x}",
        parent_pid, arg0, arg1, arg2, arg3);
    if parent_pid == 0 {
        return -1; // EPERM
    }

    let spawn = |entry_fn: u64, arg: u64, stack_size: u64| -> Result<u32, i64> {
        let sz = if stack_size == 0 {
            1024 * 1024 // 1 MiB default
        } else {
            (stack_size as usize).max(512 * 1024) // 512 KiB minimum
        };
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
            Err(errno) => {
                log::error!("[thread-create] POSIX spawn FAILED entry={:#x} arg={:#x} stk={:#x} errno={}",
                    arg2, arg3, stack_size, errno);
                (-errno, None)
            }
        };
        log::warn!("[trace] sys_thread_create POSIX out={:#x} attr={:#x} entry={:#x} arg={:#x} stk={:#x} -> {}",
            arg0, arg1, arg2, arg3, stack_size, r);
        if let Some(child_pid) = child_pid_opt {
            log::warn!("[thread-create] spawned child={} entry={:#x}", child_pid, arg2);

            // Give the newborn an IMMEDIATE slice by entering the child from inside
            // the creator's pthread_create syscall (never returning here; the
            // creator gets r=0 on its later cooperative resume). The Flutter engine
            // bootstrap depends on this ordering: FlutterEngineRunInitialized posts
            // the root-isolate-launch task to the freshly-created UI thread, and the
            // platform thread (pid 1) then blocks waiting for it. If the UI thread
            // is only marked Running and left for "later" pickup, main()/runApp may
            // never run promptly → no frame is ever scheduled → nothing presents.
            //
            // This was OFF on aarch64 because the nested enter-while-in-a-syscall
            // corrupted the creator's resume (next pthread_create FML_CHECK abort).
            // That corruption was the cooperative-resume register/stack loss since
            // FIXED: callee-saved x24-x28 + x30/LR, FP/SIMD (vector-stub save), the
            // RETURN-mode x0=rax delivery, and the SP_EL1 syscall-stack reset. The
            // creator resumes via build_image, which is now lossless for a syscall
            // boundary (caller-saved x6/x7/x9-x18 are dead across the svc per the
            // ABI; everything live is preserved), so re-enabling is safe.
            {
                let parent = crate::process::current_pid();
                if parent != 0 && child_pid != parent {
                    let my_cpu = crate::arch::smp::this_cpu().cpu_id;
                    if crate::process::try_claim_cpu_for(child_pid, my_cpu) {
                        let urip = crate::arch::syscall::user_rip();
                        let ursp = crate::arch::syscall::user_rsp();
                        crate::process::save_return_context(parent, urip, ursp);
                        crate::process::save_full_user_gprs(parent);
                        crate::process::set_rax(parent, 0);
                        crate::process::save_xstate(parent);
                        crate::process::enter_user_by_pid_noreturn(child_pid);
                    }
                }
            }
            let _ = child_pid;
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
pub(crate) fn sys_thread_exit(code: u64) -> i64 {
    let pid = crate::process::current_pid();
    if code != 0 {
        let user_rip = crate::arch::syscall::user_rip();
        let user_rsp = crate::arch::syscall::user_rsp();
        log::warn!("[trace] sys_thread_exit pid={} code={:#x} rip={:#x} rsp={:#x}",
            pid, code, user_rip, user_rsp);
        dump_recent_syscalls(SYSCALL_TRACE_DEPTH);
    }
    // Activate post-exit syscall trace window so we can see what pid=1 does
    // after it resumes from pthread_cond_wait.
    POSTEXIT_TRACE_COUNT.store(0, Ordering::Relaxed);
    // Disabled: engine is healthy past pid-2 exit; trace just spams logs.
    // POSTEXIT_TRACE_ACTIVE.store(true, Ordering::Relaxed);
    sys_exit(code)
}

/// Wait for a thread to finish and return its exit code.
pub(crate) fn sys_thread_join(thread_handle: u64, retval: u64) -> i64 {
    // pthread_join(thread, retval) MUST return 0 on success — the joined thread's
    // exit value goes into *retval, NOT the return code. The Dart VM wraps this in
    // VALIDATE_PTHREAD_RESULT and FATALs ("pthread error: %d") on any non-zero return,
    // so returning the exit code (or ECHILD/EAGAIN) crashed worker threads.
    // Block until the target exits (busy-wait with pause; preemption runs it), then 0.
    let tid = if thread_handle > 0x100000 {
        match crate::process::find_tid_by_fs_base(thread_handle) {
            Some(t) => t,
            None => {
                if retval != 0 { unsafe { *(retval as *mut u64) = 0; } }
                return 0; // already exited+reaped (or never existed) — treat as joined
            }
        }
    } else {
        thread_handle as u32
    };

    let mut spins: u64 = 0;
    loop {
        match crate::process::waitpid(tid) {
            Ok(code) => {
                if retval != 0 { unsafe { *(retval as *mut u64) = code as u64; } }
                return 0;
            }
            Err("not exited") => {
                spins += 1;
                if spins > 2_000_000_000 {
                    // Safety cap so a deadlocked join can't hang forever; treat as joined.
                    if retval != 0 { unsafe { *(retval as *mut u64) = 0; } }
                    return 0;
                }
                // Cooperatively yield so the join TARGET (or another runnable
                // thread) can run and exit. A bare spin_pause never makes progress
                // on aarch64: a thread spinning here at EL1 can't be timer-preempted
                // (the tick returns early when taken from EL1), so the join target
                // would never be scheduled and this would spin to the 2e9 cap. Hand
                // the core to a runnable thread and RE-EXEC this join syscall on
                // resume to re-check waitpid. (x86 relied on kernel-mode preemption
                // running the target; aarch64 must yield explicitly.)
                let me = crate::process::current_pid();
                if me != 0 {
                    if let Some(next) = crate::syscall::cooperative_sched_target(me) {
                        if next != me {
                            let urip = crate::arch::syscall::user_rip();
                            let ursp = crate::arch::syscall::user_rsp();
                            crate::process::save_return_context_reexec(me, urip, ursp);
                            crate::process::save_full_user_gprs(me);
                            crate::process::set_rax(me, crate::embedder::abi::SYS_THREAD_JOIN);
                            crate::process::save_xstate(me);
                            crate::process::enter_user_by_pid_noreturn(next);
                        }
                    }
                }
                crate::arch::spin_pause();
            }
            Err(_) => {
                if retval != 0 { unsafe { *(retval as *mut u64) = 0; } }
                return 0;
            }
        }
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
pub(crate) fn sys_clone(flags: u64, stack_top: u64) -> i64 {
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
