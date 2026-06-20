use super::state::{FutexKey, FUTEX_WAITERS};
use alloc::vec::Vec;

/// Build the address-space-identity key for `va` from an explicit page-table root.
///
/// No page-table WALK — `root_phys` is the process's already-loaded root (the
/// value the CPU's CR3/TTBR0 holds for that address space). This is the cheap,
/// correct identity (foundation blueprint Step 4); the per-op virtual→physical
/// translate that double-faulted in Attempt-1 is gone.
#[inline]
pub(crate) fn futex_key_for(root_phys: u64, va: u64) -> FutexKey {
    FutexKey::Private { root_phys, va }
}

/// Build the futex key for `va` in the CURRENT process's address space.
///
/// MUST be called from the context of the process that owns `va` (the waiter
/// itself, or a wake/signal issued by a thread of the same process — every futex
/// is private, so a futex word only ever belongs to one address space). Reads the
/// current page-table root ONCE; `current_user_root` takes PTABLE_LOCK and releases
/// it, so callers must invoke this BEFORE acquiring FUTEX_WAITERS to preserve the
/// Step-3 lock order (FUTEX_WAITERS → PTABLE_LOCK).
#[inline]
pub(crate) fn futex_key_for_current(va: u64) -> FutexKey {
    let root = crate::process::current_user_root();
    FutexKey::Private { root_phys: root, va }
}

pub(crate) fn futex_waiter_add(addr: u64, pid: u32) {
    let key = futex_key_for_current(addr);
    let mut table = FUTEX_WAITERS.lock();
    let waiters = table.entry(key).or_insert_with(Vec::new);
    if !waiters.contains(&pid) {
        waiters.push(pid);
    }
}

pub(crate) fn futex_waiter_remove(addr: u64, pid: u32) {
    let key = futex_key_for_current(addr);
    let mut table = FUTEX_WAITERS.lock();
    if let Some(waiters) = table.get_mut(&key) {
        waiters.retain(|&waiter| waiter != pid);
        if waiters.is_empty() {
            table.remove(&key);
        }
    }
}

/// ISR-safe variant of [`futex_waiter_remove`]. The generic-timer ISR drives
/// cond-wait timeout expiry (poll::check_timerfds_and_wake_try) and must never
/// block on a lock an interrupted syscall already holds — on a single core that
/// is an instant, IRQ-masked self-deadlock. Returns `false` on contention so the
/// caller can retry on the next tick rather than spin.
///
/// Takes a PRE-COMPUTED [`FutexKey`] (from the parked thread's CondWaitState):
/// the waiter may belong to a process OTHER than the one current on this CPU, so
/// the key CANNOT be re-derived here — and deriving it would touch PTABLE_LOCK /
/// the page tables under the IRQ mask, the exact per-op translate that
/// double-faulted in Attempt-1.
pub(crate) fn futex_waiter_remove_try(key: FutexKey, pid: u32) -> bool {
    match FUTEX_WAITERS.try_lock() {
        Some(mut table) => {
            if let Some(waiters) = table.get_mut(&key) {
                waiters.retain(|&waiter| waiter != pid);
                if waiters.is_empty() {
                    table.remove(&key);
                }
            }
            true
        }
        None => false,
    }
}

pub(crate) fn futex_waiter_present(addr: u64, pid: u32) -> bool {
    let key = futex_key_for_current(addr);
    let table = FUTEX_WAITERS.lock();
    table.get(&key).map_or(false, |waiters| waiters.contains(&pid))
}

pub(crate) fn futex_waiter_for(addr: u64, exclude: u32) -> Option<u32> {
    // The waiter table is now keyed by (page-table root, va) — a process-private
    // identity — so a bucket only ever holds waiters of ONE address space and the
    // old cross-process VA collision is impossible. `exclude` (the caller's pid)
    // skips the caller itself; the group-leader filter is kept as belt-and-braces
    // (it is now redundant but harmless — every waiter under this key is already a
    // sibling).
    let grp = if exclude == 0 { 0 } else { crate::process::get_group_leader(exclude) };
    let key = futex_key_for_current(addr);
    let table = FUTEX_WAITERS.lock();
    table.get(&key).and_then(|waiters| {
        waiters.iter().copied().find(|&pid| {
            pid != exclude && (grp == 0 || crate::process::get_group_leader(pid) == grp)
        })
    })
}
