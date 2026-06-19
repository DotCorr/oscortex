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

/// Gate: true once CPU 0 has entered the scheduler and claimed the init thread.
/// APs must not run the scheduler before this (else they'd race the BSP to claim
/// the init thread). Set inside `enter_user_by_pid_noreturn` on CPU 0. Mirrors
/// the aarch64 SMP gate so the arch-neutral scheduler code links on both arches.
pub static SMP_SCHED_READY: AtomicBool = AtomicBool::new(false);

#[inline]
pub fn sched_ready() -> bool {
    SMP_SCHED_READY.load(Ordering::Acquire)
}
#[inline]
pub fn set_sched_ready() {
    SMP_SCHED_READY.store(true, Ordering::Release);
}

// ── Per-CPU data ──────────────────────────────────────────────────────────────

/// Per-CPU mutable state — one entry per logical processor.
#[repr(C)]
pub struct PerCpuData {
    pub cpu_id:   u32,
    pub lapic_id: u32,
    pub online:   AtomicBool,
    pub current_pid: AtomicU32,
}

const ZERO_CPU: PerCpuData = PerCpuData {
    cpu_id:   0,
    lapic_id: 0,
    online:   AtomicBool::new(false),
    current_pid: AtomicU32::new(0),
};
static mut PER_CPU_DATA: [PerCpuData; MAX_CPUS] = [ZERO_CPU; MAX_CPUS];

/// Total logical CPUs online (including BSP).
pub static CPU_COUNT: AtomicU32 = AtomicU32::new(1);

/// Return this CPU's ID (by looking up the local APIC ID).
pub fn current_cpu_id() -> u32 {
    let lapic_id = crate::arch::x86_64::apic::local_apic_id();
    let n = CPU_COUNT.load(Ordering::Relaxed) as usize;
    unsafe {
        for i in 0..n {
            if PER_CPU_DATA[i].lapic_id == lapic_id {
                return i as u32;
            }
        }
    }
    0
}

/// Return this CPU's per-CPU entry.
pub fn this_cpu() -> &'static PerCpuData {
    let cpu_idx = current_cpu_id() as usize;
    unsafe { &PER_CPU_DATA[cpu_idx] }
}

// ── AP entry ──────────────────────────────────────────────────────────────────

/// Called by Limine on each AP after `cpu.bootstrap()` writes the goto pointer.
/// `info.extra_argument()` carries the `cpu_idx` assigned by the BSP.
pub unsafe extern "C" fn ap_entry(info: &MpInfo) -> ! {
    let cpu_idx = info.extra_argument() as u32;

    while !crate::KERNEL_INIT_DONE.load(Ordering::Acquire) {
        core::hint::spin_loop();
    }

    crate::arch::x86_64::ap_init(cpu_idx);

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

    // SMP scheduling isn't verified safe on real hardware yet: the scheduler is
    // not fully SMP-safe, ap_init faulted on a real 2016 Skylake Mac, and the BSP
    // then ground through a spin-count AP timeout (PAUSE is ~140 cycles on
    // Skylake, so the old 200x5M budget was ~minutes per AP) — hanging the boot at
    // cortex::smp. Single-core is the proven-stable config (the freeze is fixed by
    // the serial-GC engine, not SMP). Boot single-core by default; build with
    // `--features smp` to develop AP bring-up.
    #[cfg(not(feature = "smp"))]
    {
        let _ = smp_resp;
        log::info!("[SMP] single-core boot (AP bring-up gated behind the `smp` feature)");
    }

    #[cfg(feature = "smp")]
    {
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
            CPU_COUNT.store(cpu_idx + 1, Ordering::Release);
            cpu.bootstrap(ap_entry as MpGotoFunction, cpu_idx as u64);
            cpu_idx += 1;
        }

        // Wait for each AP to come online — bounded by BOTH a wall-clock deadline AND a
        // hard, clock-INDEPENDENT spin cap. The deadline (rdtsc_ns) is the normal path.
        // The spin cap is the fail-safe for firmware where the monotonic clock does not
        // advance as assumed: Apple Mac firmware doesn't wire the legacy PIT and
        // rdtsc_ns() divides the raw TSC by a FIXED rate, so on a Mac the deadline can
        // never be reached and the BSP would spin FOREVER here — exactly the observed
        // hang at `cortex::smp 0.5s` on a real Intel MacBook. The spin cap makes infinite
        // spin impossible: the wait always terminates and the BSP continues, advertising
        // only the APs that actually came online (so the box boots single-core-safe).
        let expected = cpu_idx as usize;
        const AP_TIMEOUT_NS: u64 = 500_000_000; // 0.5 s per AP (normal path)
        const AP_SPIN_CAP: u64 = 50_000_000;    // clock-independent backstop (~secs, only
                                                //  ever hit when the clock is frozen/wrong)
        for i in 1..expected {
            let deadline = crate::arch::rdtsc_ns().wrapping_add(AP_TIMEOUT_NS);
            let mut spins: u64 = 0;
            while !unsafe { PER_CPU_DATA[i].online.load(Ordering::Acquire) } {
                spins = spins.wrapping_add(1);
                if crate::arch::rdtsc_ns() >= deadline || spins >= AP_SPIN_CAP {
                    log::warn!("[SMP] CPU {} (lapic={}) timed out (spins={}) — continuing",
                        i, unsafe { PER_CPU_DATA[i].lapic_id }, spins);
                    break;
                }
                core::hint::spin_loop();
            }
        }

        // Publish the ONLINE count, not the configured count. The scheduler reads
        // `CPU_COUNT > 1` to decide whether to take the home-pinned SMP path, and
        // `pick_home_cpu_for_app` pins apps to cores `1..CPU_COUNT`. If an AP failed
        // to come online we MUST NOT advertise it — otherwise an app pinned to that
        // dead core would never be scheduled (starves at a black screen). APs come
        // online contiguously from index 1 under Limine, so the online count is also
        // a valid scan range for `current_cpu_id()`.
        let online = (0..expected)
            .filter(|&i| unsafe { PER_CPU_DATA[i].online.load(Ordering::Acquire) })
            .count();
        CPU_COUNT.store(online as u32, Ordering::Release);
        log::error!("[SMP] {}/{} CPU(s) online — CPU_COUNT={}", online, expected, online);
    }
}

/// Broadcast a reschedule IPI to all other online CPUs.
pub fn broadcast_resched_ipi() {
    let my_lapic_id = crate::arch::x86_64::apic::local_apic_id();
    let n = CPU_COUNT.load(Ordering::Relaxed) as usize;
    for i in 0..n {
        let cpu = unsafe { &PER_CPU_DATA[i] };
        if cpu.online.load(Ordering::Acquire) && cpu.lapic_id != my_lapic_id {
            crate::arch::x86_64::apic::send_resched_ipi(cpu.lapic_id);
        }
    }
}
