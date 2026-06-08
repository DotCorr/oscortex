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
pub(crate) static FUTEX_WAITERS: Mutex<BTreeMap<u64, Vec<u32>>> = Mutex::new(BTreeMap::new());
pub(crate) static FUTEX_PENDING_WAKES: Mutex<BTreeMap<u64, u32>> = Mutex::new(BTreeMap::new());

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CondWaitState {
    Waiting { cond: u64, mutex: u64, seq: u32, timeout_ns: u64 },
    AcquiringMutex { mutex: u64, timed_out: bool },
}

pub(crate) static COND_WAIT_STATE: Mutex<BTreeMap<u32, CondWaitState>> = Mutex::new(BTreeMap::new());

