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

## Phased plan

Each phase ends in a committed, verifiable state. We do not advance until the
current phase's checkpoint passes.

### Phase 0 — Build infrastructure (prerequisite, no OSCortex code yet)
- [ ] Install `depot_tools`.
- [ ] Full engine checkout via `gclient` at the SDK's engine hash
      (`cc8e596aa65130a0678cc59613ed1c5125184db4`, Flutter 3.41.1) — keep history
      shallow to save disk (~tight on 76 GB free).
- [ ] Linux/amd64 build environment (Docker, since dev host is arm64 macOS).
- **Checkpoint:** a *baseline* `linux-x64` debug embedder builds from source and
  produces a `libflutter_engine.so` matching the prebuilt we already use. This
  proves the toolchain before we change anything.

### Phase 1 — Map + define the OSCortex platform target
- [ ] Enumerate every host primitive the Dart VM OS layer + fml require; map each
      to an existing OSCortex syscall or mark "to implement".
- [ ] Add `oscortex` as a GN target OS (toolchain, defines, sysroot) — start by
      cloning the `linux` target and renaming, building against OSCortex headers.
- **Checkpoint:** the engine *configures* for `--target-os=oscortex` and fails
  only on genuinely-missing OSCortex primitives (a known, listed set).

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
