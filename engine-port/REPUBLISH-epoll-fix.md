# Republish the engine with the x86 interact-freeze crash-free fix

The x86 in-app interact `#GP` (`ip=0x140d84cdb`, dart:io `EventHandler::Poll`) is
the engine reading the `epoll_wait` syscall number (`0x47B`=1147) as an event
count and overrunning its 16-slot `events[]` array. The **reliable crash-free
defense** is an engine-side validation that rejects bogus `epoll_wait` counts —
patch [`patches/0005-epoll-size-validation.patch`](patches/0005-epoll-size-validation.patch).

The kernel side is *partially* fixed in the main tree (`perform_cooperative_yield`
now uses `save_return_context_reexec`), but a rare residual race remains; this
engine patch makes the build crash-free regardless. **The engine `.so` is fetched
(gitignored), so this fix only takes effect once the engine artifact is
re-published.**

Verified: with this patch in the engine, `GPF (0x140d84cdb) = 0` across runs
(`ship1/ship2`); without it, the unmodified engine `#GP`s in ~2/3 interact runs.

## Steps

> The **publish** step (4) uploads to the GitHub release / R2 and needs **your**
> credentials — I can't run it for you.

Prereqs: Docker running + the `oscx-engine` build container (`setup-engine-build.sh`).

1. **Apply the patches** (now includes 0005, wired into `apply-port.sh`):
   ```
   bash engine-port/apply-port.sh
   ```

2. **Rebuild the engine** (x64; add your arm64 build too if you ship aarch64):
   ```
   bash engine-port/build-engine.sh oscortex
   ```

3. **Bump the artifact version** so a fresh fetch pulls the new build —
   `engine-port/artifact.config`:
   ```
   ARTIFACT_VERSION="oscortex-engine-3"   # was oscortex-engine-2
   ```

4. **Publish** (GitHub release / R2 — *your credentials*):
   ```
   bash engine-port/publish-engine.sh
   ```

5. **Re-fetch + rebuild the ISO**:
   ```
   bash scripts/fetch-engine.sh
   KERNEL_FEATURES=smp X86_AOT=1 SKIP_CORE_APPS=1 bash scripts/build-iso.sh
   ```

6. **Verify crash-free**:
   ```
   SMP=1 APPS=files INTERACT=1 DUR=130 python3 dev-tools/test/x86-multiapp.py
   ```
   Expect `GPF (0x140d84cdb) = 0`. `[OSX-GUARD] bogus epoll_wait … dropping batch`
   appears ~50× per run — each is a *caught* kernel leak (harmless; the kernel
   race is the source, not the engine).

## Notes

- **Immediate local test without re-publishing:** the guarded `.so` built this
  session is at
  `~/Desktop/Dotcorr/oscortex-engine-build/engine/engine/src/out/oscortex_release_x64/libflutter_engine.so`
  — copy it over `tools/flutter-engine/libflutter_engine.so` and rebuild the ISO.
- **Quiet the log for shipping:** the `[OSX-GUARD] bogus epoll_wait` line fires
  once per caught leak (~50/run). Rate-limit the `Syslog::PrintErr` in
  `HandleEvents` (a static counter, first N) if it's noisy — the silent drop is
  the actual fix.
- **True elimination:** root-cause the kernel rare race so the size-validation
  never fires (the engine patch then becomes pure defense-in-depth). That race
  is intermittent and lives in the boot-fragile syscall-resume path — see
  `memory/scheduler-rework.md` for the dead-ends and the lock-free
  instrumentation approach that worked.
