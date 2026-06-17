# OSCortex — agent instructions

OSCortex is a from-scratch OS: a Rust microkernel (x86_64 + aarch64, no glibc/std,
software-composited framebuffer, no GPU) running a custom prebuilt Flutter embedder
as the shell + apps. Single source tree; active branch `feat/native-engine-port`,
base `develop`.

## ‼️ READ `rules.md` BEFORE STARTING AND BEFORE REPORTING DONE — MANDATORY
[rules.md](rules.md) is binding for every agent. The short version:
- **Never present a stub/placeholder/mock as done.** Label it in code, report it as a
  stub, and never call it "done/working/complete".
- **"Done" = verified end-to-end on the REAL artifact** (the actual ISO/binary the
  user runs), with saved proof. Compiles/renders/tests-pass ≠ done.
- **Report status as exactly one of: DONE / UNVERIFIED / SCAFFOLD-STUB / NOT STARTED.**
- **Finish the task or name the gap precisely** (what's missing, why, effort). No
  silent scope-narrowing.
- Keep [docs/FEATURE_STATUS.md](docs/FEATURE_STATUS.md) honest and current.

Violating these wastes the maintainer's time and breaks trust. Slower-but-true beats
fast-but-fake, always.

## Build / test (x86, the primary target)
- Release build with the SMP app-launch fix: `KERNEL_FEATURES=smp X86_AOT=1 SKIP_CORE_APPS=1 bash scripts/build-iso.sh` → `oscortex.iso`. Add `input-hud` to KERNEL_FEATURES only for local debugging (never in a release).
- The SMP app-launch fix is inert on a single core — test under `-smp 2`+.
- Verify on the real ISO before claiming anything works (Rule 5).

## Hard constraints
- **No Claude/Anthropic attribution** on commits or anything cloud-bound.
- Only verified work goes to cloud. Published == locally verified.
- Never enter the maintainer's credentials/tokens — that is the maintainer's action.
