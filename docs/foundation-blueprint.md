# Foundation Blueprint — fix the freeze class for good (preemption + syscall correctness)

> Produced 2026-06-20 by a 12-agent research+synthesis workflow (Redox/Linux/seL4/Zircon/Dart-engine
> research + live OSCortex mapping). This is the EXECUTABLE plan; supersedes the dated
> spec-phase3 where they conflict (favor the live-tree line numbers here, re-verify at edit).

## Unified diagnosis (one root cause, two faces, both arches)

The freeze class = a cooperative foundation where (a) the kernel has NO mechanism to involuntarily
reclaim a CPU from a non-yielding thread, and (b) the Linux-syscall emulation loses wakes. When no
thread can be forced off AND wakes are unreliable, any of {2nd-app launch, busy/install loop, Dart GC
safepoint} parks the system permanently — even the kernel-drawn cursor (compositor only repaints from
the BSP timer ISR, which never regains uncontended time).

Every freeze = `timer_preempt_switch_try` (process/mod.rs:2670) failing to switch:
1. **Preemption is DISCRETIONARY not MANDATORY.** x86 ISR switches only when `should_preempt` (idt.rs:692):
   quantum expired OR a *different* focused target has input/baton. A CPU-bound focus thread in pure Dart
   (GC mark, install loop) never enters a syscall, never hits the cooperative yield → only involuntary
   preempt can reclaim.
2. **The one mandatory path (slice expiry) is UNRELIABLE.** `slice_expired = account_tick_try(cur)` (idt.rs:687);
   `account_tick_try` (process/mod.rs:3212) does `PTABLE_LOCK.try_lock()` → None→false on contention → the
   quantum is never decremented and the mandatory time-slice preempt SILENTLY never fires.
3. **Even when it tries it self-cancels:** bail on `preempt_disabled()` (PREEMPT_DEPTH>0); bail on
   `!is_preemptable` (A0 — a permanent no-op, `set_preemptable` has ZERO callers); bail on PTABLE_LOCK
   contention; foreground-exclusive single-core → PICK returns the SAME pid → None → no switch.
4. **aarch64 is strictly worse:** `let preempt_enabled = false` (apic.rs:277) hard-disables EL0 preemption.

The cooperative side that should compensate is lossy: **FUTEX_WAITERS keyed by raw VIRTUAL address**
(state.rs:28) → shell (pid1) + app loading the same .so at identical VAs collide → a wake pops the wrong
waiter → parks forever (the post-app-launch render freeze). FUTEX_WAIT value-check (futex.rs:442) and
enqueue (~502) are in SEPARATE lock acquisitions → a wake in the gap is lost. FUTEX_WAIT_BITSET (op 9,
futex.rs:604 — glibc `pthread_cond_timedwait`) IGNORES bitset + abs timeout. FUTEX_CMP_REQUEUE (glibc
cond_broadcast) returns EINVAL → wake-all that, mis-keyed, wakes 0. The crutches (S6 KICK idt.rs:659,
S9 wake-assist idt.rs:765-788/apic.rs:200-254, cond_miss_bridge, glibc cond+16/+40 pokes that now stomp
__wrefs/__unused of the 48-byte struct = a heap-corruption suspect) exist only to survive this.

**Why arch-neutral:** the missing primitive is identical on both arches — a thread holding the core can't
be safely forced off, a parked thread can't be reliably woken. Linux proves the two preempt gates
(preempt_count HARD veto for held locks; need_resched LAZY soft, off-quantum-only) must be SEPARATE —
OSCortex collapses them into one veto, which re-introduces the freeze the moment is_preemptable goes
false. Dart confirms the only corrupting preempt window (EC=0x24) is a mutator in `kThreadInGenerated`
holding raw ObjectPtrs — NOT kernel-observable → REQUIRES an engine ABI signal; the forced valve is the
mandatory backstop when the black box mis-brackets. The serial-GC flags we ship are a workaround that
collapses the multi-thread safepoint to one thread; true per-CPU preemption is the real fix.

## The 7 steps (lowest-risk-first, shipping single-core byte-stable throughout)

**Step 1 — Forced-quantum safety valve** (process/mod.rs timer_preempt_switch_try@2670 + account_tick_try@3212;
idt.rs@721/687; apic.rs@313/264). (a) reliable slice accounting: per-CPU lock-free slice counter the ISR
decrements, reconcile into slice_left under PTABLE_LOCK (assert never <0). (b) `timer_preempt_switch_try(cur,
regs, forced: bool)`; bail = `if preempt_disabled(){None}` (HARD) then `if !forced && !is_preemptable(cur){None}`
(LAZY); forced = slice_expired. (c) both ISR sites pass slice_expired. (d) PREEMPT_BAILED_PREEMPTABLE counter.
RISK LOW (lazy gate inert while is_preemptable always-true; the spent-quantum-always-preempts half is
load-bearing). Pattern: Linux preempt_count vs TIF_NEED_RESCHED_LAZY.

**Step 2 — is_preemptable COUNTER + SYS_SET_PREEMPTABLE + engine bracket** (process/mod.rs@183/297/1190/1254;
dispatch.rs; engine-port FML EnterSafepoint/Transition seam). bool→`preempt_locks: AtomicU32`, is_preemptable
:=(==0), inc/dec pairs. Engine brackets isolate-owning sections (false on Dart_EnterIsolate/EnterSafepoint→
generated, true on Transition*ToNative/ToBlocked/Dart_ExitIsolate). RISK MED-HIGH, gated by the black box
(see Q1). Pattern: Redox PreemptGuard RAII; Dart Thread::EnterIsolate.

**Step 3 — Atomic futex WAIT** (futex.rs FUTEX_WAIT@437-565; process try_block Running→Blocked-only+bool;
wait.rs). lock FUTEX_WAITERS ONCE → read_volatile(uaddr) under lock → compare (EAGAIN if changed) → push →
try_block (false if a wake already landed → skip sleep) → drop lock → THEN yield/halt. Wake takes SAME lock,
unblock-guard Blocked→Running idempotent. Kills the lost-wakeup window. Do NOT hold the lock across the yield.
RISK MED. Pattern: Redox FUTEX_WAIT order; Linux futex_wait_setup.

**Step 4 — Address-space-identity futex KEY** (state.rs@28; futex.rs drop futex_phys_of@341/388, magic
addrs@27, pid1 bypass@268; wait.rs). raw u64 VA → `FutexKey::Private{pml4_phys, va}` (engine has NO MAP_SHARED
— grep-confirm; use the process pml4_phys, NO page-table walk), reserve `Shared{frame_phys}`. Compute key ONCE
at WAIT, store in the waiter + CondWaitState (ISR-timeout remove takes the pre-computed key, no translate under
IRQ). RISK MED-HIGH (Attempt-1 #DF'd here — do NOT translate per-op). Verify on -smp 2 TWO engines (files+weblink).
Pattern: Linux PRIVATE futex; corrected spec-phase2a (address-space identity, not phys alone).

**Step 5 — Correct cond_var** (futex.rs op9@604 + REQUEUE; posix.rs cond@1245-1808; poll.rs delete ISR
cond-expiry@600-708). (a) FUTEX_WAIT_BITSET: honor MATCH_ANY bitmask + ABSOLUTE timeout + FUTEX_CLOCK_REALTIME,
kernel ETIMEDOUT(110), then DELETE the COND_WAIT_STATE fake-timeout machinery. (b) FUTEX_CMP_REQUEUE (move
waiters cond→mutex, address-ordered). (c) DELETE the glibc cond+16/+40 pokes (stomp __wrefs/__unused). RISK MED.
Pattern: glibc cond_timedwait→WAIT_BITSET|CLOCK_REALTIME; cond_broadcast→CMP_REQUEUE. The one place OSCortex
needs MORE than Redox (prebuilt glibc).

**Step 6 — Real two-layer switch_to + CONTEXT_SWITCH_LOCK + per-CPU run queues** (process/mod.rs CONTEXT_SWITCH_LOCK@751,
switch_to/finish_hook, RunQueue, point-of-no-return@2733, percpu PICK@1693; arch/*/mod.rs switch_to_inner; idt.rs
replace frame-rewrite@726-747; apic.rs preempt_enabled=true@277). switch_to (Rust: FP save/restore gated on
ran-user, CR3/TTBR0 swap, base regs) + switch_to_inner (naked: ONLY callee-saved GPRs+sp, NEVER ret/eret, NEVER FP,
end with jmp to finish_hook). CONTEXT_SWITCH_LOCK = outermost; INCOMING thread releases it in finish_hook after the
point-of-no-return; outgoing MUST NOT. Per-CPU RunQueue, affinity at enqueue, running-flag cross-core exclusion.
Enable aarch64 EL0 preempt (now safe — real saved context). RISK HIGHEST (botched handoff = permanently-held lock =
M6 desync hang; switch_to_inner touching FP corrupts the 102-word frame). Pattern: Redox two-layer switch +
switch_finish_hook; seL4 point-of-no-return; Zircon split FP/GPR save.

**Step 7 — Delete crutches + unify clock** (idt.rs KICK@654/wake-assist@765; apic.rs@150/200; poll.rs force_wake@434/
timerfd-pending@446/0x47B@1172/monotonic_ns@711; mod.rs rdtsc_ns@178→TSC_HZ). ONLY after 3-6 verify (broadcasts wake>0,
epoll clean edges). Delete S6/S9/force_wake/cond-bridges/timerfd-pending-synth/0x47B-re-exec. Model epoll/timerfd/
eventfd on the same register-under-lock-then-block. Unify monotonic_ns + clock_gettime to ONE epoch+formula; rdtsc_ns
use calibrated TSC_HZ (do early, cheap). RISK MED but only if last (premature delete re-exposes every missed-wake as a
hang — Attempt-1 evidence). Pattern: Redox needs no KICK (correct wakes); Linux clocksource.

## Sequence rationale
Step 1 first (load-bearing backstop; the freeze returns if Step 2 sets is_preemptable=false WITHOUT it). 2 = engine
signal (closes EC=0x24, gated by the black box, sits behind the valve). 3→4→5 = futex/cond correctness (ordering →
key identity → cond semantics). 6 = highest-risk structural (depends on Step 3 to replace the in-syscall hand-off).
7 = strictly last (crutches load-bearing until correctness lands). Steps 1-5 are shared code (arch-neutral); Step 6's
asm is the one divergence (same FP/CR3-outside-asm discipline both arches); aarch64 EL0 preempt flips on only in Step 6.

## Open questions / black-box decisions
1. **(pivotal) Can the PREBUILT engine be re-patched** to emit SYS_SET_PREEMPTABLE around Dart_EnterIsolate/EnterSafepoint?
   engine-port has the OSCortex FML backend but NO Transition/safepoint shim. If not → is_preemptable stays best-effort,
   only Step-1 valve + per-CPU pinning protect (weaker — delays but can't prevent the EC=0x24 mid-isolate window).
2. Does the prebuilt glibc emit FUTEX_CMP_REQUEUE on cond_broadcast + use the 48-byte __pthread_cond_s? Trace live.
3. MAP_SHARED absence (Step 4 private-key correctness) — grep sys_mmap + validate at runtime before deleting futex_phys_of.
4. Ship Steps 1-5 with the frame-rewrite switch before Step 6? Yes (valve makes it safe); EC=0x24 fully closed only after 6.
5. cond_miss_bridge is a SCHEDULER crutch (cross-address waking), dies with Step 6 not Step 4 — confirm before deleting.
6. Verification: the collision + GC freeze ONLY manifest with TWO engines under -smp 2 — every gate from Step 3 on MUST
   run multiapp + teardown/relaunch on the real ISO with saved serial + screendump (single-engine looked correct yet #DF'd).
