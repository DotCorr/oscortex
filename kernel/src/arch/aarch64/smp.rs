//! SMP — aarch64 multiprocessor scaffolding (PSCI / GICv3 SGI based).
//!
//! This is a single-core-only scaffold: the per-CPU table and `this_cpu()`
//! lookup are real (so shared scheduler/process code compiles and runs on the
//! BSP), but AP wake-up via PSCI `CPU_ON` and reschedule IPIs via GIC SGIs are
//! not yet implemented.

use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use limine::request::MpResponse;

/// Maximum CPUs supported.
pub const MAX_CPUS: usize = 64;

/// Per-CPU mutable state — one entry per logical processor.
///
/// Field layout mirrors the x86_64 backend so shared code is portable.
#[repr(C)]
pub struct PerCpuData {
    pub cpu_id:      u32,
    /// MPIDR_EL1 affinity of this core (aarch64 analogue of the x86 LAPIC id).
    pub mpidr:       u64,
    pub online:      AtomicBool,
    pub current_pid: AtomicU32,
}

const ZERO_CPU: PerCpuData = PerCpuData {
    cpu_id:      0,
    mpidr:       0,
    online:      AtomicBool::new(false),
    current_pid: AtomicU32::new(0),
};
static mut PER_CPU_DATA: [PerCpuData; MAX_CPUS] = [ZERO_CPU; MAX_CPUS];

/// Total logical CPUs online (including BSP).
pub static CPU_COUNT: AtomicU32 = AtomicU32::new(1);

/// Read this core's MPIDR_EL1 affinity bits.
#[inline]
fn read_mpidr() -> u64 {
    let v: u64;
    unsafe { core::arch::asm!("mrs {}, mpidr_el1", out(reg) v, options(nomem, nostack)) };
    v & 0x00FF_FFFF // Aff0..Aff2
}

/// Return this CPU's ID (by matching MPIDR_EL1 against the per-CPU table).
pub fn current_cpu_id() -> u32 {
    let n = CPU_COUNT.load(Ordering::Relaxed) as usize;
    // Single-core fast path: the BSP is always CPU 0. This also makes the timer
    // ISR's per-CPU lookup robust against a transiently-inconsistent PER_CPU_DATA
    // read (the lookup result indexes PTABLE_LOCK_RECURSION/PER_CPU_DATA, and a
    // garbage index would panic the kernel from inside the IRQ handler).
    if n <= 1 {
        return 0;
    }
    let mpidr = read_mpidr();
    unsafe {
        let table = &*core::ptr::addr_of!(PER_CPU_DATA);
        for i in 0..n.min(MAX_CPUS) {
            if table[i].mpidr == mpidr {
                return i as u32;
            }
        }
    }
    0
}

/// Return this CPU's per-CPU entry.
pub fn this_cpu() -> &'static PerCpuData {
    let cpu_idx = current_cpu_id() as usize;
    unsafe { &(*core::ptr::addr_of!(PER_CPU_DATA))[cpu_idx] }
}

/// Wake all APs. Currently single-core only.
///
/// TODO(arm): bring APs online via PSCI `CPU_ON` (SMC/HVC) and have each call
/// `crate::arch::ap_init()` then `crate::sched::ap_start()`.
pub fn init(_smp_resp: Option<&'static MpResponse>) {
    unsafe {
        let table = &mut *core::ptr::addr_of_mut!(PER_CPU_DATA);
        table[0].cpu_id = 0;
        table[0].mpidr = read_mpidr();
        table[0].online.store(true, Ordering::Release);
    }
    CPU_COUNT.store(1, Ordering::Release);
    log::info!("[SMP] aarch64 single-core scaffold (AP wake via PSCI not implemented)");
}

/// Broadcast a reschedule IPI to all other online CPUs.
///
/// TODO(arm): send a GIC SGI to the target CPU interface(s). No-op (single core).
pub fn broadcast_resched_ipi() {}
