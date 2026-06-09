//! Interrupt-controller + tick/vsync scaffolding — aarch64 (GIC + generic timer).
//!
//! On x86 this is the local APIC; on aarch64 the equivalent responsibilities are
//! split between the GIC (CPU interface: EOI, IPIs, routing) and the ARM generic
//! timer (CNTP/CNTV, the periodic tick that drives the vsync cadence). These are
//! compilable stubs preserving the public surface the shared kernel calls; real
//! GICv2/GICv3 + CNTV_CTL programming comes later.

use core::sync::atomic::{AtomicU32, Ordering};

/// Approximate timer ticks per millisecond (placeholder; real value is read
/// from `CNTFRQ_EL0` once the generic timer is programmed).
pub static APIC_TICKS_PER_MS: AtomicU32 = AtomicU32::new(62500);

/// Requested vsync cadence in Hz (0 = off).
static VSYNC_HZ: AtomicU32 = AtomicU32::new(0);

/// Tick rate of the production scheduler timer (Hz). Mirrors the x86 APIC-timer
/// 1 ms cadence closely enough for the cooperative scheduler's quantum logic.
const SCHED_TICK_HZ: u64 = 1000;

/// Initialise the interrupt controller on the BSP (early).
///
/// The GIC distributor + CPU interface are already live (brought up in
/// `boot_prod`). Here we only INSTALL the production IRQ handler — which drives
/// the SAME shared cooperative-scheduler hand-off the x86 APIC-timer ISR uses.
/// The generic timer itself is armed later by [`start_scheduler_tick`], once the
/// full kernel (heap, scheduler, process table) is initialised and we are about
/// to enter userspace — mirroring x86, which doesn't tick into a half-built
/// kernel.
pub fn init_bsp() {
    super::vectors::set_irq_handler(production_irq_handler);
}

/// Arm the generic timer for the periodic scheduler tick and unmask IRQ
/// delivery. Call once, just before entering the first EL0 thread, so the timer
/// only fires when there is a real scheduler + process table to preempt into.
pub fn start_scheduler_tick() {
    APIC_TICKS_PER_MS.store(
        (super::timer::freq_hz() / 1000).max(1) as u32,
        Ordering::Release,
    );
    super::timer::init_periodic(SCHED_TICK_HZ);
    // Note: we deliberately do NOT unmask IRQs at EL1 here. The `eret` into EL0
    // restores SPSR_EL1 = EL0t with I/F unmasked, so timer IRQs begin firing the
    // instant we drop to userspace — and never interrupt the half-built EL1
    // entry path (write_cr3 → image build → eret). The kernel idle/cortex loop
    // unmasks via `enable_and_halt` when it has no user thread to run.
    log::info!(
        "[apic] aarch64 scheduler timer armed @ {} Hz (CNTFRQ={} Hz), preemption live",
        SCHED_TICK_HZ,
        super::timer::freq_hz()
    );
}

/// Production IRQ handler — acknowledge at the GIC, and on a generic-timer PPI
/// from EL0 run the shared cooperative-scheduler preemption hand-off, exactly
/// mirroring `x86_64::idt::apic_timer_handler`.
///
/// The EL1 vector stub (`vectors.rs`) has already saved the full EL0 register
/// file into the [`super::vectors::TrapFrame`] `f` points at, and will
/// `RESTORE_FRAME` + `eret` from it after we return. So to switch threads we
/// simply (a) decide via the shared scheduler, (b) overwrite `f` with the next
/// thread's saved frame, and (c) switch TTBR0 — the stub's eret then enters the
/// next thread. The full ARM frame round-trips through each thread's
/// `arch_trapframe` slot so scratch registers survive (the x86-named `UserRegs`
/// the shared scheduler uses carries only a subset).
fn production_irq_handler(f: &mut super::vectors::TrapFrame) {
    let intid = super::gic::acknowledge();

    // Spurious / non-timer interrupts: EOI (if real) and return.
    if intid != super::timer::TIMER_PPI {
        if intid < 1020 {
            super::gic::eoi(intid);
        }
        return;
    }

    // Timer tick bookkeeping + re-arm the next interval.
    super::timer::on_tick();
    // EOI now so the GIC can deliver the next timer interrupt right after eret.
    super::gic::eoi(intid);

    // Wake threads whose timerfd deadlines have elapsed, and deliver any pending
    // cross-thread wakes — exactly as the x86 APIC-timer ISR does (idt.rs). On
    // aarch64 this was missing, so the engine's UI/raster/IO worker threads that
    // epoll_wait on a timerfd went Blocked and were NEVER transitioned back to
    // Running → the cooperative scheduler found no runnable thread and the whole
    // engine deadlocked right after spawning its worker pool. The try_-variants
    // are ISR-safe (skip on lock contention rather than deadlock).
    crate::syscall::check_timerfds_and_wake_try();
    crate::process::handle_pending_wakes_try();

    // Only preempt interrupts taken FROM EL0 (a running user thread). SPSR_EL1
    // M[3:0] == 0 means the interrupted context was EL0t. Kernel-mode ticks
    // (idle/cortex loop) just return — there is no user frame to switch.
    let from_el0 = (f.spsr & 0xF) == 0;
    if !from_el0 {
        return;
    }

    let cur = crate::process::current_pid();
    if cur == 0 {
        return;
    }

    // Quantum accounting + the same "preempt toward the input/focus target"
    // policy the x86 handler applies.
    let slice_expired = crate::process::account_tick_try(cur).unwrap_or(false);
    let focus = crate::wm::focus_pid();
    let target = if focus != 0 { focus } else { 1 };
    let should_preempt = slice_expired
        || (cur != 0
            && target != 0
            && cur != target
            && (crate::wm::input_pending_for(target) > 0
                || (target == 1 && crate::wm::embedder_baton_due())));
    {
        static PREEMPT_TICK_LOG: AtomicU32 = AtomicU32::new(0);
        let n = PREEMPT_TICK_LOG.fetch_add(1, Ordering::Relaxed);
        if n < 80 && (n % 8 == 0 || should_preempt) {
            log::warn!(
                "[arm-preempt-tick] #{} cur={} slice_expired={} should={} focus={}",
                n, cur, slice_expired, should_preempt, focus
            );
        }
    }
    if !should_preempt {
        return;
    }

    // Map the live EL0 frame → the shared `UserRegs` (x86-named subset) the
    // scheduler operates on. SP_EL0→rsp, ELR_EL1→rip, SPSR→rflags; args/
    // callee-saved per the Linux aarch64 ABI mapping used elsewhere in the port.
    let cur_regs = trapframe_to_userregs(f);

    // Snapshot the FULL ARM frame for `cur` so its scratch registers survive.
    // If the PTABLE lock is busy, skip preemption this tick (resume cur) rather
    // than switch with a half-saved frame.
    let full: [u64; 36] = frame_as_array(f);
    if !crate::process::arch_store_trapframe(cur, &full) {
        return;
    }

    let switch = crate::process::timer_preempt_switch_try(cur, &cur_regs);
    {
        static SWITCH_LOG: AtomicU32 = AtomicU32::new(0);
        let n = SWITCH_LOG.fetch_add(1, Ordering::Relaxed);
        if n < 60 {
            log::warn!(
                "[arm-preempt-switch] #{} cur={} -> next={:?} states={}",
                n, cur, switch.as_ref().map(|s| s.0),
                crate::process::debug_runnable_states()
            );
        }
    }
    if let Some((next_pid, next_regs, ttbr0)) = switch
    {
        // Switch the low-half translation base to the next thread's space.
        crate::arch::memory::write_cr3(ttbr0);

        // Restore the next thread's frame. Prefer its full ARM snapshot (taken
        // when IT was last timer-preempted); fall back to rebuilding from the
        // shared `UserRegs` for a thread that last yielded via a syscall / is
        // entering fresh.
        if let Some(saved) = crate::process::arch_take_trapframe(next_pid) {
            apply_array_to_frame(f, &saved);
        } else {
            userregs_to_trapframe(f, &next_regs);
        }
    }
}

/// Build the shared `UserRegs` from a live EL0 trap frame.
fn trapframe_to_userregs(f: &super::vectors::TrapFrame) -> crate::process::UserRegs {
    crate::process::UserRegs {
        rip: f.elr,
        rsp: f.sp_el0,
        rflags: f.spsr,
        rax: f.x[8],
        rdi: f.x[0],
        rsi: f.x[1],
        rdx: f.x[2],
        rcx: f.x[3],
        r8: f.x[4],
        r9: f.x[5],
        r10: f.x[9],
        r11: f.x[10],
        rbx: f.x[19],
        r12: f.x[20],
        r13: f.x[21],
        r14: f.x[22],
        r15: f.x[23],
        rbp: f.x[29],
    }
}

/// Write a shared `UserRegs` back into a live EL0 trap frame (lossy fallback
/// path — used only when the next thread has no full ARM snapshot).
fn userregs_to_trapframe(f: &mut super::vectors::TrapFrame, r: &crate::process::UserRegs) {
    f.elr = r.rip;
    f.sp_el0 = r.rsp;
    // Force a sane EL0t PSTATE (SPSR M[3:0]=0, interrupts unmasked) on resume.
    f.spsr = 0;
    f.x[8] = r.rax;
    f.x[0] = r.rdi;
    f.x[1] = r.rsi;
    f.x[2] = r.rdx;
    f.x[3] = r.rcx;
    f.x[4] = r.r8;
    f.x[5] = r.r9;
    f.x[9] = r.r10;
    f.x[10] = r.r11;
    f.x[19] = r.rbx;
    f.x[20] = r.r12;
    f.x[21] = r.r13;
    f.x[22] = r.r14;
    f.x[23] = r.r15;
    f.x[29] = r.rbp;
}

/// Serialise a trap frame to the 36-word array stored in `Process::arch_trapframe`.
/// Layout matches `vectors::TrapFrame` / the SAVE_FRAME stack image:
/// [0..31]=x0..x30, 31=SP_EL0, 32=ELR, 33=SPSR, 34=ESR, 35=pad.
fn frame_as_array(f: &super::vectors::TrapFrame) -> [u64; 36] {
    let mut a = [0u64; 36];
    a[..31].copy_from_slice(&f.x);
    a[31] = f.sp_el0;
    a[32] = f.elr;
    a[33] = f.spsr;
    a[34] = f.esr;
    a
}

/// Restore a 36-word saved array into a live trap frame.
fn apply_array_to_frame(f: &mut super::vectors::TrapFrame, a: &[u64; 36]) {
    f.x.copy_from_slice(&a[..31]);
    f.sp_el0 = a[31];
    f.elr = a[32];
    f.spsr = a[33];
    f.esr = a[34];
}

/// Finish any deferred MMIO mapping after `mm::init()`.
///
/// On x86 this maps the xAPIC MMIO window; on aarch64 the GIC MMIO regions would
/// be mapped here. No-op stub.
pub fn finish_xapic_init() {}

/// Per-AP interrupt-controller init.
///
/// TODO(arm): enable this core's GIC redistributor/CPU interface.
pub fn init_ap() {}

/// Signal End-Of-Interrupt to the controller.
///
/// TODO(arm): write the interrupt id to GICC_EOIR / ICC_EOIR1_EL1.
pub fn eoi() {}

/// Route the legacy serial/ExtINT line.
///
/// x86-specific (LINT0 in ExtINT mode). No aarch64 equivalent — no-op.
pub fn configure_lint0_for_extint() {}

/// Return this core's interrupt-controller id (x86 LAPIC-id analogue).
///
/// Derived from MPIDR_EL1 affinity bits.
pub fn local_apic_id() -> u32 {
    let v: u64;
    unsafe { core::arch::asm!("mrs {}, mpidr_el1", out(reg) v, options(nomem, nostack)) };
    (v & 0x00FF_FFFF) as u32
}

/// Send a reschedule IPI to the given target core.
///
/// TODO(arm): raise a GIC Software-Generated Interrupt (SGI) to `_target`.
pub fn send_resched_ipi(_target_lapic_id: u32) {}

// ── Vsync cadence ───────────────────────────────────────────────────────────

/// Set the desired vsync cadence (Hz). 0 disables it.
pub fn set_vsync_hz(hz: u32) {
    VSYNC_HZ.store(hz, Ordering::Release);
}

/// Returns true when a vsync interval has elapsed since the last reset.
///
/// TODO(arm): compare CNTVCT_EL0 against the last fired timestamp scaled by
/// CNTFRQ_EL0 / VSYNC_HZ. Always returns false in the scaffold.
pub fn vsync_due() -> bool {
    false
}

/// Reset the vsync interval reference timestamp.
///
/// TODO(arm): snapshot CNTVCT_EL0. No-op stub.
pub fn reset_vsync_last_tsc() {}
