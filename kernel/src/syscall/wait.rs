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

pub(crate) fn futex_waiter_present(addr: u64, pid: u32) -> bool {
    let table = FUTEX_WAITERS.lock();
    table.get(&addr).map_or(false, |waiters| waiters.contains(&pid))
}
