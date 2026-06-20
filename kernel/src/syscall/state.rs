use alloc::collections::BTreeMap;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use spin::Mutex;

pub(crate) const ENGINE_LIBRARY_PATH: &str = "/system/lib/libflutter_engine.so";
pub(crate) static ENGINE_HOST_PID: AtomicU32 = AtomicU32::new(0);
pub(crate) static ENGINE_PROC_TABLE_PTR: AtomicU64 = AtomicU64::new(0);
pub(crate) static WM_WAITER_PID: AtomicU32 = AtomicU32::new(0);
pub(crate) static WM_WAITER_DEADLINE: AtomicU64 = AtomicU64::new(0);
pub(crate) static FS_BOOTSTRAP_LOGGED: AtomicU32 = AtomicU32::new(0);

pub(crate) const SYSCALL_TRACE_DEPTH: usize = 64;

#[derive(Clone, Copy)]
pub(crate) struct SyscallTraceEntry {
    pub pid: u32,
    pub nr: u64,
    pub a0: u64,
    pub a1: u64,
    pub a2: u64,
    pub rip: u64,
}

pub(crate) static SYSCALL_TRACE_BUF: Mutex<[SyscallTraceEntry; SYSCALL_TRACE_DEPTH]> =
    Mutex::new([const { SyscallTraceEntry { pid: 0, nr: 0, a0: 0, a1: 0, a2: 0, rip: 0 } }; SYSCALL_TRACE_DEPTH]);
pub(crate) static SYSCALL_TRACE_HEAD: AtomicU32 = AtomicU32::new(0);

/// Address-space-identity futex key (foundation blueprint Step 4).
///
/// The waiter table used to be keyed by the raw userspace virtual address. That
/// is process-LOCAL: the shell (pid 1) and a launched app load the same libc /
/// Flutter engine `.so` at IDENTICAL virtual addresses, so their internal pthread
/// mutex/cond words COLLIDE in one bucket → a wake pops the wrong process's waiter
/// and the intended one parks forever (the post-app-launch render freeze).
///
/// The correct, cheap key = `(the process's page-table ROOT physical address, va)`.
/// `sys_mmap` (syscall/handlers/engine.rs) takes only `(hint, size, prot)` — there
/// is NO flags argument, so `MAP_SHARED` cannot be expressed and EVERY mapping is
/// private/anonymous → EVERY futex is private → this key is provably exact. Threads
/// of the SAME process share one `pml4_phys` (spawn_thread/clone_thread set
/// `p.pml4_phys = parent_pml4_phys`), so same-process threads share a key and
/// correctly collide (same memory); different processes have different roots and
/// correctly separate — WITHOUT any per-op virtual→physical page-table walk (the
/// per-op translate is what double-faulted Attempt-1; see the deleted
/// `futex_phys_of`). The key is computed ONCE, from the already-loaded root, at the
/// point a waiter is recorded — never under an IRQ mask.
///
/// `Shared { frame_phys }` is reserved for a future `MAP_SHARED` and is never
/// constructed today.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub(crate) enum FutexKey {
    /// Private/anonymous futex: identified by (page-table root, virtual address).
    Private { root_phys: u64, va: u64 },
    /// MAP_SHARED futex (reserved, unused): identified by backing physical frame.
    Shared { frame_phys: u64 },
}

impl FutexKey {
    /// The virtual address this key refers to (for diagnostics / glibc pokes that
    /// operate on the userspace word). `Shared` has no single VA → returns 0.
    #[inline]
    pub(crate) fn va(&self) -> u64 {
        match *self {
            FutexKey::Private { va, .. } => va,
            FutexKey::Shared { .. } => 0,
        }
    }
}

pub(crate) static FUTEX_WAITERS: Mutex<BTreeMap<FutexKey, Vec<u32>>> = Mutex::new(BTreeMap::new());
pub(crate) static FUTEX_PENDING_WAKES: Mutex<BTreeMap<FutexKey, u32>> = Mutex::new(BTreeMap::new());

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CondWaitState {
    /// `cond_key` is the PRE-COMPUTED FutexKey for `cond`, captured at enqueue time
    /// while the waiter is the current process. The generic-timer ISR cond-timeout
    /// path (poll::check_timerfds_and_wake_try) uses it to remove the waiter WITHOUT
    /// re-deriving the key under the IRQ mask (no page-table access in the ISR).
    Waiting { cond: u64, cond_key: FutexKey, mutex: u64, seq: u32, timeout_ns: u64 },
    AcquiringMutex { mutex: u64, timed_out: bool },
}

pub(crate) static COND_WAIT_STATE: Mutex<BTreeMap<u32, CondWaitState>> = Mutex::new(BTreeMap::new());

