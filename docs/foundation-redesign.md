# OSCortex Foundation Redesign — Scheduler + Syscall Layer

**Status:** DESIGN — for review. No core code changed yet. 2026-06-18.

**Goal:** Replace the two *ad-hoc, accreted* kernel layers — the **scheduler** and the
**Linux/POSIX syscall-emulation** — with principled foundations (the Redox blueprint +
correct/complete Linux semantics), so we eliminate the *class* of bugs (freezes,
recovery-deadlocks, orphan/futex collisions, the engine GPF) instead of patching symptoms.

**Why now:** chasing the x86 app-launch freeze produced a stack of mutually-propping
patches — foreground-exclusive → home-core pinning → sticky-claim → KICK/force-wake →
crash-defocus → orphan-reap → futex-purge. Each patch props up the previous one. That's
debt. Both layers are piles of special-cases; the bugs are *symptoms* of the accretion.
Fix the layers, not the symptoms.

---

## Part A — What we have today (the two ad-hoc layers)

### A1. Scheduler — cooperative core + bolted-on SMP, ~10 special-cases

Selection is a flat `PTABLE[256]` scan (`process/mod.rs:next_runnable_pid_locked`) gated by
a chain of special-cases, each added to dodge a corruption the *previous* design caused:

| Special-case | file:line | Why it exists (the comment) |
|---|---|---|
| Foreground-exclusive | process/mod.rs ~1544 | "Two concurrent heavy Flutter VMs overwhelm the cooperative context save/restore → corrupted callee-saved reg → #GP" |
| home_cpu pinning | process/mod.rs ~1626 | "an engine stays on ONE core … per-CPU scratch never split" |
| Sticky `current_cpu` claim | process/mod.rs ~2054 | "releasing on every hand-off → both cores chase the same threads → livelock at ~1 fps" |
| Vsync `baton` shortcut | idt.rs ~693, apic.rs ~217 | "pumps a launched app's frames on its core" |
| De-starve / input-priority | poll.rs ~1028, idt.rs ~687 | engine threads busy-loop epoll_wait and starve input |
| `KICK_REQUESTED` / force-wake | idt.rs ~650, apic.rs ~150 | "break cooperative deadlocks" (deferred kick every ~500 ms) |
| Bootstrap-spin gate | poll.rs ~1265, wm ~359 | suppress churn during engine bring-up |
| AP idle-park loop | sched/mod.rs ~92, idt.rs ~636 | AP polling PTABLE_LOCK every tick stalls BSP first-frame under TCG SMP |
| 0x47B re-execute | poll.rs (several) | blocking syscall can't return until ready → re-enter on wake |
| Cooperative-yield full-GPR save | process/mod.rs ~2577 | IRETQ-vs-SYSRETQ x86 detail |

Two Redox primitives are **declared but dead**: `is_preemptable` (process/mod.rs:183 — stored,
never checked) and `CONTEXT_SWITCH_LOCK` (process/mod.rs:754 — `#[allow(dead_code)]`).
Preemption is gated only by `PREEMPT_DEPTH` (kernel-lock depth), and on aarch64 EL0 preemption
is OFF entirely (`preempt_enabled = false`).

### A2. Syscall layer — partial POSIX emulation, correctness gaps + ~15 hacks

The prebuilt Flutter/Dart engine is a **Linux binary**; the kernel hand-emulates the syscalls
it uses. The emulation is incomplete, and the gaps are papered over with force-wakes and
seq-bumping. Ranked by bug-causation (from the audit):

1. **`close()` cleans NOTHING** (`handlers/fd.rs:sys_close`). It does *not* remove the fd from
   `EPOLL_TABLE` / `TIMERFD_TABLE` / `EVENTFD_TABLE`, and does *not* wake threads blocked in
   `epoll_wait` on it. → **stale entries that keep being reported as ready, with the
   now-freed `data` cookie** (the DescriptorInfo the engine freed on close). This is the
   strongest kernel-side candidate for the EventHandler dangling-pointer #GP. Real Linux
   cleans the fd out of every epoll set and wakes waiters on close.
2. **epoll_event ABI** — packed 12/offset-4 (x86) vs 16/offset-8 (aarch64), handled per-arch
   but fragile; a wrong write = engine reads a garbage `data.ptr`.
3. **timerfd `pending` is faked** — `force_wake_all_task_runners` pokes `pending` to break a
   cooperative deadlock, capped at 1 to avoid a "BOGUS expiration count" that corrupted dart:io
   timeout processing. A correct timerfd never has its count synthesized.
4. **futex is VA-keyed** (`wait.rs` `FUTEX_WAITERS: BTreeMap<u64-VA, Vec<pid>>`) → cross-process
   collision when shell + app load the same `.so` at the same VA; mitigated by group-leader
   scoping, but VA-keying is fundamentally wrong.
5. **cond_var is a hand-rolled state machine** with hardcoded glibc struct offsets
   (`cond+16`, `cond+40`) and "engine-storm"/"cond-bridge" wake hacks — instead of a real
   `futex_wait`/`futex_wake`.
6. **clock mismatch** — `monotonic_ns()` (saturating add) vs `clock_gettime` (div/mod) drift
   1–2 ns; timerfd deadlines and epoll readiness read from different formulas.
7. **threads** — `thread_create` force-runs the child immediately (bootstrap hack);
   `thread_join` spins to 2e9 then "treats as joined" (aarch64 livelock cap).

The `force-wake`, `KICK_REQUESTED`, `cond-bridge`, `engine-storm`, `pid1-bypass`,
`bootstrap-spin` hacks all exist to compensate for **(a)** the cooperative scheduler (no real
preemption to break a spin) and **(b)** the incomplete syscalls (stale entries, faked fires).
Fix (a) and (b) and these evaporate.

---

## Part B — The target

### B1. Scheduler (Redox blueprint — see memory [[redox-blueprint]])

- **Per-CPU run queues**, affinity-filtered selection (`current`/`idle` per CPU). `next_runnable`
  becomes "scan *my* CPU's queue", not a 256-slot global scan with a home_cpu filter.
- **One global `CONTEXT_SWITCH_LOCK` + non-preemptable bail:** never switch *away* from a
  thread with `is_preemptable == false` (set while holding a userspace lock / mid-VM-critical
  section). This is the principled version of "don't corrupt a preempted thread."
- **Two-layer context switch:** `switch_to()` (FP/SIMD save + guarded restore + address-space
  swap) → naked `switch_to_inner()` (callee-saved only). Exact register sets per arch from Redox.
- **Physical-address-keyed futex** (shared with B2.4).
- **Simple round-robin** for now (Redox ran priority-less RR for years; DWRR/EEVDF is later tuning,
  not needed for the shell).

### B2. Syscall layer (correct + complete Linux semantics)

1. **`close(fd)`** removes the fd from every epoll set + the timerfd/eventfd tables, wakes any
   thread blocked on it. (Likely the freeze fix.)
2. **epoll** — arch-native event layout, `DEL` wakes blocked waiters, `timeout=0` returns
   immediately vs blocks correctly.
3. **timerfd** — `pending` only ever incremented by a *real* expiry (`1 + (now-deadline)/period`);
   no force-wake fakery; cleaned on close.
4. **futex** — keyed by physical address (translate the user VA), wake only same-physaddr
   waiters. Deletes the VA-collision + group-leader scoping.
5. **cond_var** — a real `futex_wait`/`futex_wake` against the condvar word; delete the
   state machine, the glibc-offset writes, and the storm/bridge hacks.
6. **clock** — one monotonic source feeding both `monotonic_ns` and `clock_gettime`.
7. **threads** — `create` returns and lets the scheduler run the child (no forced immediate
   slice); `join` blocks on a real wait/wake (no spin cap).

---

## Part C — Special-cases DELETED (the payoff)

| Special-case | Fate under the foundation | Phase |
|---|---|---|
| `KICK_REQUESTED` + `force_wake_all_task_runners` | **ELIMINATED** — real preemption breaks spins; no cooperative deadlock to kick | 4 |
| home_cpu pinning + sticky `current_cpu` claim | **ELIMINATED** — becomes the per-CPU run-queue + its load-balancing policy | 3–4 |
| cond-bridge / engine-storm / glibc-offset hacks | **ELIMINATED** — real futex-based cond_var | 1 |
| de-starve / vsync-baton shortcuts | **SIMPLIFIED** — standard "wake the thread on its CPU's queue" | 3–4 |
| AP idle-park polling loop / bootstrap-spin gate | **SIMPLIFIED** — a per-CPU-queue readiness flag, not a per-tick conditional | 3–4 |
| timerfd `pending` faking / BOGUS-count cap | **ELIMINATED** — correct expiry accounting | 1 |
| Foreground-exclusive | **MAYBE KEPT** — see Risk E1 (Dart isolate-confinement) | — |
| 0x47B re-execute, IRETQ/SYSRETQ save | **KEPT** (arch detail) — isolated to the arch layer | 3 |

Net effect: the `next_runnable` selection collapses from ~110 lines of special-cases to a
per-CPU queue pop; ~15 syscall hacks delete; a large amount of diagnostic logging deletes.

---

## Part D — Phased migration (each phase verified on the real ISO before the next)

**Phase 1 — Syscall correctness (LOW risk, additive, highest payoff). START HERE.**
- 1a. `close()` cleanup of epoll/timerfd/eventfd + wake-blocked-waiters. *Test: does the x86
  EventHandler GPF stop?* (Strong hypothesis it does.)
- 1b. timerfd correct `pending`; remove the BOGUS-count cap once force-wake is gone.
- 1c. cond_var → real futex_wait/wake; delete storm/bridge/glibc-offset hacks.
- 1d. unify the clock.
- Risk: LOW — these make behavior *more* Linux-correct; the engine expects Linux. Keep the
  current cooperative scheduler unchanged during Phase 1.

**Phase 2 — Redox primitives (MEDIUM risk).**
- 2a. Physical-address-keyed futex (replace VA + group-scoping).
- 2b. Enforce `is_preemptable` in the timer-preempt path (it's already stored).
- 2c. Wire `CONTEXT_SWITCH_LOCK` into the switch path.
- Risk: MEDIUM — touches the preempt/lock hot path; single-core must stay green.

**Phase 3 — Scheduler core (HIGH risk — the kernel's heart).**
- 3a. Per-CPU run queues + affinity-filtered selection; replace `next_runnable_pid_locked`.
- 3b. Two-layer `switch_to`/`switch_to_inner` per arch.
- 3c. Turn on real preemptive time-slicing (now safe, given 2b/2c).
- Risk: HIGH — do behind a feature flag; keep the single-core path bit-stable; verify multi-app
  on `-smp 1` and `-smp 2` at every step.

**Phase 4 — Delete the dead special-cases + their diagnostics.**
- Remove home_cpu/sticky-claim/KICK/force-wake/baton-shortcut/bootstrap-spin/AP-park and the
  `[force-wake]`/`[destarve]`/`[cond-bridge]`/`[epoll-*]` logging. Net LOC *down*.

---

## Part E — Risks & open questions (need decisions)

- **E1. Dart isolate-confinement.** The comment at process/mod.rs:1576 says running the shell
  and app concurrently trips Dart's "Isolate main is owned by os thread X, scheduled from Y"
  abort. If that's a hard Dart-VM constraint, **foreground-exclusive must stay** even with a
  perfect scheduler (it's then a *correct* policy, not a workaround). Open: can serial-GC /
  the engine allowlist / an engine-port patch lift it? → I should verify before Phase 3.
- **E2. The engine GPF.** Phase 1a (`close()` cleanup) is the leading *kernel-side* fix. If it
  doesn't resolve it, the residual is engine-internal (Dart VM timeout_queue_ corruption) and
  needs engine source — a separate track, isolable once the foundation is clean.
- **E3. Transition instability.** The kernel heart changes in Phase 3. Mitigation: feature-flag
  the new scheduler, keep the old path compiled until the new one is green on both arches.
- **E4. Prebuilt-engine expectations.** Validate each correctness change against actual engine
  behavior via the headless harnesses (`dev-tools/test/{arm,x86}-multiapp.py`).

---

## Part F — Cleanup folded in (the standing rule)

- Each phase deletes its special-cases + diagnostic logging as they become dead (Phase 4 is
  almost entirely deletion).
- Out-of-source bloat (audit 2026-06-17, on the maintainer's OK): `.claude/worktrees/` (4.1 GB
  stale agent caches), `*.trace` dumps (910 MB), `*.bak_syncswitch` engine backups (91 MB),
  `apps/_x86_aot_backup/` (verify first). All non-source.
- Two blanket `#![allow(dead_code)]` files (`kernel/src/main.rs`, `drivers/virtio_input.rs`)
  hide real dead code — replace with item-level allows to surface it.

---

## Decisions needed before I touch code

1. **Phase order** — syscall-correctness first (Phase 1), scheduler core last (Phase 3). OK?
2. **E1** — investigate lifting the Dart isolate-confinement constraint, or accept
   foreground-exclusive persists as a correct policy?
3. **Green-light Phase 1** (low-risk syscall correctness, starting with `close()` cleanup —
   our best shot at the freeze without touching the scheduler heart)?

---

## Progress log — overnight 2026-06-17 → 18 (autonomous)

- **Phase 1a — `close()` cleanup: DONE, built, x86-verified, KEPT.** `poll::on_fd_closed(fd)`
  now purges a closed fd from `EPOLL_TABLE` (as epfd and as watched fd), `TIMERFD_TABLE`,
  `EVENTFD_TABLE`, and wakes blocked epoll waiters. Previously `sys_close` did *nothing* for
  synthetic fds (≥64, not in `OPEN_FILES`) — a real correctness gap, now fixed. x86 still
  boots / renders / launches: **no regression.**
- **It does NOT fix the freeze.** The EventHandler `#GP` (ip=0x140d84cdb) still fires on in-app
  interaction (GPF=1 → recover → relaunch → stall = FROZEN). This **re-confirms (4th independent
  way)** the corruption is **engine-internal** (Dart VM `timeout_queue_`), not the kernel epoll
  path. The clean freeze fix needs **engine source**, not kernel work — it is a separate track
  from this foundation.
- **Honest scope note:** Phase 1b (drop `force_wake` fakery) couples to Phase 3 (real preemption);
  Phase 1c (cond_var → real `futex_wait`) couples to Phase 2a (physaddr futex). The
  *independently* safe Phase-1 items are `close()`-cleanup (done), clock unify, epoll-DEL wakes.
  Phase 2 (physaddr futex / `is_preemptable` / context-switch-lock) and Phase 3 (per-CPU
  preemptive scheduler core) are MEDIUM/HIGH risk on the engine's critical sync + the kernel's
  heart — **I am not landing those unattended.** They need you in the loop (and Phase 3 behind a
  feature flag, per Part D).
- **Decision E1 (Dart isolate-confinement) still open** — gates whether foreground-exclusive
  survives Phase 3.

### Implementation specs written (read-only deep-dive, overnight) — ready to greenlight

Two code-level migration specs now exist (design only, no code changed):
- **[spec-phase2a-physaddr-futex.md](spec-phase2a-physaddr-futex.md)** — promote the *already-existing*
  `futex_phys_of` (futex.rs:331) from a fallback overlay to the actual `BTreeMap` key. Collapses ~600 of
  ~650 futex lines and **deletes 13 of 15 special-cases** (the magic engine addresses, `cond_miss_bridge`,
  `engine_broadcast_storm_wake`, the pid1 bypass, `futex_wake_all_known_waiters`, the pending mechanism,
  the group-leader scoping). Only the `caller==0` ISR force-wake + the cooperative hand-off stay.
  **No `MAP_SHARED` exists in the engine → no same-phys/diff-VA edge case → clean + forward-correct.**
- **[spec-phase3-percpu-scheduler.md](spec-phase3-percpu-scheduler.md)** — per-CPU affinity-filtered run
  queues + the two dead Redox primitives (`is_preemptable` @183, `CONTEXT_SWITCH_LOCK` @754, both confirmed
  zero-caller) wired live + two-layer `switch_to`. Deletes S2/S3/S8/S10 outright. Behind a **new
  `percpu-sched` cargo feature (default OFF)** so the shipping ISO is byte-identical (verify by binary diff).

**Two findings that reshape the plan:**
1. **The aarch64 102-word trap frame is *complementary* to a two-layer `switch_to`, not in conflict** — it
   is the *user* context (restored on `eret`); the new `KernelContext` is the *kernel* context (restored on
   `ret` between in-kernel threads). The migration *adds* the kernel half; it does not replace the existing
   frame machinery. Do **not** duplicate the FP save (vectors.rs:26-31 is the single FP source).
2. **The scheduler's worst hacks are syscall debt, not scheduler bugs.** S6 (KICK/force-wake) and S9
   (wake-assist ISR re-entry) exist only because parked threads miss epoll/timerfd/futex wakes. They are
   load-bearing until the *syscall* layer is correct. **So the global order is: syscall correctness
   (Phase 1) → phys-futex (Phase 2a) → THEN the scheduler rework can shed S6/S9.** Doing the scheduler
   first would force re-adding them.

**Revised phase order (confirmed by the specs):** Phase 1 (syscall correctness, done: close-cleanup) →
Phase 2a (phys-futex — deletes the futex special-cases AND unblocks the scheduler's S6/S9 deletion) →
Phase 3 (per-CPU scheduler, feature-flagged; **step 1 — wiring `is_preemptable` live — is low-risk and can
land even in the single-core shipping path** since it only *adds* a preempt bail). E1 is a one-line
swappable `affinity_ok` predicate, so it does not block starting Phase 3; it only decides variant (a)
foreground-exclusive vs (b) full concurrency at the end.

### Phase 2a — Attempt 1 built + reverted (2026-06-18)

Implemented the phys-keyed futex for real (built clean, ~400 lines of hacks deleted). **The shell rendered
without any hacks — phys-keying validated.** But launching a 2nd engine **double-faulted the kernel**
(nondeterministic fault-in-`page_fault_full_handler` = stack-overflow/race), a regression, so it was
**reverted**. Two root causes, both in [spec-phase2a-physaddr-futex.md](spec-phase2a-physaddr-futex.md)
("Attempt 1 findings"): (1) `translate_user_page` was called on *every* futex op (must translate once per
WAIT and cache); (2) the deleted `cond_miss_bridge` was a **cooperative-scheduling crutch** for the worker
bootstrap, not just a collision fix — so **Phase 2a is coupled to Phase 3** and must land with it (or with
a labeled phys-safe bootstrap kick). The reverted tree keeps the working VA-keyed futex + Phase-1a
`close()` cleanup.

### Phase 3 — started + first step verified (2026-06-18)

Added the `percpu-sched` cargo feature (default OFF → shipping path byte-identical) and a clean per-CPU
PICK behind it (`next_runnable_pid_percpu`, ~45 lines replacing the ~110-line special-case pile; keeps
foreground-exclusive + `home_cpu` affinity + the vsync baton, drops the input-priority shortcut and the
sticky-claim complexity). **Verified flag-on on the real x86 ISO: `-smp 2` RESPONSIVE (shell + app render
across two cores), `-smp 1` renders + launches.** The vsync baton proved load-bearing (without it `-smp 2`
never presents). This is the FIRST step only — explicit run queues, the two-layer `switch_to`,
`is_preemptable`, and deleting the S6/S9 crutches remain. See
[spec-phase3-percpu-scheduler.md](spec-phase3-percpu-scheduler.md) "Progress".
