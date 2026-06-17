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
- **M2 ✅ Physical-address-keyed futex.** `futex_wake_waiters` now scopes by the PHYSICAL
  address of the futex word (`futex_phys_of` = translate the waiter's VA via its page-table
  root), the address-space-independent identity; falls back to group-leader scoping only if
  a translation fails (never drops a wake the old path allowed). Behaviorally identical
  while apps share the shell's address space (same pml4 → same physaddr → same decision);
  becomes the correct key once engines are isolated into separate address spaces. *Verified:
  engine bring-up boots + renders the launcher (present→65, 0 faults).* NOTE: still takes
  `PTABLE_LOCK` to read each waiter's pml4 (`pml4_phys_of`) — fully lock-free lookup is a
  later optimization; correctness/keying is the M2 deliverable.
- **M3 ✅ Reschedule-IPI primitive on both arches.** x86 was already complete
  (`broadcast_resched_ipi` → `send_resched_ipi` → vector 0x40 → `apic_resched_handler`),
  and `set_state(Running)` already calls `broadcast_resched_ipi` on a wake. Filled the
  aarch64 gap: `gic::send_sgi_all_but_self`/`send_sgi` (write GICD_SGIR), `SGI_RESCHED`
  (SGI 0), `broadcast_resched_ipi` now sends it (guarded on `CPU_COUNT > 1` so single-core
  never touches the GIC — it's on the hot set_state path), and the IRQ handler recognizes
  + EOIs SGI 0. *Compile-verified both arches × {default, smp}; provably single-core-neutral
  (guarded no-op + the SGI-0 branch is unreachable on one core). SGI delivery + the actual
  rerun-scheduler-on-IPI action are exercised at M5 (APs idle until then).* NOTE: the wake
  sites still use the cooperative direct-enter on the same core; M3 adds the CROSS-core
  signal — replacing direct-enter wholesale waits for M4/M5's per-CPU run queues.
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
