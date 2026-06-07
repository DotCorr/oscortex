# Native Flutter Engine Port to OSCortex (AOT)

Status: IN PROGRESS — branch `feat/native-engine-port`
Owner: Tahiru Agbanwa
Started: 2026-06-07

## Goal (one sentence)

Stop running a Linux-built Flutter engine through a syscall-emulation shim +
binary patches, and instead build the Flutter engine **from source as a
first-class OSCortex target, in AOT mode** — so the engine speaks OSCortex
natively, needs no runtime codegen, and the entire hack pile can be deleted.

## Why (the decision)

The current model is "OSCortex pretends to be Linux":
- the kernel emulates the Linux syscall ABI,
- `tools/flutter-engine/engine_patch.py` does binary surgery on a Linux `.so`,
- the JIT engine compiles the whole framework to memory every boot (W^X, ~GBs,
  60–90s warm-up),
- the deserializer is hacked to limp an AOT snapshot the JIT engine can't read.

This is a house of cards. It is also fundamentally incompatible with the project
goal: a lite, fast, scalable, embedded-capable OS. JIT-on-emulated-Linux cannot
get there.

**Precedent that this is the sanctioned path, not an exotic one:** the Flutter
engine already ships non-Linux platform backends. The entire QNX (a commercial
RTOS) port of `fml` is **one 473-byte file** (`fml/platform/qnx/paths_qnx.cc`);
everything else reuses `fml/platform/posix/*`. Fuchsia (a microkernel OS) has a
full backend too. Flutter is *designed* to be ported to new operating systems.
OSCortex adds `fml/platform/oscortex` and a Dart-VM OS backend, exactly as they did.

**Two birds, one stone:** because we control the build, we build the engine in
**AOT (release) mode** with an OSCortex `gen_snapshot`. Native targeting and AOT
are the *same* effort, not two separate ones.

## The port surface (what we own vs. what's free)

| Layer | Effort | Notes |
|---|---|---|
| `fml/platform/oscortex` | small | reuse `posix/*`; override only what differs (paths, maybe threads) |
| Dart VM OS layer (`runtime/vm/os_*.cc`, `os_thread_*.cc`) | **the real work** | threads, VM heap mmap, virtual memory, time, TLS, condvars |
| GN build target (`--target-os=oscortex`, toolchain, sysroot) | one-time infra | the gnarly setup |
| Skia software raster, `dart:ui`, `flow`, `display_list` | ~none | OS-agnostic; keep software raster (no GPU) |
| AOT build + matching `gen_snapshot` | we control it | this is where AOT falls out |
| Embedder (`tools/flutter-embedder`) | minor | already a custom C-API embedder; adjust for AOT data path |

The OS-specific code surface is **bounded** (QNX proves it). The *build
infrastructure* is the mountain.

## Port surface map (measured 2026-06-07, from the real checkout)

The exact code we own. Everything else in the engine (~millions of LOC: Skia,
dart:ui, flow, display_list, the embedder) is OS-agnostic and untouched.

**Dart VM OS layer** — mirror the linux impl against OSCortex native syscalls:
- `runtime/vm/os_oscortex.cc`        ← clone `os_linux.cc` (782 lines): time,
  monotonic clocks, memory/RSS, ProcessId, NumberOfAvailableProcessors, print,
  DSO base, GC notify. ~30 `OS::` methods (see `vm/os.h`).
- `runtime/vm/os_thread_oscortex.cc` ← clone `os_thread_linux.cc` (235 lines):
  thread create/join, mutex, monitor/condvar, TLS.
  → ~1,000 lines total. Most map 1:1 to syscalls OSCortex already has.

**fml platform layer** — like QNX (which overrode just 1 file), reuse posix:
- `fml/platform/oscortex/message_loop_oscortex.cc` ← clone `message_loop_linux.cc`
  (epoll + timerfd + eventfd wakeup). **THE critical file** — this is the engine's
  UI/raster thread pump, and its hand-emulated equivalent is exactly what has been
  livelocking rendering today. Porting it natively is the chance to make the
  wakeup mechanism provably correct → the native port also *fixes the sync bugs*,
  not just enables AOT.
- `fml/platform/oscortex/paths_oscortex.cc` ← trivial (QNX = 1 file).
- Reuse `fml/platform/posix/*` as-is (file, mapping, native_library, process,
  command_line) — contingent on OSCortex POSIX being clean.

**Build-system glue:**
- `flutter/tools/gn`: add `oscortex` to `--target-os` choices (next to `qnx`).
- `build/config/` + toolchain: add an `oscortex` config (clone `linux`).
- `fml/BUILD.gn`: add an `if (is_oscortex)` sources block (mirrors the `is_qnx`/
  `is_linux` switch at lines ~189-200).
- Dart `runtime/vm/BUILD.gn`: select `os_oscortex.cc` / `os_thread_oscortex.cc`.
- Software-only: drop Impeller (GPU) from the oscortex target — OSCortex rasters
  in software, and Impeller is the slowest ~300 objects of every build.

**Total core port: ~1,200 lines + GN config.** Bounded, exactly as QNX/Fuchsia
predicted. The AOT snapshot/gen_snapshot work is separate (Phase 3).

**Where the port code lives:** in the OSCortex repo (tracked, no remnants), under
`engine-port/`, applied into the external engine checkout by a script before
building. The 30 GB engine checkout itself stays out of the repo.

## Phased plan

Each phase ends in a committed, verifiable state. We do not advance until the
current phase's checkpoint passes.

### Phase 0 — Build infrastructure ✅ DONE (2026-06-07)
- [x] Install `depot_tools` (in container `oscx-engine`).
- [x] Full engine checkout via `gclient` at Flutter 3.41.1 / `582a0e7c55`
      (`name:"."` per `engine/scripts/standard.gclient`; first try used `name:"flutter"`
      → wrong layout, fixed). 22 GB.
- [x] Linux/amd64 build environment (Docker on arm64 macOS, emulated).
- [x] **Checkpoint HIT:** baseline `out/host_debug_unopt_x64/libflutter_engine.so`
      built from source — 377 MB x64, exports the C embedder API
      (`FlutterEngineRun` et al.), debug/JIT as expected. Toolchain proven.

### Phase 1 — Map + define the OSCortex platform target  ✅ DONE (2026-06-07)
- [x] Enumerate the host-primitive surface — see "Port surface map" above
      (~1,200 lines: Dart VM os_* + fml message_loop/paths, reuse posix).
- [x] Add `oscortex` as a `--target-os`. Refined approach (vs. a full new
      toolchain): OSCortex has no sysroot/libc yet (it runs linux-ABI binaries via
      emulation), so `--target-os=oscortex` links against **linux** but sets a new
      `is_oscortex` GN flag that will select the OSCortex backend sources. A true
      OSCortex sysroot/toolchain is a later sub-phase once OSCortex grows a libc.
      Edits (tracked in `engine-port/patches/`, applied by `apply-port.sh`):
      `flutter/tools/gn` (choices + x64 cpu + linux-link override + software-only,
      no GPU) and `build/config/BUILDCONFIG.gn` (`is_oscortex` declare_arg).
- [x] **Checkpoint HIT:** `flutter/tools/gn --target-os=oscortex …` configures
      cleanly ("Made 1714 targets"), `out/oscortex_debug_unopt/args.gn` shows
      `target_os = "linux"` + `is_oscortex = true`.

### Phase 2 — Implement the OSCortex platform backend
- [ ] `fml/platform/oscortex/` (reuse posix; override the minimum).
- [ ] Dart VM `os_oscortex.cc` / `os_thread_oscortex.cc` against OSCortex
      native syscalls (NOT the Linux-emulation numbers).
- **Checkpoint:** a debug/JIT engine built *for OSCortex* boots on OSCortex with
  NO `engine_patch.py` and NO Linux-syscall emulation in the hot path.

### Phase 3 — AOT
- [ ] Build the engine in release/AOT mode for the OSCortex target.
- [ ] Build an OSCortex `gen_snapshot`; compile the shell app to an AOT snapshot.
- [ ] Wire the embedder's AOT-data path (scaffolding partly exists); resolve the
      known last blocker from the prior AOT attempt: the VM-isolate base-object
      handoff (`app_snapshot.cc: expects N base objects, provided 0`).
- **Checkpoint:** the shell renders from an AOT snapshot — no JIT, no W^X codegen,
  fast cold start, fraction of the RAM.

### Phase 4 — Delete the hacks (no remnants)
Once the native AOT path renders, remove — not disable — the scaffolding:
- [ ] `tools/flutter-engine/engine_patch.py` and its build-step invocations.
- [ ] Linux-syscall-emulation paths in the kernel that only existed for the
      foreign engine (audit each; keep only what the native ABI needs).
- [ ] Deserializer/JIT workarounds, the JIT codegen W^X path.
- [ ] The 1ms/adaptive-pump warm-up crutch if AOT removes the need.
- [ ] Embedded blobs baked into the kernel → load from disk (Limine modules /
      VFS), shrinking the kernel back toward ~60 MB.
- **Checkpoint:** `git grep` for the old hack symbols returns nothing; the build
  has no dead AOT-on-JIT code.

## Build infrastructure facts (verified 2026-06-07)

- Dev host: macOS arm64. Disk free: ~76 GB (tight — engine checkout is large).
- Docker daemon: running (linux/amd64 builds go here).
- Flutter SDK: 3.41.1 stable, engine `cc8e596aa65130a0678cc59613ed1c5125184db4`.
- Local engine source is only a 66 MB sliver (no Dart VM) — full checkout needed.
- Official prebuilt: ONLY `linux-x64-embedder.zip` (debug/JIT) exports the C
  embedder API; release/profile linux-x64 are GTK-wrapped (no embedder API) —
  confirming a from-source build is required for native + AOT.

## Honest risks / unknowns

- Disk: a full `gclient` checkout + build outputs may exceed 76 GB free. May need
  a reduced/shallow sync or external scratch space.
- Cross-build: building linux/amd64 from arm64 macOS via Docker is slow (hours).
- Dart VM OS port depth: the GC heap's virtual-memory expectations and thread
  model are the substantive unknowns; OSCortex's cooperative single-core
  scheduler may need work to satisfy them cleanly (ties into the SMP item).
- This is months-scale. Progress is measured by phase checkpoints, not by a
  single "it works."

## Cleanup principle

Per the codebase-audit-cleanup rule: when a hack is superseded by the native
path, it is **removed**, not left dormant. No remnants, no dead toggles.
