//! SMP — Symmetric Multi-Processing startup for x86_64 via the Limine protocol.
//!
//! ## Startup sequence
//!
//! 1. BSP finishes full kernel init (heap, logging, scheduler, Cortex).
//! 2. BSP calls `smp::init()` which reads the Limine MpResponse.
//! 3. For each non-BSP CPU in the response, `init()`:
//!    a. Allocates a slot in `PER_CPU_DATA`.
//!    b. Calls `cpu.bootstrap(ap_entry, cpu_idx)` — Limine's parking stub
//!       then jumps to `ap_entry` on that AP.
//! 4. Each AP executes `ap_entry`: waits for KERNEL_INIT_DONE, calls
//!    `arch::ap_init()`, marks itself online, then enters the scheduler idle.
//!
//! ## Per-CPU data
//!
//! `PER_CPU_DATA[i]` holds the APIC ID and online status of CPU i.
//! `this_cpu()` scans by LAPIC ID to find the current CPU's entry.

use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use limine::mp::{MpInfo, MpGotoFunction};
use limine::request::MpResponse;

// ── Constants ─────────────────────────────────────────────────────────────────

/// Maximum CPUs supported.
pub const MAX_CPUS: usize = 64;

// ── Per-CPU data ──────────────────────────────────────────────────────────────

/// Per-CPU mutable state — one entry per logical processor.
#[repr(C)]
pub struct PerCpuData {
    pub cpu_id:   u32,
    pub lapic_id: u32,
    pub online:   AtomicBool,
}

const ZERO_CPU: PerCpuData = PerCpuData {
    cpu_id:   0,
    lapic_id: 0,
    online:   AtomicBool::new(false),
};
static mut PER_CPU_DATA: [PerCpuData; MAX_CPUS] = [ZERO_CPU; MAX_CPUS];

/// Total logical CPUs online (including BSP).
pub static CPU_COUNT: AtomicU32 = AtomicU32::new(1);

/// Return this CPU's per-CPU entry by matching LAPIC ID.
pub fn this_cpu() -> &'static PerCpuData {
    let apic_id = crate::arch::x86_64::apic::local_apic_id();
    let n = CPU_COUNT.load(Ordering::Relaxed) as usize;
    for i in 0..n {
        let cpu = unsafe { &PER_CPU_DATA[i] };
        if cpu.lapic_id == apic_id {
            return cpu;
        }
    }
    unsafe { &PER_CPU_DATA[0] }
}

// ── AP entry ──────────────────────────────────────────────────────────────────

/// Called by Limine on each AP after `cpu.bootstrap()` writes the goto pointer.
/// `info.extra_argument()` carries the `cpu_idx` assigned by the BSP.
pub unsafe extern "C" fn ap_entry(info: &MpInfo) -> ! {
    let cpu_idx = info.extra_argument() as u32;

    while !crate::KERNEL_INIT_DONE.load(Ordering::Acquire) {
        core::hint::spin_loop();
    }

    crate::arch::x86_64::ap_init();

    unsafe {
        PER_CPU_DATA[cpu_idx as usize].online.store(true, Ordering::Release);
    }

    log::info!("[SMP] AP cpu={} lapic_id={} online", cpu_idx, info.lapic_id);

    crate::sched::ap_start(cpu_idx);
}

// ── BSP SMP init ──────────────────────────────────────────────────────────────

/// Wake all APs. Called by BSP after `KERNEL_INIT_DONE` is set.
/// `smp_resp` comes from the MpRequest placed in main.rs.
pub fn init(smp_resp: Option<&'static MpResponse>) {
    let bsp_lapic_id = crate::arch::x86_64::apic::local_apic_id();

    unsafe {
        PER_CPU_DATA[0].cpu_id   = 0;
        PER_CPU_DATA[0].lapic_id = bsp_lapic_id;
        PER_CPU_DATA[0].online.store(true, Ordering::Release);
    }

    let resp = match smp_resp {
        Some(r) => r,
        None => {
            log::warn!("[SMP] Limine SMP response missing — single-core only");
            return;
        }
    };

    let cpus = resp.cpus();
    let ap_count = cpus.iter().filter(|c| c.lapic_id != bsp_lapic_id).count();
    if ap_count == 0 {
        log::info!("[SMP] No APs — single-core");
        return;
    }
    log::info!("[SMP] {} total CPU(s) — waking {} AP(s)", cpus.len(), ap_count);

    let mut cpu_idx: u32 = 1;
    for cpu in cpus {
        if cpu.lapic_id == bsp_lapic_id { continue; }
        if cpu_idx as usize >= MAX_CPUS {
            log::warn!("[SMP] MAX_CPUS limit reached — skipping remaining APs");
            break;
        }
        unsafe {
            PER_CPU_DATA[cpu_idx as usize].cpu_id   = cpu_idx;
            PER_CPU_DATA[cpu_idx as usize].lapic_id = cpu.lapic_id;
        }
        cpu.bootstrap(ap_entry as MpGotoFunction, cpu_idx as u64);
        cpu_idx += 1;
    }

    // Wait up to ~2 s per AP to come online.
    let expected = cpu_idx as usize;
    'outer: for i in 1..expected {
        for _ in 0..200u32 {
            if unsafe { PER_CPU_DATA[i].online.load(Ordering::Acquire) } {
                continue 'outer;
            }
            for _ in 0..5_000_000usize { core::hint::spin_loop(); }
        }
        log::warn!("[SMP] CPU {} (lapic={}) timed out",
            i, unsafe { PER_CPU_DATA[i].lapic_id });
    }

    CPU_COUNT.store(cpu_idx, Ordering::Release);
    let online = (0..expected)
        .filter(|&i| unsafe { PER_CPU_DATA[i].online.load(Ordering::Acquire) })
        .count();
    log::info!("[SMP] {}/{} CPU(s) online", online, expected);
}
