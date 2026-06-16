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
| **x86 app launch + render (SMP)** | **DONE (this session, headless)** | Verified `-smp 2`: Files renders, 0 panics. **UNVERIFIED on bare metal.** ~70–85% of runs render; intermittent **non-fatal** first-frame stall remains |
| x86 app launch (single core) | KNOWN-BROKEN, recoverable | Original dart:io crash; OS now survives it (resilience) but app doesn't render. SMP is the fix |
| **aarch64 app launch** | **KNOWN-BROKEN (freezes); SMP advanced 2026-06-17** | Single-core (shipped run-arm.sh): app launch freezes (same cooperative-scheduler root cause as x86). SMP work this session under HVF (real cores): the AP now runs the app (was idle); fixed a real bug — the AP never called enable_fpu_simd so CNTKCTL_EL1 was unset → the app's CNTVCT_EL0 read trapped (EC=0x18). PRECISE remaining wall: the engine bootstrap needs a pump to finish on the AP; the cooperative hand-off alone stalls, and the only pump (ISR-initiated enter_user_by_pid_noreturn_try) corrupts EL0 thread state → EC=0x24 data abort. Genuine fix = corruption-free per-CPU context switch (Redox-style switch_to) — multi-session. x86 SMP app-launch works (~70–85%); aarch64 does not yet |
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
