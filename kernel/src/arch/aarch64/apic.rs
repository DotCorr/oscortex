//! Interrupt-controller + tick/vsync scaffolding — aarch64 (GIC + generic timer).
//!
//! On x86 this is the local APIC; on aarch64 the equivalent responsibilities are
//! split between the GIC (CPU interface: EOI, IPIs, routing) and the ARM generic
//! timer (CNTP/CNTV, the periodic tick that drives the vsync cadence). These are
//! compilable stubs preserving the public surface the shared kernel calls; real
//! GICv2/GICv3 + CNTV_CTL programming comes later.

use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};

/// Approximate timer ticks per millisecond (placeholder; real value is read
/// from `CNTFRQ_EL0` once the generic timer is programmed).
pub static APIC_TICKS_PER_MS: AtomicU32 = AtomicU32::new(62500);

/// Requested vsync cadence in Hz. Default 60 so the vsync boundary fires (and the
/// embedder's posted baton is delivered → FlutterEngineOnVsync → a frame is
/// produced & presented) even before/without the embedder calling set_vsync_hz.
/// Without a non-zero cadence, vsync_due() is always false, push_vsync is never
/// called, and the engine schedules frames forever without ever presenting one.
static VSYNC_HZ: AtomicU32 = AtomicU32::new(60);

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

    // Cross-core reschedule IPI (SMP, SGI 0). Taking the interrupt already woke
    // this core from wfi; EOI and return. Once APs run the scheduler (M5), this is
    // where the woken core re-picks a runnable thread.
    if intid == super::gic::SGI_RESCHED {
        super::gic::eoi(intid);
        return;
    }

    // Non-timer interrupts. virtio-mmio input SPIs (slot i → INTID 48+i) drive the
    // pointer/keyboard; drain them into the WM queue, then EOI. Everything else is
    // spurious/unhandled: EOI (if real) and return.
    if intid != super::timer::TIMER_PPI {
        if crate::drivers::virtio_input::owns_intid(intid) {
            crate::drivers::virtio_input::handle_irq();
        }
        if intid < 1020 {
            super::gic::eoi(intid);
        }
        return;
    }

    // Timer tick bookkeeping + re-arm the next interval.
    super::timer::on_tick();
    // EOI now so the GIC can deliver the next timer interrupt right after eret.
    super::gic::eoi(intid);

    // SMP: a secondary core that has NOT yet engaged user scheduling does ZERO
    // work here (just the EOI above, which also wakes it from wfi to re-poll). This
    // keeps an idling AP from touching PTABLE_LOCK / the wake machinery and thus
    // from contending with the BSP during the engine's single-core bring-up — the
    // contention that previously broke app launch under smp>1. The BSP (cpu 0) is
    // always engaged.
    if !super::smp::cpu_engaged(super::smp::current_cpu_id()) {
        return;
    }

    // Wake threads whose timerfd deadlines have elapsed, and deliver any pending
    // cross-thread wakes — exactly as the x86 APIC-timer ISR does (idt.rs). On
    // aarch64 this was missing, so the engine's UI/raster/IO worker threads that
    // epoll_wait on a timerfd went Blocked and were NEVER transitioned back to
    // Running → the cooperative scheduler found no runnable thread and the whole
    // engine deadlocked right after spawning its worker pool. The try_-variants
    // are ISR-safe (skip on lock contention rather than deadlock).
    crate::syscall::check_timerfds_and_wake_try();
    crate::process::handle_pending_wakes_try();

    // Periodic deferred task-runner kick (mirrors the x86 APIC ISR, idt.rs
    // ~586-596). The cooperative engine reaches a fully-parked bootstrap state
    // during FlutterEngineRunInitialized: every thread waits on a cond/epoll and
    // the wake that would un-park the isolate-bootstrap worker comes from a thread
    // that is itself parked (wake-before-anyone-can-wake). x86 breaks this with a
    // periodic KICK_REQUESTED pulse that the in-syscall wait/unlock loops consume
    // to call force_wake_all_task_runners (which pokes EVERY timerfd/eventfd, armed
    // or not, releasing every epoll waiter). aarch64 never set this flag, so the
    // engine deadlocked deterministically. We only set the flag here (ISR-safe);
    // force_wake_all_task_runners itself takes blocking locks and must run in
    // syscall context, where the wait loops consume the flag.
    {
        static TR_KICK_CTR: AtomicU32 = AtomicU32::new(0);
        let k = TR_KICK_CTR.fetch_add(1, Ordering::Relaxed);
        if k % 256 == 0 {
            crate::syscall::KICK_REQUESTED.store(true, Ordering::Release);
        }
    }

    // Drive the vsync boundary on every tick (even kernel-mode ticks), mirroring
    // the x86 APIC ISR (idt.rs). wm::tick()/compositor::tick() advance the window
    // manager + compositor and arm the embedder's vsync baton; without this the
    // engine schedules frames forever (schedule_frame label=idle) but the baton is
    // never delivered, so FlutterEngineOnVsync is never called and no frame is
    // ever produced or presented. (aarch64 had only a no-op vsync_due() scaffold.)
    // GLOBAL per-tick work (vsync baton, WM tick, USB poll) runs on the BSP ONLY —
    // these are single-driver subsystems; letting a 2nd core drive them too would
    // double-deliver vsync batons / double-poll USB. APs only do per-CPU scheduling
    // below.
    if super::smp::current_cpu_id() == 0 && vsync_due() {
        crate::wm::tick();
        // Deliver the vsync baton ONLY — do NOT call compositor::tick() here: its
        // render_frame() does a full-screen blit/fill that, in the timer ISR under
        // slow emulation, starves the engine's main thread (boot livelocked in
        // fb::fill_rect at "[host] starting"). Flutter frames render via the
        // present syscall path (present_surface → render_frame) in normal context.
        crate::compositor::vsync_baton_tick();
        reset_vsync_last_tsc();
        // Poll the USB xHCI runtime for HID input (mouse/keyboard) on the vsync
        // boundary — mirrors the x86 APIC ISR (idt.rs). Drains transfer events
        // (→ wm::push_pointer/push_key) and re-arms the interrupt transfer.
        crate::drivers::usb::poll();
    }

    // Only the EL0-preempt switch below requires an interrupted EL0 frame. SPSR_EL1
    // M[3:0] == 0 means the interrupted context was EL0t.
    let from_el0 = (f.spsr & 0xF) == 0;
    if !from_el0 {
        // KERNEL-MODE WAKE-ASSIST — direct port of the x86 APIC ISR (idt.rs).
        // The aarch64 engine threads (UI/raster/IO, pids 2-7) spend ~all their time
        // Blocked in EL1 syscalls (epoll_wait/futex/cond), and pid 1 spins in its
        // wm_event_wait loop at EL1. So most timer ticks land in kernel mode
        // (from_el0=false). Without this, those ticks were dropped — the engine's
        // UI thread Animator never got the CPU to call vsync_callback → no baton →
        // no frame ever presented; and the bootstrap stalled (~7/8 boots) waiting
        // for a wake that never re-entered a runnable thread. When a due vsync
        // baton (pid 1) or queued input exists and the current thread is idle/
        // blocked, DRIVE the target from kernel context — exactly what x86 does.
        // enter_user_by_pid_noreturn_try resumes via the cooperative path
        // (build_image → eret_to_el0, which resets SP_EL1 to the syscall stack, so
        // the abandoned IRQ frame is reclaimed). EOI already happened above.
        let cur = crate::process::current_pid();
        let focus = crate::wm::focus_pid();
        let target = if focus != 0 { focus } else { 1 };
        // HOME-CORE per-core wake-assist (mirrors x86 idt.rs). Only the target's HOME
        // core re-enters it: the BSP pumps the shell (home 0), an AP pumps the app
        // pinned to it (home 1+). try_claim_cpu_for_try is home-gated so this can't
        // steal another core's thread. The ISR-initiated resume below was previously
        // disabled (// BISECT) as the suspected cause of "pid=2 Dart corruption" — but
        // that predates the 2026-06-16 per-CPU page-table-lock + ABBA lock-order fixes,
        // the real corruption source. Re-enabled: the app's engine bootstrap stalls at
        // FlutterEngineRunInitialized without it (the worker the isolate launch waits
        // on is parked and only this resume re-enters it), exactly as on x86.
        let my_cpu = super::smp::current_cpu_id();
        let target_is_mine = crate::process::home_cpu_of_try(target) == Some(my_cpu);
        if target != 0 && target_is_mine
            && (cur == 0 || crate::process::is_blocked_try(cur))
            && (crate::wm::input_pending_for(target) > 0
                || crate::wm::embedder_baton_due(target))
            && cur != target
        {
            // WAKE ONLY — do NOT do the ISR-initiated resume here. The prior session
            // fingered enter_user_by_pid_noreturn_try from the timer ISR as the cause
            // of "pid=2 Dart corruption", and re-enabling it reproduced exactly that:
            // a real EL0 data abort (EC=0x24) in engine code on the AP. Marking the
            // target Running and letting the cooperative hand-off (the sys_thread_create
            // immediate-enter + futex hand-off, which complete the engine bootstrap on
            // x86) enter it avoids resuming a thread from an ISR mid-EL1-critical-state.
            let _ = crate::process::set_state_try(target, crate::process::ProcState::Running);
        }
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
    // EL0 preemption stays OFF — empirically it does NOT fix the post-app-launch
    // freeze and currently can't even fire: in a 45s render test the timer ISR
    // landed in EL1 (kernel) on EVERY tick (arm-preempt-tick=0), because the threads
    // spend ~all their time in EL1 syscalls / cooperative-handoff loops. And the
    // freeze itself is a multi-threaded Dart GC stop-the-world safepoint that can't
    // converge on ONE core — the deadlocked engine threads are blocked in EL1
    // cond_wait/epoll, which EL0 preemption can't touch. The real fixes are SMP
    // (parallel cores → the GC converges) or serial GC (engine allowlist patch); see
    // [[post-app-launch-freeze]]. Preemption is still wanted for the general
    // starvation class, but needs EL0 IRQ delivery wired first (separate work).
    let preempt_enabled = false;
    let should_preempt = preempt_enabled
        && (slice_expired
            || (cur != 0
                && target != 0
                && cur != target
                && (crate::wm::input_pending_for(target) > 0
                    || crate::wm::embedder_baton_due(target))));
    {
        static PREEMPT_TICK_LOG: AtomicU32 = AtomicU32::new(0);
        let n = PREEMPT_TICK_LOG.fetch_add(1, Ordering::Relaxed);
        if n < 4000 && (n % 64 == 0 || should_preempt) {
            log::warn!(
                "[arm-preempt-tick] #{} cur={} slice_expired={} should={} focus={} states={}",
                n, cur, slice_expired, should_preempt, focus,
                crate::process::debug_runnable_states()
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
    let full: [u64; 102] = frame_as_array(f);
    if !crate::process::arch_store_trapframe(cur, &full) {
        return;
    }

    let switch = crate::process::timer_preempt_switch_try(cur, &cur_regs);
    {
        static SWITCH_LOG: AtomicU32 = AtomicU32::new(0);
        let n = SWITCH_LOG.fetch_add(1, Ordering::Relaxed);
        if n < 400 {
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

        // Set the next thread's TLS base (TPIDR_EL0). The cooperative
        // enter_user_by_pid_noreturn path does this via set_fs_base, but the
        // timer-preempt path did NOT — so a thread first scheduled by a timer
        // tick (e.g. a freshly-spawned Dart VM worker) ran with TPIDR_EL0 left
        // at the previous thread's value (or 0), and its first thread-local
        // access (dart::ThreadRegistry::GetFreeThreadLocked) faulted near null.
        crate::arch::cpu::set_fs_base(crate::process::get_proc_fs_base(next_pid));

        // Restore the next thread's frame. Prefer its full ARM snapshot (taken
        // when IT was last timer-preempted); fall back to rebuilding from the
        // shared `UserRegs` for a thread that last yielded via a syscall / is
        // entering fresh.
        if let Some(saved) = crate::process::arch_take_trapframe_try(next_pid) {
            apply_array_to_frame(f, &saved);
        } else {
            userregs_to_trapframe(f, &next_regs);
            // The shared `UserRegs` carries no slot for the aarch64 callee-saved
            // x24–x28 or the link register x30, so `userregs_to_trapframe` left
            // those at the *preempted* thread's values. Patch them from next's
            // saved snapshot — otherwise a thread resumed here after a cooperative
            // syscall yield inherits a stale x24–x28 (live Dart heap pointers) or
            // a stale x30 (its next `ret` branches to a leaked value, e.g. a
            // screen dimension). Mirrors what `build_image`/EnterUserRegs restore
            // on the cooperative path, making both resume routes lossless.
            if let Some((x24, x25, x26, x27, x28, lr)) =
                crate::process::aarch64_resume_extras_try(next_pid)
            {
                f.x[24] = x24;
                f.x[25] = x25;
                f.x[26] = x26;
                f.x[27] = x27;
                f.x[28] = x28;
                f.x[30] = lr;
            }
            // RETURN-mode threads (resumed *after* the svc) must receive the
            // syscall result in x0; userregs_to_trapframe wrote x0 = rdi (arg0).
            // next_regs.rax holds the result (set via set_rax). RE-EXEC threads
            // keep x0 = arg0 (the flag is false), so this is a no-op for them.
            if crate::process::aarch64_ret_in_x0_try(next_pid) == Some(true) {
                f.x[0] = next_regs.rax;
            }
        }
    } else {
        // No switch this tick (no OTHER runnable thread): `cur` keeps running and
        // erets via the UNMODIFIED live frame `f`, advancing past this PC. But we
        // already stored a full snapshot above and set `arch_frame_valid` — which
        // is now STALE (it describes a PC `cur` is about to run past). Discard it,
        // or a later COOPERATIVE resume of `cur` (enter_user_by_pid_noreturn, which
        // now prefers the full frame) would replay this stale frame and branch to
        // a wrong PC / leaked x30. Consume-and-drop clears the valid flag.
        let _ = crate::process::arch_take_trapframe_try(cur);
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

/// Serialise a trap frame to the 102-word array stored in `Process::arch_trapframe`.
/// Layout matches `vectors::TrapFrame` / the SAVE_FRAME stack image:
/// [0..31]=x0..x30, 31=SP_EL0, 32=ELR, 33=SPSR, 34=ESR, 35=pad,
/// [36..100)=v0..v31 (2 words each), 100=FPSR, 101=FPCR.
fn frame_as_array(f: &super::vectors::TrapFrame) -> [u64; 102] {
    let mut a = [0u64; 102];
    a[..31].copy_from_slice(&f.x);
    a[31] = f.sp_el0;
    a[32] = f.elr;
    a[33] = f.spsr;
    a[34] = f.esr;
    for (i, lane) in f.v.iter().enumerate() {
        a[36 + i * 2] = lane[0];
        a[36 + i * 2 + 1] = lane[1];
    }
    a[100] = f.fpsr;
    a[101] = f.fpcr;
    a
}

/// Restore a 102-word saved array into a live trap frame (GPRs + FP/SIMD).
fn apply_array_to_frame(f: &mut super::vectors::TrapFrame, a: &[u64; 102]) {
    f.x.copy_from_slice(&a[..31]);
    f.sp_el0 = a[31];
    f.elr = a[32];
    f.spsr = a[33];
    f.esr = a[34];
    for i in 0..32 {
        f.v[i][0] = a[36 + i * 2];
        f.v[i][1] = a[36 + i * 2 + 1];
    }
    f.fpsr = a[100];
    f.fpcr = a[101];
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

/// CNTVCT_EL0 snapshot at the last fired vsync boundary.
static VSYNC_LAST_CNT: AtomicU64 = AtomicU64::new(0);

#[inline]
fn read_cntvct() -> u64 {
    let v: u64;
    unsafe { core::arch::asm!("mrs {}, cntvct_el0", out(reg) v, options(nomem, nostack)) };
    v
}

#[inline]
fn read_cntfrq() -> u64 {
    let v: u64;
    unsafe { core::arch::asm!("mrs {}, cntfrq_el0", out(reg) v, options(nomem, nostack)) };
    v
}

/// Returns true when a vsync interval has elapsed since the last reset.
///
/// Mirrors the x86 rdtsc-based cadence using the generic timer: the period in
/// counter ticks is CNTFRQ_EL0 / VSYNC_HZ. Self-advances the reference on a hit
/// (like x86) so the caller's explicit reset is idempotent.
pub fn vsync_due() -> bool {
    let hz = VSYNC_HZ.load(Ordering::Relaxed);
    if hz == 0 { return false; }
    let freq = read_cntfrq();
    if freq == 0 { return false; }
    let period = freq / hz as u64;
    if period == 0 { return false; }
    let now = read_cntvct();
    let last = VSYNC_LAST_CNT.load(Ordering::Relaxed);
    if now.wrapping_sub(last) >= period {
        VSYNC_LAST_CNT.store(now, Ordering::Relaxed);
        true
    } else {
        false
    }
}

/// Reset the vsync interval reference timestamp to now.
pub fn reset_vsync_last_tsc() {
    VSYNC_LAST_CNT.store(read_cntvct(), Ordering::Relaxed);
}
