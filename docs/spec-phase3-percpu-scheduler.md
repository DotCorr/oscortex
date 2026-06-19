# Spec — Phase 3: Per-CPU Preemptive Scheduler

> **Status: STARTED (2026-06-18) — `percpu-sched` cargo feature (default OFF) + a clean per-CPU PICK
> landed behind it; flag-on boots + renders + launches Files RESPONSIVE on x86 `-smp 1`. Shipping path
> (flag off) byte-identical. See "Progress" at the end.** Companion to
> [foundation-redesign.md](foundation-redesign.md)
> (Part B/Phase 3). Produced 2026-06-18 (overnight) by a read-only mapping pass over the real tree on
> `feat/native-engine-port`. **Line numbers are as-of-2026-06-18 — re-verify at implementation.**
> This is the highest-risk phase — it touches the kernel's heart and lands **behind a cargo feature
> (default OFF)** so the shipping cooperative path stays byte-identical. Do not implement before the
> syscall-correctness work (Phase 1 + Phase 2a) lands — S6/S9 deletion depends on it (§3).

**Target:** Per-CPU scheduler state + affinity-filtered run queue; ONE `CONTEXT_SWITCH_LOCK` with an
`is_preemptable` bail-out; two-layer `switch_to`/`switch_to_inner`; simple round-robin.

---

## 1. CURRENT STATE

### 1.1 Scheduler data structures

| Structure | Location | Role |
|---|---|---|
| `struct Process` | process/mod.rs:117-265 | PCB. Sched fields: `pml4_phys`(121), `regs`(123), `state`(125), `slice_left`(147), `current_cpu`(176), `is_preemptable`(183), `home_cpu`(190); aarch64 `arch_trapframe:[u64;102]`(202)+flags. |
| `static mut PTABLE: [Process; MAX_PROCS]` | process/mod.rs:567-570 | The ONLY run queue — a flat 256-slot array; "the queue" is a linear scan. No per-CPU queues. |
| `PTABLE_LOCK` | process/mod.rs:675 (impl 575-671) | ONE global CAS'd `owner: AtomicU32` over all of PTABLE. Acquire → `preempt_disable_cpu`(630); drop → `preempt_enable_cpu`(662). |
| `PREEMPT_DEPTH: [AtomicU32; MAX_CPUS]` | process/mod.rs:691-692 | Per-CPU non-preemptable depth. **The real non-preemptable mechanism** (not `is_preemptable`). |
| `PerCpuData` (x86) | arch/x86_64/smp.rs:46-52 | `{cpu_id, lapic_id, online, current_pid}`. Only `current_pid` is mutable sched state. No queue/idle/preempt-flag. |
| `PerCpuData` (aarch64) | arch/aarch64/smp.rs:18-29 | adds `engaged` (the AP-park gate). |
| `CONTEXT_SWITCH_LOCK: AtomicBool` | process/mod.rs:754 | **`#[allow(dead_code)]`, never referenced.** |
| `Scheduler` (kernel tasks) | sched/mod.rs:49-63 | A SEPARATE round-robin for kernel-only tasks with its own `context_switch` — disjoint from the user scheduler. |

### 1.2 The PICK function

`next_runnable_pid_locked(current, my_cpu)` — **process/mod.rs:1543-1656**. Called under PTABLE_LOCK from
`next_runnable_pid`(1658), `timer_preempt_switch`(2445), `timer_preempt_switch_try`(2515). It interleaves
the round-robin scan (1622-1640) with *every* SMP + foreground policy inline: fg/exclusive compute
(1554-1569), `smp`(1580), input shortcut (1587-1599), vsync-baton shortcut (1604-1614), the scan with
home_cpu+exclusive+claim filters (1622-1640), stay-on-current fallback (1642-1653). No policy/mechanism
split — which is what makes every special-case a localized edit to this one function.

### 1.3 Every SMP special-case

| # | Special-case | file:line | Why it exists |
|---|---|---|---|
| S1 | **Foreground-exclusive** | process/mod.rs:1543-1569, filter 1629/1646 | Two concurrent heavy Flutter VMs overwhelm the cooperative save/restore → a backgrounded host switches in with a corrupted callee-saved reg → #GP. Serialize to one VM group. |
| S2 | **home-core pinning** | assign 972-976; filter 1626-1628,1591,1607,1645 | App host pinned to its own core so the engine runs with its OWN per-CPU GPR scratch. |
| S3 | **Sticky `current_cpu` claim** | 2054-2064 (not released on hand-off); writes 1594,1634,2072 | Releasing the claim per hand-off → both cores chase the same threads → ~1fps livelock. |
| S4 | **Vsync-baton** | PICK 1604-1614; ISR idt.rs:670-679 / apic.rs:168-181; wake-assist idt.rs:693, apic.rs:217 | Pumps a launched app's frames on its core (else `FlutterEngineOnVsync` is never called). |
| S5 | **De-starve / input-priority** | PICK 1587-1599; `[destarve-epoll]` poll.rs:~1088; should_preempt idt.rs:688-694 | Engine workers busy-loop `epoll_wait`, never yield → input/baton starves → force a preempt. |
| S6 | **KICK / force-wake** | set idt.rs:650-660 (~500 ticks) / apic.rs:150-156 (256); consumed poll.rs:427 `force_wake_all_task_runners` | Break cooperative deadlocks: the thread that would wake is itself parked → periodic pulse pokes every timerfd/eventfd to release epoll waiters. |
| S7 | **Bootstrap-spin gate** | wm/mod.rs:359-360 `flutter_bootstrap_spin_active`; gates idt.rs:692, apic.rs:769 | Suppress preempt/wake churn during single-threaded engine bring-up. |
| S8 | **AP idle-park** | x86 sched/mod.rs:92-122 + idt.rs:636-639; aarch64 smp.rs:277-308 + apic.rs:125-127 + `CPU_HAS_HOME_WORK` 1223-1236 | An AP polling PTABLE_LOCK every tick stalls the BSP's first frame → AP stays in wfi/hlt, ISR does ZERO work, until an app is pinned to it. |
| S9 | **Wake-assist ISR re-entry** | x86 idt.rs:744-780; aarch64 apic.rs:183-260 | Cooperative hand-off can't re-enter a force_wake-released thread → ISR directly enters the home target when blocked + baton/input due. FP-safety + home-gated. |
| S10 | **enter_user SMP claim back-off** | process/mod.rs:2065-2089 | A hand-off may target a pid another core runs → re-pick instead of panic. |

**Two dead Redox primitives (zero callers — confirmed):**
- `is_preemptable: bool` (process/mod.rs:183) — set by `set_preemptable`(1197), read only by the public
  `is_preemptable(pid)` accessor (1261) **which has no callers**. The preempt bail checks
  `preempt_disabled()` (PREEMPT_DEPTH>0), **not** this field.
- `CONTEXT_SWITCH_LOCK` (process/mod.rs:754) — declared, never read.

So today: preemption is gated only by per-CPU kernel-lock depth (`PREEMPT_DEPTH`); user-thread switching
*rewrites the IRQ trap frame* (not a `switch_to`); aarch64 EL0 preemption runs through the cooperative
re-entry path with the full-fidelity frame saved in `arch_trapframe`.

---

## 2. TARGET DESIGN

### 2.1 Per-CPU state struct (both arches converge)

```rust
pub struct PerCpuData {
    pub cpu_id: u32, pub lapic_id: u32 /* mpidr on aarch64 */, pub online: AtomicBool,
    // ── scheduler state (NEW; was global) ──
    pub current_pid:  AtomicU32,                  // already exists
    pub idle_ctx:     UnsafeCell<KernelContext>,  // switch_to_inner target when the queue empties
    pub run_queue:    SpinMutex<RunQueue>,        // this CPU's ready ring; affinity-filtered AT ENQUEUE
    pub need_resched: AtomicBool,                 // set by IPI/timer; consumed at the next safepoint
}
pub struct RunQueue { slots: [u32; MAX_PROCS], head: usize, len: usize }  // fixed-cap, no alloc
```

**Affinity lives at enqueue, not in the scan** — a thread goes only into `PerCpuData[home_cpu].run_queue`,
so PICK has no home filter (S2 dissolves into the data structure). `idle_ctx` makes the AP's
implicit wfi/hlt loop (S8) an explicit idle context.

### 2.2 Affinity-filtered run-queue + round-robin pick

```
fn pick_next(cpu) -> Option<pid>:            // under run_queue lock + CONTEXT_SWITCH_LOCK
    rq = PER_CPU[cpu].run_queue
    for i in 0..rq.len:                       # one rotation, O(ready) not O(256)
        pid = rq.slots[(rq.head + i) % rq.cap]
        p = PTABLE[idx_of(pid)]
        if p.state != Running: continue        # lazily skip dead/blocked
        if !affinity_ok(p, cpu): continue      # see E1 variants (§4)
        rq.head = (rq.head + i + 1) % rq.cap    # round-robin cursor advance
        return Some(pid)
    return None                               # → switch_to_inner(idle_ctx[cpu])

fn enqueue(pid):                              # on wake / spawn / unblock
    cpu = PTABLE[idx_of(pid)].home_cpu        # affinity decided ONCE, here
    PER_CPU[cpu].run_queue.push_if_absent(pid)
    PER_CPU[cpu].need_resched = true
    if cpu != this_cpu(): send_resched_ipi(cpu)   # replaces broadcast KICK
```

No priorities/fair-share (matches the "simple round-robin" target). Replaces the 1622-1640 scan + all
inline policy except the affinity predicate (§4).

### 2.3 `CONTEXT_SWITCH_LOCK` + `is_preemptable` bail — exact enforcement points

```rust
fn maybe_switch(cpu, cur_pid):
    if !is_preemptable_now(cpu) { return; }           // ← the bail
    if CONTEXT_SWITCH_LOCK.compare_exchange(false,true,Acquire,Relaxed).is_err() { return; }
    let next = pick_next(cpu);
    switch_to(cur_pid, next);                           // two-layer (§2.4)
    // CONTEXT_SWITCH_LOCK released by the NEW thread after it resumes inside switch_to
```

**`is_preemptable` MUST be false at exactly:**
1. **Holding any kernel-visible lock** — today PTABLE_LOCK (auto-bump 630/drop 662). Extend the same
   discipline to the WM lock + futex/epoll table locks (the ABBA-prone ones) so a switch can never
   strand them cross-core.
2. **Inside an explicit critical region** — the page-table section already brackets with
   `preempt_disable/enable` (732-748).
3. **At a non-safepoint** — when the running thread holds a *userspace* lock the kernel knows about
   (engine enters a futex/cond wait or a userspace mutex on behalf of a Dart isolate). **This is the
   slot `is_preemptable` was designed for (doc-comment 177-183) but never wired** — the principled
   replacement for S1: instead of banning concurrency wholesale, ban *preemption at the unsafe instant*.

**`PREEMPT_DEPTH` vs `is_preemptable`:** keep both, distinct jobs. `PREEMPT_DEPTH`/`preempt_disabled()` →
points 1+2 (this CPU is in a kernel critical section). `is_preemptable` → point 3 (this thread is in a
userspace critical section that may migrate). The bail checks `!preempt_disabled() && is_preemptable(cur)`.

### 2.4 Two-layer `switch_to` / `switch_to_inner` — and the trap-frame reconciliation

**Today there is no user-thread `switch_to`** — user switching rewrites the hardware trap frame in place
(idt.rs:565-582 x86 / aarch64 frame restore), cooperative hand-off rebuilds via
`enter_user_by_pid_noreturn`. The only real `switch_to` is the kernel-task one (sched/mod.rs →
`arch::context_switch`, arch/x86_64/mod.rs:107-130, arch/aarch64/mod.rs:191-216). Target unifies:

- **`switch_to(prev,next)`** (Rust, under CONTEXT_SWITCH_LOCK): save FP/XSTATE, update `current_pid`,
  swap CR3/TTBR0 (`pml4_phys`), set syscall stack top + FS base/TPIDR → tail-call →
- **`switch_to_inner(&mut prev.kctx, &next.kctx)`** (naked asm): save prev callee-saved → load next →
  `ret`/`eret`. The new thread resumes *inside* `switch_to` and releases CONTEXT_SWITCH_LOCK.

**Callee-saved sets (already correct in `arch::context_switch` — reuse for the kernel-context half):**
- x86_64: `rbx, rbp, r12-r15` + RIP via `ret`; RSP via `[old_sp]`/`new_sp` (arch/x86_64/mod.rs:110-128).
- aarch64: `x19..x28, x29(FP), x30(LR)` stp-pairs + SP (arch/aarch64/mod.rs:194-214).

**The key subtlety — the 102-word frame is COMPLEMENTARY, not in conflict:**
- aarch64: the EL1 vector stub already saves the **entire** EL0 file (`x0..x30, SP_EL0, ELR, SPSR, ESR,
  v0..v31, FPSR/FPCR`) = **816 bytes = 102×u64** before any Rust runs (vectors.rs:38-50), copied to the
  PCB's `arch_trapframe`(202). So there is **no need for `switch_to_inner` to save user GPRs** — the full
  frame already exists; the two-layer switch needs only the *kernel* context (x19..x30) for the
  switched-while-in-syscall case. **Do not duplicate the FP save** — the vector stub's pre-NEON capture
  (vectors.rs:26-31) is the single source of the FP file.
- x86_64: the timer ISR pushes all 15 GPRs → `TimerTrapFrame`(idt.rs:467-488), rewritten from `next.regs`
  (565-582). XSTATE saved separately (process/mod.rs:2464/2483). Keep this ISR frame as the user
  snapshot; `switch_to_inner` handles only the 6-register kernel set + RSP.

**Net:** 102-word frame = **user** context (restored on eret); 6/12-register `KernelContext` = **kernel**
context (restored on ret between two in-kernel threads). The migration *adds* the explicit KernelContext
half + CONTEXT_SWITCH_LOCK discipline; it does **not** replace the existing trap-frame machinery.

---

## 3. SPECIAL-CASES DELETED

| # | Special-case | Deleted? | Why |
|---|---|---|---|
| S2 | home-core pinning | **Deleted (absorbed)** | Affinity at enqueue; PICK has no home filter. |
| S3 | sticky claim | **Deleted** | A pid lives in exactly one CPU's queue → two cores can't chase it. |
| S8 | AP idle-park | **Deleted** | Empty queue → switch to `idle_ctx` with IRQs on; timer just sets `need_resched` (no PTABLE_LOCK). AP never touches the global table → no BSP contention. Remove `engaged`/`CPU_HAS_HOME_WORK`/both ISR early-returns. |
| S10 | claim back-off | **Deleted** | No cross-core claim races: one queue, one CPU. |
| S5 | de-starve/input-priority | **Mostly deleted** | True preemption preempts a busy `epoll_wait` on its slice regardless; input target just enqueued + resched-IPI'd. (A soft focus *bias* may stay, not a crutch.) |
| S9 | wake-assist re-entry | **Mostly deleted** | Wake → state Running + enqueue + need_resched; picked next tick by home CPU. **Caveat: only fully true once the syscall-layer wake correctness lands** (Phase 1/2a) — until then a residual kick may be needed. |
| S6 | KICK/force-wake | **Deleted IFF syscall-layer fixed** | The pulse exists only because parked threads miss wakes — a *syscall-emulation* gap, not a scheduler gap. **Owned by the syscall rework; blocked on Phase 1/2a.** |
| S7 | bootstrap-spin gate | **Retained, simplified** | During bring-up the engine's threads simply aren't enqueued yet → the gate becomes a natural no-op; keep `flutter_init_ready` only if a measurable bring-up regression appears. |
| S4 | vsync-baton | **Retained (not a sched bug)** | The compositor↔engine vsync contract; delivered by enqueuing the embedder thread + resched-IPI instead of the PICK shortcut. |
| S1 | foreground-exclusive | **E1-DEPENDENT (§4)** | The one special-case that may not be deletable — depends on Dart isolate-confinement. |

---

## 4. THE E1 DEPENDENCY (Dart isolate confinement)

**The only in-code statement of the constraint** is the foreground-exclusive justification in PICK:

> process/mod.rs:1574-1579 — "We KEEP foreground-exclusive … letting the shell run concurrently with the
> app trips Dart's isolate-confinement check (**\"Isolate main is owned by os thread X, failed to
> schedule from os thread Y\" → sys_abort**)."

There is **no separate enforcement primitive** — confinement is enforced *implicitly* by S1 + S2. The Dart
VM asserts an isolate is only ever entered from the OS thread that owns it; OSCortex satisfies this by
pinning a whole engine's threads to one core and never running two engines' Dart concurrently. Violation =
the intermittent **EC=0x24** corruption (the remaining ~23% on aarch64; same class on x86).

**The scheduler forks on the E1 outcome — implement `affinity_ok` as a single swappable predicate:**

- **Variant (a) — confinement STAYS (default):**
  ```
  affinity_ok(p, cpu): p.home_cpu == cpu
                       && (fg_group dead || group_leader(p) == fg_group)   // foreground-exclusive
  ```
  Shell parked while an app is foreground (or enqueued but affinity-rejected). Exactly today's behavior,
  minus S3/S8/S10. `is_preemptable` still guards each thread's own isolate critical sections.
- **Variant (b) — confinement LIFTED (full concurrency):**
  ```
  affinity_ok(p, cpu): p.home_cpu == cpu     // home affinity ONLY, no foreground gate
  ```
  Shell (home 0) + app (home 1) run Dart concurrently on disjoint cores. The confinement assert holds
  **because the per-CPU queue guarantees a thread is never picked by a non-home core** → each isolate's
  owning OS thread is permanently on one core → "scheduled from Y ≠ owner X" never arises.
  **Prerequisite:** the embedder must give each isolate a stable owning thread (never hand it to a
  pooled/migratory worker). Probe by flipping the predicate and running multiapp `-smp 2` with two heavy
  apps; clean (no sys_abort/EC=0x24) ⇒ (b) reachable; regression ⇒ (a) is the ceiling until the embedder
  changes.

**Recommendation:** default (a); make (b) a one-line predicate swap to test. Everything else (per-CPU
queues, two-layer switch, `is_preemptable` bail) is identical in both variants.

---

## 5. RISKS + FEATURE-FLAG STRATEGY

**Feature flag (shipping path byte-unchanged):** a `smp` feature already exists (Cargo.toml:58, gates AP
bring-up) and the single-core default never wakes an AP (so every `smp`/`home_cpu` branch is already
inert). Land the rework behind a **new** `percpu-sched` feature (implies `smp`):
- **OFF (default):** compile the *current* PICK + ISR switch paths verbatim. **Binary-diff `oscortex.iso`
  before/after the feature is added-but-off to prove byte-identity.**
- **ON:** per-CPU queues + two-layer switch + `is_preemptable` bail; develop/test only under `-smp 2+`.

**Migration order (lowest-risk first):**
1. **Make `is_preemptable` live (no queue change).** Wire enforcement point 3; have the existing preempt
   bail also check `is_preemptable(cur)`. **The only step that can land in the shipping single-core path**
   (it strictly *adds* a bail → can only reduce preemption). Validates the field end-to-end. *Risk: low.*
2. Introduce `run_queue` + `enqueue` on wake/spawn, PICK reads it as a filtered view of PTABLE (belt-and-
   suspenders). Behind `percpu-sched`.
3. Replace PICK with `pick_next`; delete S2/S3/S10. Behind `percpu-sched`.
4. **Add two-layer `switch_to`/`switch_to_inner` + CONTEXT_SWITCH_LOCK**; route timer ISR + cooperative
   yield through `maybe_switch`. **Highest-risk** (the IRQ-frame ↔ kernel-context reconciliation §2.4).
5. Delete S8 once the per-CPU queue is the source of truth.
6. Delete S5/S9, then S6 — **only after** the syscall-layer wake correctness (Phase 1/2a) lands.

**Highest-risk steps:** step 4 (aarch64 102-word frame must stay the user-context source; do not let
`switch_to_inner` touch FP — vectors.rs:26-31; a botched "new thread releases CONTEXT_SWITCH_LOCK on
resume" handoff = a permanently-held switch lock, the exact PTABLE_LOCK-desync failure class from
[[smp-bringup]] M6 bug #1). Deleting S6/S9 prematurely hangs the bootstrap (they are load-bearing until
the syscall rework). E1 variant (b) before the embedder guarantees stable isolate ownership reintroduces
EC=0x24 — keep (a) default.

**Per-increment verification:** (1) single-core ISO byte-diff unchanged (feature off); (2) multiapp
`SMP=2` x86 + arm — no regression vs the current ~77%(arm)/~7-8(x86) baseline; (3) `debug_runnable_states`
(process/mod.rs:1515) on a wedge to confirm queue state.

---

## 6. TEST PLAN

`dev-tools/test/{x86,arm}-multiapp.py` (boot under `-smp N`, QMP-click-launch apps, inject pointer for
`DUR`s, sample `present_callback` + `embedder/ptr` → RESPONSIVE vs FROZEN; a wedged VM stops servicing
QMP so a blocked QMP call past the socket timeout is itself a FREEZE).

| Test | Command | Pass |
|---|---|---|
| Single-core no-regression (ship gate) | default build; `SMP=1 …x86` + arm | Identical behavior **+ byte-diff of `oscortex.iso` proving the default path is unchanged.** |
| x86 SMP converged | `ISO=oscortex.iso SMP=2 APPS=files,canvas DUR=120 …x86` | Each app renders on its home core; never fatal; `present` advances during injection; QMP responsive. |
| aarch64 SMP converged | `SMP=2 APPS=weblink DUR=45 …arm` (HVF) | App paints on core 1 while shell runs core 0; ≥ current 10/13; no EC=0x24. |
| E1 variant probe | variant (b): home-only `affinity_ok`; `SMP=2 APPS=files,canvas` | Clean ⇒ (b) reachable; sys_abort/EC=0x24 ⇒ stay (a). |
| `is_preemptable` bail (step 1, single-core) | default build; multiapp + sustained pointer | No new corruption; preemption count drops only inside marked critical sections. |

**"Converged" =** `present_callback` advances monotonically + smoothly across the whole window for every
launched engine (a plateau = a stalled engine); `embedder/ptr` tracks injected moves; zero sys_abort /
EC=0x24 / #GP; QMP never times out; on `-smp 2` two engines show simultaneous progress (variant b) OR the
focused engine progresses while the other is cleanly parked + OS stable (variant a); single-core
byte-identical + behavior-identical to pre-rework.

---

**Note:** S6 (KICK) and S9 (wake-assist) are **syscall-emulation debt, not scheduler bugs** — they exist
because parked threads miss epoll/timerfd/futex wakes. They must die with the **syscall** rework
(Phase 1 + Phase 2a), not this one. That makes the correct global order: **syscall correctness →
phys-futex → then this scheduler rework can shed its worst hacks.**

---

## Progress — Phase 3 STARTED + first step VERIFIED (2026-06-18)

- **`percpu-sched` cargo feature added** (Cargo.toml, default OFF, implies `smp`). Flag-off = the current
  cooperative+SMP scheduler with the new code `#[cfg]`-gated out, so the shipping path is byte-identical by
  construction.
- **Clean per-CPU PICK landed behind the flag** — `next_runnable_pid_percpu` (process/mod.rs). One
  affinity-filtered round-robin keeping ONLY what's load-bearing: foreground-exclusive (variant a),
  `home_cpu` affinity, and the vsync baton (retained per the design — the compositor/engine contract).
  DROPS the input-priority shortcut (S5) and the sticky-claim complexity. ~45 lines replacing the
  ~110-line special-case pile in `next_runnable_pid_locked`.
- **Verified on the real x86 ISO (flag-on):**
  - `-smp 2`: shell renders, Files launches + renders (crash=False), **RESULT RESPONSIVE** — per-CPU
    scheduling works across two cores (app on its home core, shell on the BSP), present advancing 110s.
  - `-smp 1`: shell renders, Files launches + renders (crash=False), then idle-plateaus to the harness
    FROZEN false-positive under `interact=False` — IDENTICAL to the current shipping PICK (which also
    plateaus when idle). Correct vsync-paced idle behaviour, not a regression.
  - The **vsync baton is load-bearing**: dropping it made `-smp 2` loop in `schedule_frame` without ever
    presenting (no `present_callback`). Re-added -> both core counts present. S4 stays, as designed.
- **E1 / variant-b (true concurrency) PROBED — the most promising lead.** Dropping foreground-exclusive
  under SMP (run shell + app concurrently, each on its home core) does **NOT** trip Dart's
  isolate-confinement abort — both engines rendered, **zero EC=0x24 / sys_abort**. The per-CPU `home_cpu`
  affinity satisfies the "isolate owned by thread X" rule, exactly as §4 variant (b) hoped. BUT present
  then flatlines: the vsync-baton/present path is **shell-only**, not concurrent-engine-aware, so it stalls
  once two engines run. **Variant b (true multi-app concurrency) is REACHABLE** once the baton/present path
  schedules per-engine — a tractable next step and the key unblocker for "many apps, more cores." Reverted
  to variant a (verified working) for now; the finding is in the `next_runnable_pid_percpu` comment.
- **NOT done — the bigger, riskier pieces remain** (each its own careful step):
  1. Explicit per-CPU run queues + `enqueue` at the wake/spawn/unblock sites (replaces the O(MAX_PROCS)
     scan).
  2. The two-layer `switch_to`/`switch_to_inner` + `CONTEXT_SWITCH_LOCK` (HIGHEST risk).
  3. Wire `is_preemptable` as the preempt bail (the dead primitive; needs a meaningful setter).
  4. Delete the cooperative crutches S6/S9 (force-wake/wake-assist) — coupled to Phase 2a + the syscall
     layer; they currently still fire (and still help) under the flag.
  This is the FIRST step (a clean, flag-gated, working per-CPU PICK), NOT Phase 3 complete.
- **`oscortex.iso` is currently the flag-ON build.** For shipping, rebuild WITHOUT
  `KERNEL_FEATURES=percpu-sched` (the default).
