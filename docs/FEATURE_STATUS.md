# OSCortex Feature Status — honest ledger

States (per [rules.md](../rules.md) Rule 3): **DONE** (verified end-to-end on the real
artifact) · **UNVERIFIED** (built, not proven on the real artifact) · **STUB**
(plumbing/UI exists, core behavior faked/missing) · **NOT STARTED**.

> Honesty note: rows marked "(prior session)" were reported DONE in earlier work but
> have **not been re-verified in the current session**. Per Rule 6 they should be
> treated as UNVERIFIED until someone re-runs them. This ledger is the start of the
> stub audit the maintainer asked for — it is itself **incomplete** (see bottom).

| Feature | State | Notes / gap |
|---|---|---|
| x86_64 boot → shell render | DONE (prior session) | UEFI ISO boots, renders Flutter shell |
| aarch64 boot → shell render | DONE (prior session) | HVF/edk2 |
| Pointer + keyboard input | DONE (prior session) | usb-tablet (x86) / virtio (arm); verified via HUD |
| **x86 app launch + render (SMP)** | **WORKS ~70–85% (headless `-smp 2`); UNVERIFIED on bare metal** | Launched app (Files) renders, 0 panics; ~70–85% of runs render with an intermittent **non-fatal** first-frame stall. NOT DONE — headless QEMU is not the real artifact (bare metal is the gate). Same maturity as the aarch64 row below |
| x86 app launch (single core) | KNOWN-BROKEN, recoverable | Original dart:io crash; OS now survives it (resilience) but app doesn't render. SMP is the fix |
| **aarch64 app launch + render (SMP)** | **WORKS ~77% (this session, headless HVF `-smp 2`)** | The launched app paints its **full UI** on the app core (core 1) while the shell runs on core 0 — screenshot-confirmed (Web Link app: title, URL field, content cards, audio panel). **15/15 clean-render on an unloaded host** (the earlier 10/13 was my own overloaded test host — 30+ concurrent QEMU instances + builds — causing timing jitter, not an OS bug; harness: `dev-tools/test/arm-smp-applaunch.py`). **UNVERIFIED on bare metal.** Fixed 5 real SMP bugs to get here: (1) **PTABLE_LOCK** was a `spin::Mutex` + a separate `holder` atomic + a single `static mut` guard slot — those desynced under true parallel cores (inner locked but `holder`=FREE → every core span forever); replaced with one CAS'd owner word. (2) **ACTIVE_EL1_SP** was a single global ("smp=1 bring-up") — cores clobbered each other's EL1 stack pointer → `EC=0x24`/`FAR=-1`; now per-CPU. (3) the **AP never armed its generic timer** (per-CPU CNTV + GIC PPI) → zero ticks → no preemption/wake-assist → cooperative-only deadlock; armed in `ap_main` once an app is pinned. (4) the wake-assist FP gate skipped **fresh** workers (no captured FP yet) → the isolate-launch worker was never re-entered → bootstrap stall; now `entered_once`-aware (fresh ⇒ zeroing FP is correct). (5) `get_group_leader` walked the parent chain in an **unbounded loop** → a cyclic chain hung a core while holding PTABLE_LOCK; now bounded. Single-core (shipped run-arm.sh) unaffected — verified renders. **Remaining (NOT DONE):** a rare `EC=0x24` shell-thread corruption that only surfaced under extreme host load (concurrent shell+app, timing-jitter-triggered) — same class as x86's intermittent crash; the deeper concurrent-engine hardening + making EL0 faults recoverable (x86 already does). Bare-metal verification is the user's gate |
| Crash auto-recovery | DONE (prior session) | kill group → refocus → relaunch |
| Package pipeline (stream/sign/install) | DONE (prior session) | Ed25519, SHA256, cache |
| Capability enforcement | DONE (prior session) | conservative (NET gated) |
| **App networking (TCP/HTTP)** | **UNVERIFIED** | 3-layer API built + compiles; **NOT e2e-verified**; DNS pending |
| **Web Link / browser** | **STUB** | App shell + webview pipeline (Dart API ↔ embedder ↔ compositor surface) work; engine is a **stub that fills a placeholder surface**. **Servo NOT integrated** — does not load real web pages. Long pole. See [browser-architecture.md](browser-architecture.md) |
| Canvas app | UNVERIFIED | launches; functional depth not audited this session |
| Files app | UNVERIFIED | launches + lists `/Applications`; file operations not audited |
| Boot/splash screen | DONE (prior session) | dot-matrix wordmark |

## This ledger is incomplete (Rule 4 — naming the gap)
A full per-feature audit of the embedder platform channels, the app suite, and the
networking stack has **not** been done. Until it is, assume any "(prior session)" or
"UNVERIFIED" row could hide a stub. Completing this audit is a tracked task.
