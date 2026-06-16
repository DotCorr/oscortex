# OSCortex SMP — Multi-Core Scheduler Architecture & Roadmap

Status: **single-core by default** (the `smp` Cargo feature gates AP bring-up off — see
`kernel/src/arch/*/smp.rs`). This document is the principled plan to make OSCortex
*actually scale across cores*, replacing the cooperative single-core scheduler with a
Redox-style per-CPU preemptive one. It exists because six prior patch-and-pray SMP
attempts failed for lack of a plan; build to this, incrementally, never breaking the
single-core path.

## Why single-core today

The post-app-launch freeze is a multi-threaded Dart-VM GC **stop-the-world safepoint**
that cannot converge on one core (the GC threads need to run *simultaneously* to all
reach the safepoint). Single-core preemption was tried — it does not fix it. Multi-core
is the real fix. But naively turning APs loose on the current scheduler corrupted state
(6 attempts), and AP bring-up itself hung a real Skylake at boot. So: single-core is the
*stable* config now; SMP is a methodical foundation upgrade.

## Current model (what we're replacing)

- **User scheduling = cooperative hand-off.** `enter_user_by_pid_noreturn(pid)` restores a
  process's full context and *never returns*; every blocking primitive (futex, cond,
  mutex, epoll, fd) yields by calling it. Wakers **directly enter the woken thread**.
- **Preemption = timer-ISR frame rewrite.** The timer ISR saves the full trap frame,
  calls `timer_preempt_switch_try` to pick the next thread, overwrites the live frame, and
  returns into it. EL0 preemption is currently **disabled on aarch64** (`preempt_enabled =
  false`) and quantum-gated on x86.
- **Global lock.** `PTABLE_LOCK` (recursive, per-CPU holder + recursion counts) guards the
  process table; ISR paths use `try_lock` to stay deadlock-free.
- **Sync.** Futex table is `BTreeMap<VA, Vec<pid>>` with a `get_group_leader` address-space
  filter (interim cross-process fix). Cond/mutex are cooperative + a `force_wake_all_task_
  runners` deadlock-breaker that pokes every timerfd.
- A cooperative kernel `context_switch(old_sp, new_sp)` (callee-saved only) **already
  exists** on both arches — this is the `switch_to_inner` primitive Redox uses.

## Target model (Redox blueprint, adapted)

1. **Per-CPU scheduler state + affinity run queue** — each CPU has its own `current`/`idle`
   context; thread selection filters by `sched_affinity`. (We already have sticky
   `current_cpu` affinity + per-CPU `current_pid`.)
2. **`CONTEXT_SWITCH_LOCK` + non-preemptable bail-out** — one atomic serializes switches;
   never switch *away* from a thread that is non-preemptable (holding a kernel lock, or at
   a non-safepoint). This is the piece that corrupted preempted Dart threads.
3. **Two-layer `switch_to(prev,next)` / naked `switch_to_inner`** — exact callee-saved +
   FP/cr3/ttbr save sets. (`context_switch` is the inner half; add the high-level wrapper.)
4. **Physical-address-keyed futex** — key the waiter table by the *physical* address of the
   futex word (+ address-space validation), replacing VA+group-scoping. Lock-free vs
   `PTABLE_LOCK`; correct across cores and across the shell/app shared-VA collision.
5. **IPI-based wakeups** — wakers `set_state(Running)` + send a reschedule IPI to the
   target core; they do **not** jump into the woken thread. (x86 `broadcast_resched_ipi`
   exists; aarch64 needs the GIC SGI.)

## Milestones (each keeps single-core working; AP code gated on the `smp` feature)

- **M0 ✅ AP bring-up → idle.** Secondary cores boot and park (done; gated off by default).
- **M1 ⏳ Safe-preemption core: `preempt_disable()` + `CONTEXT_SWITCH_LOCK`.** Per-CPU
  preempt-disable depth (auto-engaged while `PTABLE_LOCK` is held); the timer-preempt path
  bails when this CPU is non-preemptable, so we never switch away from a global-lock holder.
  Additive + dormant where preemption is off; real effect on the x86 quantum-preempt path.
  *Verify: both arches still boot + render single-core, no regression.*
- **M2 Physical-address-keyed futex.** Translate the user VA → physaddr; key/validate by it.
  Removes `PTABLE_LOCK` from the wake path. *Verify: engine bring-up still boots (the futex
  is load-bearing for it); cross-process no false wakes.*
- **M3 IPI-based wakeup model.** Convert the `enter_user_by_pid_noreturn` wake sites to
  `set_state(Running)` + reschedule IPI. Implement the aarch64 GIC-SGI reschedule IPI.
  *Verify: single-core unaffected (IPI is a no-op on 1 core; scheduler picks woken threads).*
- **M4 Two-layer `switch_to` + per-CPU current/idle contexts.** Build the high-level switch
  over `context_switch`; give each CPU an idle context. *Verify: BSP uses it with no
  regression; an AP runs a per-CPU idle loop (heartbeat log) under `--features smp`.*
- **M5 Turn APs loose + verify the freeze is gone.** APs pull runnable threads (affinity
  run queue); foreground app's engine threads distribute across cores; GC safepoint
  converges. *Verify: launch an app under `-smp 2/4`, sweep, no freeze; then UTM + Mac.*

## Hard rules
- Never break the single-core boot. All AP/SMP-only code is behind `#[cfg(feature="smp")]`
  or a `CPU_COUNT > 1` runtime check.
- Verify every milestone in QEMU (both arches) before committing; only verified work ships.
- `is_preemptable`/non-preemptable bail is the safety net for preempting Dart — do not
  enable EL0 preemption broadly until safepoint cooperation or the bail covers it.

See also: kernel/src/process/mod.rs (scheduler), kernel/src/arch/*/smp.rs (per-CPU + AP),
kernel/src/syscall/handlers/futex.rs (futex), kernel/src/arch/*/{idt,apic,vectors}.rs
(preemption). Memory: the Redox patterns + prior-attempt log live in the agent's notes.
