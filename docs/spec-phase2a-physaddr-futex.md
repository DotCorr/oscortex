# Spec — Phase 2a: Physical-Address-Keyed Futex

> **Status: ATTEMPT 1 BUILT + REVERTED (2026-06-18) — phys-keying validated on the shell, but it
> double-faulted app-launch and is coupled to Phase 3. See "Attempt 1 findings" at the end.**
> Companion to [foundation-redesign.md](foundation-redesign.md)
> (Part B/Phase 2). Produced 2026-06-18 (overnight) by a read-only mapping pass over the real tree
> on `feat/native-engine-port`. **Line numbers are as-of-2026-06-18 — re-verify at implementation.**
> This is the code-level plan to greenlight + implement; it is not yet built or verified.

**Target:** OSCortex kernel futex subsystem.
**Goal:** Replace VA-only futex keying (which collides across processes loading the same `.so` at
identical VAs) with physical-address keying + address-space identity matching, and delete the pile
of special-cases that exist only to paper over the collision.
**Scope:** `kernel/src/syscall/handlers/futex.rs`, `kernel/src/syscall/wait.rs`,
`kernel/src/syscall/state.rs`, plus call-sites in `posix.rs` / `poll.rs`.

---

## 1. CURRENT STATE

### 1.1 Data structures (`kernel/src/syscall/state.rs`)

```rust
// state.rs:28
pub(crate) static FUTEX_WAITERS: Mutex<BTreeMap<u64, Vec<u32>>> = Mutex::new(BTreeMap::new());
// state.rs:29
pub(crate) static FUTEX_PENDING_WAKES: Mutex<BTreeMap<u64, u32>> = Mutex::new(BTreeMap::new());
// state.rs:31-37
pub(crate) enum CondWaitState {
    Waiting { cond: u64, mutex: u64, seq: u32, timeout_ns: u64 },
    AcquiringMutex { mutex: u64, timed_out: bool },
}
pub(crate) static COND_WAIT_STATE: Mutex<BTreeMap<u32, CondWaitState>> = Mutex::new(BTreeMap::new());
```

**The keying flaw:** `FUTEX_WAITERS` is `BTreeMap<u64 /* userspace VA */, Vec<u32 /* pid */>>`. The
shell (pid 1) and a launched app (pid 10+) are distinct address spaces that load the **same**
libc/Flutter `.so` at **identical** virtual addresses. Their internal pthread mutex/cond VAs collide
on the same `u64` key, so a wake from process A can pop and consume process B's waiter — the
post-app-launch render-freeze root cause (documented inline at `futex.rs:338-348`).

`FUTEX_PENDING_WAKES` and `COND_WAIT_STATE` are also VA-keyed in their value payloads.

### 1.2 The phys primitive (already half-wired) — `futex_phys_of`

```rust
// futex.rs:327-335
fn futex_phys_of(pid: u32, addr: u64) -> Option<u64> {
    let p4 = crate::process::pml4_phys_of(pid)?;
    let frame = crate::mm::paging::translate_user_page(p4, addr & !0xFFF)?;
    Some(frame | (addr & 0xFFF))
}
```

Composes `pml4_phys_of(pid)` (process/mod.rs:1694, under PTABLE_LOCK, None for Dead) and
`translate_user_page(p4, virt)` (mm/paging.rs:1058, guarded by `pml4_root_walkable`, takes the
page-table lock). **Already used opportunistically** inside `futex_wake_waiters` (futex.rs:356, 378)
as a *preferred* identity — but it never became the table key, so all the VA-collision band-aids
remain.

### 1.3 Itemized special-cases / hacks in `futex.rs`

| # | Symbol | file:line | Why it exists |
|---|--------|-----------|---------------|
| H1 | `FUTEX_ADDR_PID1_WAIT`/`WORKER_WAIT`/`HANDOFF` constants | 27-29 | Observed Flutter-engine cond/mutex VAs hard-coded to special-case the 3 addresses that deadlock bootstrap. |
| H2 | `futex_addr_is_target` | 31-37 | Predicate "is this one of the 3 magic VAs?" |
| H3 | `futex_target_waiter_count` | 39-43 | Count waiters on a magic VA → feeds tracing + bridge. |
| H4 | `futex_trace_targets` | 45-66 | Debug telemetry for the 3 magic VAs. |
| H5 | `cond_miss_bridge` | 68-195 | THE big workaround: broadcast woke 0 under the colliding key → "bridge" the wake to other VAs with same-group waiters. Pokes glibc `__wakeup_seq`/`__broadcast_seq` (164-172). |
| H6 | `engine_broadcast_storm_wake` | 197-255 | NotifyAll keeps waking 0 (wrong-key) → force-wake siblings on other cond VAs, storm-throttled. |
| H7 | `futex_pid1_postrun_bypass` | 257-284 | First 256 `FUTEX_WAIT` by pid 1 on `PID1_WAIT` return 0 without parking (bootstrap deadlock break). |
| H8 | `futex_wake_all_known_waiters` | 286-303 | Brute-force "wake one on every recorded VA" last-resort. |
| H9 | `futex_pending_post`/`_take` + `FUTEX_PENDING_WAKES` | 305-325 | Wake-before-wait race buffer for magic VAs (because H5/H6 fire async wakes). |
| H10 | group-leader fallback in `futex_wake_waiters` | 349-394 | Prefer `futex_phys_of` equality, fall back to `get_group_leader(w)==caller_grp`. Caller==0 wakes all. |
| H11 | target pending-post on missing waiter | 363-367 | Producer half of H9. |
| H12 | target-wake tracing | 400-415 | Telemetry. |
| H13 | `futex_pid1_postrun_bypass` re-call in WAIT_BITSET | 608-610 | Same bypass for op 9. |
| H14 | pending-take + task-runner pulse in WAIT | 450-470 | Consumer of H9 + a 2nd-order deadlock-breaker. |
| H15 | group-leader scoping in `futex_waiter_for` | wait.rs:62-74 | Return only a same-group waiter (used by `pthread_mutex_unlock` handoff, posix.rs:1059). |

**External call-sites** that lean on these (migration must update): `posix.rs:1059` (`futex_waiter_for`),
`posix.rs:1087,1156,1504,1570,1589` (`futex_wake_waiters`), `posix.rs:1507,1655` (`cond_miss_bridge`),
`posix.rs:1679` (`engine_broadcast_storm_wake`), `poll.rs:649` (`futex_waiter_remove_try`, ISR timeout),
`process/mod.rs:1132` (`futex_waiter_remove_all`, teardown).

---

## 2. TARGET DESIGN

### 2.1 The new key

```rust
// state.rs — NEW
pub(crate) type FutexKey = u64; // full physical address: (frame & !0xFFF) | (va & 0xFFF)

#[derive(Clone, Copy)]
pub(crate) struct FutexWaiter { pub pid: u32, pub target_va: u64 }

pub(crate) static FUTEX_WAITERS: Mutex<BTreeMap<FutexKey, Vec<FutexWaiter>>> =
    Mutex::new(BTreeMap::new());
```

`FUTEX_PENDING_WAKES` + the whole pending mechanism (H9/H11/H14) are **deleted** (§3). `COND_WAIT_STATE`
**stays** (it is per-pid → no cross-process collision) but decoupled from futex routing (§4.2).

### 2.2 Key derivation (composes the existing primitives)

```
sys_futex(WAIT) : key = futex_phys_of(current_pid, uaddr)   // park under phys key, store target_va
sys_futex(WAKE) : key = futex_phys_of(current_pid, uaddr)   // wake everyone under the SAME phys key
```

Because the key encodes the physical page, **all waiters in a bucket share the same physical word by
construction** — no per-waiter re-translation, no group-leader comparison. **Derive the key BEFORE
taking `FUTEX_WAITERS`** — the current code nests the page-table lock *under* the waiter lock
(futex.rs:358→378), a hazard the new order (`PTABLE/page-table lock → FUTEX_WAITERS`) eliminates.

### 2.3 New `wait.rs` helpers (phys-keyed)

```rust
fn futex_waiter_add(key: FutexKey, pid: u32, target_va: u64);     // dedup on (pid, target_va)
fn futex_waiter_remove(key: FutexKey, pid: u32, target_va: u64);  // retain !(pid && va); drop empty bucket
fn futex_waiter_present(key: FutexKey, pid: u32) -> bool;
fn futex_waiter_remove_all(pid: u32) -> u32;                      // teardown: retain |w| w.pid != pid
fn futex_waiter_remove_try(key: FutexKey, pid: u32) -> bool;      // ISR-safe; PRE-COMPUTED key only
fn futex_waiter_for(key: FutexKey, exclude: u32) -> Option<u32>;  // same bucket ⇒ same phys; no group filter
```

### 2.4 New `futex_wake_waiters`

```rust
pub(crate) fn futex_wake_waiters(uaddr: u64, count: u32) -> i64 {
    let caller = current_pid();
    let key = if caller == 0 { None } else { futex_phys_of(caller, uaddr) }; // derive BEFORE lock
    let wake_list: Vec<u32> = {
        let mut t = FUTEX_WAITERS.lock();
        match key {
            Some(k) => { let Some(v) = t.get_mut(&k) else { return 0; };
                         let n = (count as usize).min(v.len());
                         let woke = v.drain(..n).map(|w| w.pid).collect();
                         if v.is_empty() { t.remove(&k); } woke }
            None => { /* caller==0 ISR: VA-match fallback, drain by target_va==uaddr across buckets */ }
        }
    };
    for pid in &wake_list { set_state(*pid, Running); }
    wake_list.len() as i64
}
```

No `caller_grp`, no `get_group_leader`, no per-waiter `futex_phys_of` loop — the bucket *is* the
address-space-identity proof. WAIT/WAKE in `sys_futex` lose H7/H13 (bypass), H14 (pending+pulse), H4/H12
(tracing); op 9 (`FUTEX_WAIT_BITSET`) gets the identical phys-key treatment.

---

## 3. SPECIAL-CASES DELETED

| Hack | Delete? | Causal chain |
|------|---------|--------------|
| H1 magic constants | **DELETE** | "Special" only because shell/app identical VAs collided; phys key routes them as ordinary futexes. |
| H2 `futex_addr_is_target` | **DELETE** | Pure predicate over H1; no consumers left. |
| H3 `futex_target_waiter_count` | **DELETE** | Only fed H4 + H5. |
| H4 `futex_trace_targets` | **DELETE** | Telemetry for a vanished phenomenon. |
| H5 `cond_miss_bridge` | **DELETE** | Exists because a broadcast woke 0 under the colliding key; phys-keying routes to the exact waiters → woke>0 → nothing to bridge. Drop at posix.rs:1507,1655. |
| H6 `engine_broadcast_storm_wake` | **DELETE** | Same wrong-key symptom; drop at posix.rs:1679. |
| H7/H13 `futex_pid1_postrun_bypass` | **DELETE (verify)** | Forced pid 1 past a park that deadlocked because its wake went to the app's colliding waiter. Correct routing wakes it. RISK: also masked any genuine missing-wake bug — if bootstrap still stalls, a real bug is exposed (intended). Keep behind a temp debug cfg during bring-up, delete once §5 passes. |
| H8 `futex_wake_all_known_waiters` | **DELETE** | Needed only when the table couldn't identify the waiter. |
| H9/H11/H14 pending mechanism | **DELETE** | The wake-before-wait race was created by H5/H6 async wakes; no async bridge ⇒ standard ordered WAIT/WAKE. |
| H10 group-leader fallback | **DELETE (becomes the rule)** | Bucket already guarantees phys-identity. `caller==0` ISR force-wake stays (§4.4). |
| H12 target-wake tracing | **DELETE** | Telemetry. |
| H15 group-leader scoping (`futex_waiter_for`) | **DELETE (replace param)** | Re-keyed to phys; bucket is intrinsically same-address-space. **posix.rs:1059 must pass `futex_phys_of(pid, mutex)` instead of the raw VA.** |

**Two honest caveats — these are NOT collision workarounds and MUST stay:**
1. The `caller==0` ISR force-wake branch (kernel bring-up wakes user waiters with no address space to
   translate through) — retained as the `key == None` arm.
2. The cooperative scheduler hand-off (`next_runnable_pid` save/enter in WAIT/WAKE) — orthogonal to
   keying; stays unchanged (it is the single-core no-preempt-during-bring-up mechanism).

---

## 4. RISKS + EDGE CASES

- **4.1 Translation failure (unmapped/lazy/COW):** the futex word was just `read_volatile`'d two lines
  before park, so the page is present in practice. If translation still fails (TOCTOU vs concurrent
  unmap), return 0 (spurious wake — correct futex semantics). **Never park under a `None` key.**
  aarch64 maps anon eagerly (`eager_map_anon_page`, process/mod.rs:1738) and `sys_mmap` backs frames at
  map time → low risk. Future-proof the WAIT spin loop to re-derive the key after each `enable_and_halt`
  and migrate the waiter if the frame changed (never fires today — no COW, no MAP_SHARED).
- **4.2 `COND_WAIT_STATE` coupling:** its consumers H5/H6 are deleted. It stays for the **timeout state
  machine** only (poll.rs:628-656 ISR + posix.rs:1281-1438 cond loop). It is per-pid → no collision.
  **Only change:** store the resolved `FutexKey` alongside `cond` at park time
  (`CondWaitState::Waiting { cond_key, .. }`, computed in process context) so the ISR
  (`poll.rs:649`) calls `futex_waiter_remove_try(cond_key, pid)` **without translating** (it must not —
  it runs IRQ-masked single-core; translating would self-deadlock on PTABLE/page-table lock).
- **4.3 Timeout path:** lives entirely in `COND_WAIT_STATE` + the cond loop, not in the keys → re-keying
  doesn't touch deadlines. Single edit = §4.2's ISR removal-by-key.
- **4.4 ISR-safe try-lock:** `futex_waiter_remove_try` stays `try_lock`-based; must receive a
  pre-computed key, never derive one.
- **4.5 Real shared memory (same phys, diff VA):** **none exists** — `sys_mmap` (engine.rs:150-173)
  supports only anonymous private mappings; no MAP_SHARED/shm/memfd (grep-verified). So no
  same-phys/diff-VA case today, and phys-keying is *forward-correct* if MAP_SHARED is ever added.
- **4.6 Teardown/relaunch:** `futex_waiter_remove_all(pid)` (process/mod.rs:1132) is unaffected
  (retain by `.pid`); under phys-keying a relaunched instance gets fresh frames → fresh keys anyway.
- **4.7 Lock-ordering:** establish `PTABLE/page-table lock → FUTEX_WAITERS` consistently; audit no path
  translates while holding the waiter lock.

---

## 5. TEST PLAN

Verify on the **real ISO** (Rule 5) via `dev-tools/test/{x86,arm}-multiapp.py`. Build:
`KERNEL_FEATURES=smp X86_AOT=1 SKIP_CORE_APPS=1 bash scripts/build-iso.sh`.

- **Gate 1 — single-core no-regression (ship default):** `SMP=1 APPS=files` x86 + arm → RESPONSIVE,
  shell + Files render, no crash. The cooperative + serial-GC path must converge **without** the
  deleted bypasses/bridges. If it now stalls where a bypass forced progress, that is a *real* ordering
  bug newly exposed — fix it, don't re-add the bypass.
- **Gate 2 — SMP multi-app:** `SMP=2 APPS=files` (+ weblink, canvas) x86; aarch64 `SMP=2` — expect
  parity with the current ~77% clean-render, not worse.
- **Gate 3 — concurrent collision stress (the bug's home turf):** `SMP=2 APPS=files,weblink` (two engine
  instances, same `.so`, same VAs = the exact cross-process collision). Expect both responsive,
  `present` advancing, zero EC=0x24.
- **Gate 4 — teardown/relaunch:** x86 harness `INTERACT_N>0` (click N rounds then stop → force the GPF →
  crash-recovery → relaunch). Assert OS stays non-fatal + relaunched instance renders.
- **Pass:** every gate `RESPONSIVE`, exit 0; grep the built kernel logs to confirm the
  `cond-bridge`/`engine-bcast-storm`/`futex-postrun-bypass`/`futex-pending` tags no longer appear.
  Save serial logs + screendump PNG per gate before declaring DONE.

---

**Key finding:** `futex_phys_of` is already written and already used as a *preferred* identity inside
`futex_wake_waiters`, but bolted on as a fallback overlay rather than the key — so every VA-collision
band-aid (H1–H15) survived. Promoting it to the actual `BTreeMap` key collapses ~600 of ~650 futex lines
into a standard physaddr-keyed futex; only the `caller==0` ISR force-wake and the cooperative scheduler
hand-off are non-collision logic that must stay.

---

## Attempt 1 findings (2026-06-18) — built, partially verified, REVERTED

The full re-key was implemented and built clean: `wait::futex_key` (phys translation), `FUTEX_WAITERS`
re-keyed by physical address, and ~400 lines of hacks (H1–H15) deleted. Result on the real x86 ISO:

- **Phys-keying is VALIDATED for the shell.** The shell (1st engine) rendered — `present_callback`
  advanced — with every hack gone. The concept is sound.
- **App-launch REGRESSED — kernel double fault.** Launching a 2nd engine (Files) produced a
  **nondeterministic `#DF`** whose origin ip landed in `page_fault_full_handler`'s *prologue* (a
  fault-while-entering-the-fault-handler = stack-overflow / fault-in-fault), `crash=True`. Two runs
  diverged (68 presents then wedge vs 21 presents then #DF), confirming a race. App-launch worked before
  the change → regression. **Reverted to the working tree.**

**Two root causes — both must be fixed before retrying:**

1. **`translate_user_page` on EVERY futex op.** `futex_key` ran inside `futex_waiter_add/remove/present/
   for` and `futex_wake_waiters` — a page-table walk per op, including once per iteration of the WAIT
   sleep loop. During app-launch that races with the *app's own page-table construction* and/or deepens
   the fault path enough to overflow. **Fix:** translate the key ONCE at WAIT entry (after `read_volatile`
   proves the page present), cache it, pass the cached key to present/remove — never re-translate in the
   loop or per-op. Better: a fast path that skips translation entirely while only one address space holds
   the futex word (the common case), paying the walk only when ≥2 engines actually share a VA.

2. **`cond_miss_bridge` was load-bearing for the cooperative WORKER bootstrap, not just the collision.**
   Its Phase-B did *cross-address* waking (pid 1 broadcasts cond A while engine workers sit on unrelated
   mutex-internal futexes B/C). Phys-keying does NOT replace that — those are genuinely different words.
   It's a **cooperative-scheduling crutch**: the workers' real waker hasn't been scheduled yet. The
   principled replacement is **Phase 3 (real preemption)** — workers get scheduled and signal in order —
   OR an explicit, phys-safe "kick my group's futex waiters during bring-up" shim (clearly labeled, deleted
   with Phase 3). Do NOT re-add the VA-deref seq-poke (it dereferenced a phys key as a VA → memory-unsafe).

**Conclusion: Phase 2a is NOT a safe standalone change — it is coupled to Phase 3.** The phys-keying is
correct (the shell proves it); land it *together with* the preemptive scheduler (or a labeled phys-safe
bootstrap kick), WITH the translate-once-per-WAIT fix, and verify app-launch + concurrent multi-app on the
real ISO before declaring done. The reverted tree keeps the working VA-keyed futex + Phase-1a `close()`
cleanup.
