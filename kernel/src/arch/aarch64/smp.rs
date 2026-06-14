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
    /// An AP is "engaged" once it has joined user scheduling. While NOT engaged
    /// (online but idling pre-engage), its timer ISR does ZERO scheduler/lock work
    /// (just EOI) so it can't contend PTABLE_LOCK with the BSP during the engine's
    /// single-core bring-up. The BSP (cpu 0) is always considered engaged.
    pub engaged:     AtomicBool,
}

const ZERO_CPU: PerCpuData = PerCpuData {
    cpu_id:      0,
    mpidr:       0,
    online:      AtomicBool::new(false),
    current_pid: AtomicU32::new(0),
    engaged:     AtomicBool::new(false),
};
static mut PER_CPU_DATA: [PerCpuData; MAX_CPUS] = [ZERO_CPU; MAX_CPUS];

/// Total logical CPUs online (including BSP).
pub static CPU_COUNT: AtomicU32 = AtomicU32::new(1);

/// Set true by the BSP once it has claimed the init process and is entering
/// userspace — APs must not run the scheduler before this (else they'd race the
/// BSP to claim the init thread). Set inside `enter_user_by_pid_noreturn` on CPU 0.
pub static SMP_SCHED_READY: AtomicBool = AtomicBool::new(false);

#[inline]
pub fn sched_ready() -> bool {
    SMP_SCHED_READY.load(Ordering::Acquire)
}
#[inline]
pub fn set_sched_ready() {
    SMP_SCHED_READY.store(true, Ordering::Release);
}

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

// ── SMP Milestone 1: AP bring-up via PSCI CPU_ON ─────────────────────────────
//
// Secondary cores park in boot.rs `_start` (wfe). We wake them with PSCI CPU_ON
// (conduit read from the DTB) pointing at `_ap_trampoline` (a PHYSICAL address —
// the `-kernel` image is identity-mapped, VA==PA). The trampoline runs MMU-off,
// replicates the BSP's MMU/system state captured in `capture_boot_state`, turns
// the MMU on, then calls `ap_main`. Milestone 1: the AP just inits its GIC CPU
// interface, marks itself online, and idles — it touches NO shared user/scheduler
// state yet (that's Milestone 2+). The BSP keeps doing all the work; an AP coming
// online must not perturb the single-core path.

const AP_STACK_SIZE: usize = 64 * 1024;
const MAX_APS: usize = 3; // cores 1..=3 (core 0 = BSP)

#[repr(C, align(16))]
struct ApStacks([[u8; AP_STACK_SIZE]; MAX_APS]);
static mut AP_STACKS: ApStacks = ApStacks([[0; AP_STACK_SIZE]; MAX_APS]);

// BSP MMU/system state the AP must load before it can run kernel C/Rust.
static mut AP_MAIR: u64 = 0;
static mut AP_TCR: u64 = 0;
static mut AP_TTBR0: u64 = 0;
static mut AP_TTBR1: u64 = 0;
static mut AP_SCTLR: u64 = 0;
static mut AP_VBAR: u64 = 0;

/// Snapshot the BSP's live MMU + vector registers so an AP can install identical
/// translation before touching kernel memory. Call AFTER mmu::enable + vectors.
pub fn capture_boot_state() {
    unsafe {
        let (mair, tcr, ttbr0, ttbr1, sctlr, vbar): (u64, u64, u64, u64, u64, u64);
        core::arch::asm!(
            "mrs {0}, mair_el1",
            "mrs {1}, tcr_el1",
            "mrs {2}, ttbr0_el1",
            "mrs {3}, ttbr1_el1",
            "mrs {4}, sctlr_el1",
            "mrs {5}, vbar_el1",
            out(reg) mair, out(reg) tcr, out(reg) ttbr0,
            out(reg) ttbr1, out(reg) sctlr, out(reg) vbar,
            options(nomem, nostack),
        );
        AP_MAIR = mair;
        AP_TCR = tcr;
        AP_TTBR0 = ttbr0;
        AP_TTBR1 = ttbr1;
        AP_SCTLR = sctlr;
        AP_VBAR = vbar;
    }
}

// AP entry trampoline. PSCI starts the core here MMU-off (x0 = context_id = the
// AP's logical cpu index, which we passed as the PSCI context). It drops EL2→EL1
// if needed, enables FP, installs the BSP's MMU regs, turns the MMU on, sets VBAR,
// switches to this AP's stack, then branches to `ap_main(cpu_idx)`.
core::arch::global_asm!(
    r#"
.section .text.ap_boot, "ax"
.globl _ap_trampoline
_ap_trampoline:
    mov     x19, x0                     // x19 = cpu_idx (PSCI context_id)
    // Drop EL2 -> EL1h if we came up at EL2.
    mrs     x0, CurrentEL
    lsr     x0, x0, #2
    cmp     x0, #2
    b.ne    1f
    mov     x0, #(1 << 31)
    msr     hcr_el2, x0
    mov     x0, #0x3c5                  // DAIF masked + EL1h
    msr     spsr_el2, x0
    adr     x0, 1f
    msr     elr_el2, x0
    eret
1:
    msr     daifset, #0xf
    // Enable FP/AdvSIMD (CPACR_EL1.FPEN = 0b11).
    mrs     x0, cpacr_el1
    orr     x0, x0, #(3 << 20)
    msr     cpacr_el1, x0
    isb
    // Install the BSP's translation regs (MMU still OFF; symbols resolve to PA).
    adrp    x0, {ap_mair}
    ldr     x1, [x0, #:lo12:{ap_mair}]
    msr     mair_el1, x1
    adrp    x0, {ap_tcr}
    ldr     x1, [x0, #:lo12:{ap_tcr}]
    msr     tcr_el1, x1
    adrp    x0, {ap_ttbr0}
    ldr     x1, [x0, #:lo12:{ap_ttbr0}]
    msr     ttbr0_el1, x1
    adrp    x0, {ap_ttbr1}
    ldr     x1, [x0, #:lo12:{ap_ttbr1}]
    msr     ttbr1_el1, x1
    isb
    tlbi    vmalle1
    dsb     nsh
    isb
    // Turn the MMU on (SCTLR with M/C/I as the BSP has it).
    adrp    x0, {ap_sctlr}
    ldr     x1, [x0, #:lo12:{ap_sctlr}]
    msr     sctlr_el1, x1
    isb
    // Vector base.
    adrp    x0, {ap_vbar}
    ldr     x1, [x0, #:lo12:{ap_vbar}]
    msr     vbar_el1, x1
    // Stack: AP_STACKS has slots for cores 1..=MAX_APS (slot = cpu_idx-1). The top
    // of this AP's slot = base + cpu_idx * AP_STACK_SIZE.
    adrp    x0, {ap_stacks}
    add     x0, x0, :lo12:{ap_stacks}
    mov     x2, {stack_size}
    mul     x2, x2, x19                 // x2 = cpu_idx * AP_STACK_SIZE
    add     sp, x0, x2
    mov     x0, x19                     // ap_main(cpu_idx)
    bl      {ap_main}
2:  wfe
    b       2b
"#,
    ap_mair = sym AP_MAIR,
    ap_tcr = sym AP_TCR,
    ap_ttbr0 = sym AP_TTBR0,
    ap_ttbr1 = sym AP_TTBR1,
    ap_sctlr = sym AP_SCTLR,
    ap_vbar = sym AP_VBAR,
    ap_stacks = sym AP_STACKS,
    ap_main = sym ap_main,
    stack_size = const AP_STACK_SIZE,
);

extern "C" {
    fn _ap_trampoline();
}

/// First Rust code on a secondary core. MMU + VBAR are live; running on this AP's
/// own stack. Milestone 1: init the per-core GIC interface, register in the
/// per-CPU table, mark online, then idle. (No scheduler entry yet.)
#[no_mangle]
pub extern "C" fn ap_main(cpu_idx: u64) -> ! {
    let idx = cpu_idx as usize;
    unsafe {
        let table = &mut *core::ptr::addr_of_mut!(PER_CPU_DATA);
        if idx < MAX_CPUS {
            table[idx].cpu_id = cpu_idx as u32;
            table[idx].mpidr = read_mpidr();
            table[idx].online.store(true, Ordering::Release);
        }
    }
    // Per-core GIC CPU interface (GICv2: banked per-core at the same MMIO base).
    crate::arch::aarch64::gic::init_cpu_interface();
    CPU_COUNT.fetch_add(1, Ordering::AcqRel);
    log::error!("[SMP] AP cpu={} ONLINE (mpidr={:#x})", cpu_idx, read_mpidr());

    // AP stays ONLINE but IDLE (not engaged) — proven-stable on smp 1/2/4 with no
    // regression. Co-scheduling the engine across cores was attempted 6 ways
    // (2026-06-14): the contention/affinity/bring-up issues were each solved, but
    // bolting SMP onto the cooperative hand-off model still does NOT converge the
    // Dart GC stop-the-world safepoint — the app renders then freezes anyway. The
    // real fix is the Redox-style scheduler rework (per-CPU switch_to contexts +
    // affinity run-queue replacing the cooperative hand-off), a multi-session
    // effort; the infra for it (engaged flag, is_preemptable, sticky affinity,
    // enter_user back-off, physaddr-scoped futex, AP bring-up) is now in place.
    // See [[smp-bringup]] / [[redox-blueprint]]. Until then, idle = no regression.
    let _ = cpu_idx;
    loop {
        unsafe { core::arch::asm!("wfi", options(nomem, nostack)) };
    }
}

/// True if the given CPU has engaged user scheduling (BSP/cpu 0 always true).
#[inline]
pub fn cpu_engaged(cpu_id: u32) -> bool {
    if cpu_id == 0 {
        return true;
    }
    let i = cpu_id as usize;
    if i >= MAX_CPUS {
        return false;
    }
    unsafe { (*core::ptr::addr_of!(PER_CPU_DATA))[i].engaged.load(Ordering::Acquire) }
}

/// PSCI conduit, latched once from the DTB at wake time.
static PSCI_USE_SMC: AtomicBool = AtomicBool::new(false);

/// Raw PSCI/SMCCC call via the selected conduit. Returns x0.
unsafe fn psci_raw(use_smc: bool, fn_id: u64, a1: u64, a2: u64, a3: u64) -> i64 {
    let ret: u64;
    if use_smc {
        core::arch::asm!(
            "smc #0",
            inout("x0") fn_id => ret,
            in("x1") a1, in("x2") a2, in("x3") a3,
            out("x4") _, out("x5") _, out("x6") _, out("x7") _,
            options(nostack),
        );
    } else {
        core::arch::asm!(
            "hvc #0",
            inout("x0") fn_id => ret,
            in("x1") a1, in("x2") a2, in("x3") a3,
            out("x4") _, out("x5") _, out("x6") _, out("x7") _,
            options(nostack),
        );
    }
    ret as i64
}

/// Probe PSCI_VERSION (0x84000000, side-effect-free) on a conduit. A real PSCI
/// implementation returns a small positive version word (e.g. 0x10001 for v1.1);
/// a conduit that isn't wired returns -1 (NOT_SUPPORTED) or garbage.
unsafe fn psci_version(use_smc: bool) -> i64 {
    psci_raw(use_smc, 0x8400_0000, 0, 0, 0)
}

/// Issue PSCI CPU_ON (SMC64 fn 0xC400_0003) on the selected conduit.
unsafe fn psci_cpu_on(target_mpidr: u64, entry_pa: u64, context_id: u64) -> i64 {
    psci_raw(
        PSCI_USE_SMC.load(Ordering::Relaxed),
        0xC400_0003,
        target_mpidr,
        entry_pa,
        context_id,
    )
}

/// Wake secondary cores into `ap_main` (Milestone 1: → idle). Must run after the
/// BSP's MMU + vectors + GIC are up. Reads the PSCI conduit from the DTB; tries
/// CPU_ON for cores 1..=MAX_APS and stops at the first absent core.
pub fn wake_aps() {
    let dtb = crate::arch::aarch64::fdt::dtb_ptr();
    // Determine the PSCI conduit. Prefer the DTB; else probe both with the
    // side-effect-free PSCI_VERSION and use whichever the platform answers.
    let conduit = unsafe { crate::arch::aarch64::fdt::psci_method(dtb) };
    let use_smc = match conduit {
        Some(crate::arch::aarch64::fdt::PsciConduit::Smc) => true,
        Some(crate::arch::aarch64::fdt::PsciConduit::Hvc) => false,
        None => {
            let smc_v = unsafe { psci_version(true) };
            let hvc_v = unsafe { psci_version(false) };
            log::error!("[SMP] PSCI_VERSION probe: smc={:#x} hvc={:#x}", smc_v, hvc_v);
            // A valid PSCI version is a small positive word (>=0x10000 for v1.x,
            // 0x2 for v0.2); -1 = NOT_SUPPORTED on that conduit.
            if smc_v > 0 {
                true
            } else if hvc_v > 0 {
                false
            } else {
                log::error!("[SMP] neither conduit answers PSCI — staying single-core");
                return;
            }
        }
    };
    PSCI_USE_SMC.store(use_smc, Ordering::Relaxed);
    log::error!("[SMP] PSCI conduit = {}", if use_smc { "SMC" } else { "HVC" });

    capture_boot_state();
    let entry = _ap_trampoline as usize as u64;
    log::error!("[SMP] AP trampoline @ {:#x}; waking secondary cores...", entry);

    for idx in 1..=MAX_APS as u64 {
        // QEMU virt: core N has MPIDR Aff0 = N (< 16 cores).
        let target_mpidr = idx;
        let st = unsafe { psci_cpu_on(target_mpidr, entry, idx) };
        if st == 0 || st == -4 {
            // SUCCESS or ALREADY_ON — wait briefly for it to mark online.
            let mut spun = 0u64;
            loop {
                if unsafe { (*core::ptr::addr_of!(PER_CPU_DATA))[idx as usize].online.load(Ordering::Acquire) } {
                    break;
                }
                spun += 1;
                if spun > 200_000_000 {
                    log::error!("[SMP] core {} CPU_ON ok but never came online", idx);
                    break;
                }
                core::hint::spin_loop();
            }
        } else {
            // -2 INVALID_PARAMS = core absent → stop probing further cores.
            log::error!("[SMP] core {} CPU_ON returned {} — no more cores", idx, st);
            break;
        }
    }
    log::error!("[SMP] online CPU count = {}", CPU_COUNT.load(Ordering::Acquire));
}

/// Wake all APs. Sets up the BSP's per-CPU entry, then brings secondary cores
/// online (Milestone 1: into an idle loop).
pub fn init(_smp_resp: Option<&'static MpResponse>) {
    unsafe {
        let table = &mut *core::ptr::addr_of_mut!(PER_CPU_DATA);
        table[0].cpu_id = 0;
        table[0].mpidr = read_mpidr();
        table[0].online.store(true, Ordering::Release);
    }
    CPU_COUNT.store(1, Ordering::Release);
    wake_aps();
}

/// Broadcast a reschedule IPI to all other online CPUs.
///
/// TODO(arm): send a GIC SGI to the target CPU interface(s). No-op (single core).
pub fn broadcast_resched_ipi() {}
