# ARM/x86 branch integration plan (2026-06-12 recon)

Goal: "one source tree, both arches" true on ONE branch. Currently split:
- origin/main: v0.0.6 aarch64 real-HW render (5 fixes: SPSel a27a61d-area/7c1d751, daifset bd96e7e, crash#2 454bd63, SP-align 7e1a019, CNTV timer e9fd481) + release pipeline (a2d6750/3191527/5d0e750). 64 ahead of feat.
- feat/native-engine-port: pkg pipeline (b4dcdb5/463e689/30c3684/ff933f0), Ed25519 signing (0cf5aa8), caps (c769520), crash-recovery (62e2cd3), hardening (e870f28). 99 ahead of main.

Merge-base 320e92d. A real merge = 37 conflicted files, MODERATE severity.

STRATEGY (recommended): merge origin/main INTO feat/native-engine-port in a worktree.
Resolution heuristics:
- aarch64/{timer,enter_user,boot,vectors,mmu,apic,cpu,syscall,mod}.rs → TAKE MAIN (--theirs): it has the 5 render fixes; feat's is pre-fix. timer.rs MUST become CNTV/PPI27.
- boot_limine.rs → auto (add-only on main).
- .github/workflows/release.yml → take main (arch-labeled artifacts + aarch64 UEFI ISO).
- kernel/src/pkg/*, embedder/abi.rs (pkg syscalls 0x4C0-0x4C3), syscall/handlers/* → TAKE FEAT (--ours): main lacks the pipeline.
- MANUAL 3-way union (the real work): process/mod.rs (626 lines — crash-recovery/caps/kill_group/note_thread_exit + caps field/helpers vs ARM teardown/SP/arch_frame), syscall/dispatch.rs (pkg syscalls + cap gate vs ARM), syscall/mod.rs, main.rs, mm/mod.rs, cortex/pid0.rs (CAP_CORTEX check).

VERIFICATION GATE (all must pass before landing): (1) x86 + aarch64 kernel+embedder build, (2) x86 ISO renders shell smp=1, (3) ARM renders under `-M virt,accel=hvf -cpu host` (SP-align+CNTV are TCG-invisible — HVF mandatory), (4) pkg pipeline cold-boot 5.4MB fetch→SHA256→cache→install, (5) Ed25519+caps+crash-recovery smoke.

FALLBACK if process/mod.rs union is intractable: cherry-pick the 9 ARM/release commits onto feat (lower blast radius, aarch64/*+release.yml only): a27a61d 7c1d751 bd96e7e 454bd63 7e1a019 e9fd481 5d0e750 3191527 a2d6750.

Do it in a worktree; only fast-forward + push once the full gate passes. feat's ARM = real port but pre-render-fixes (runs in TCG, won't render on HVF).
