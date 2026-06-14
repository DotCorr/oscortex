use super::state::FUTEX_WAITERS;
use alloc::vec::Vec;

pub(crate) fn futex_waiter_add(addr: u64, pid: u32) {
    let mut table = FUTEX_WAITERS.lock();
    let waiters = table.entry(addr).or_insert_with(Vec::new);
    if !waiters.contains(&pid) {
        waiters.push(pid);
    }
}

pub(crate) fn futex_waiter_remove(addr: u64, pid: u32) {
    let mut table = FUTEX_WAITERS.lock();
    if let Some(waiters) = table.get_mut(&addr) {
        waiters.retain(|&waiter| waiter != pid);
        if waiters.is_empty() {
            table.remove(&addr);
        }
    }
}

/// ISR-safe variant of [`futex_waiter_remove`]. The generic-timer ISR drives
/// cond-wait timeout expiry (poll::check_timerfds_and_wake_try) and must never
/// block on a lock an interrupted syscall already holds — on a single core that
/// is an instant, IRQ-masked self-deadlock. Returns `false` on contention so the
/// caller can retry on the next tick rather than spin.
pub(crate) fn futex_waiter_remove_try(addr: u64, pid: u32) -> bool {
    match FUTEX_WAITERS.try_lock() {
        Some(mut table) => {
            if let Some(waiters) = table.get_mut(&addr) {
                waiters.retain(|&waiter| waiter != pid);
                if waiters.is_empty() {
                    table.remove(&addr);
                }
            }
            true
        }
        None => false,
    }
}

pub(crate) fn futex_waiter_present(addr: u64, pid: u32) -> bool {
    let table = FUTEX_WAITERS.lock();
    table.get(&addr).map_or(false, |waiters| waiters.contains(&pid))
}

pub(crate) fn futex_waiter_for(addr: u64, exclude: u32) -> Option<u32> {
    // Only return a waiter in the SAME address space as the caller (`exclude` is
    // the calling pid). The waiter table is keyed by VA alone, which collides
    // across processes that load the same .so at the same address — see
    // futex_wake_waiters for the full rationale.
    let grp = if exclude == 0 { 0 } else { crate::process::get_group_leader(exclude) };
    let table = FUTEX_WAITERS.lock();
    table.get(&addr).and_then(|waiters| {
        waiters.iter().copied().find(|&pid| {
            pid != exclude && (grp == 0 || crate::process::get_group_leader(pid) == grp)
        })
    })
}
