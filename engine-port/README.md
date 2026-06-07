# engine-port/ — OSCortex native Flutter engine port

This directory holds everything needed to build the Flutter engine **natively for
OSCortex** (AOT), and the OSCortex-specific source that gets applied into a Flutter
engine checkout. It is the reproducible, contributor-facing home of the work
described in [`../docs/native-engine-port.md`](../docs/native-engine-port.md).

**Why it's separate from the engine checkout:** the Flutter engine `gclient`
checkout is ~22 GB and lives **outside** this repo (never committed). Only our
port code + scripts live here, version-controlled, and are *applied into* that
external checkout. No 22 GB blob in git, no remnants.

## Layout

```
engine-port/
  README.md                 — this file
  setup-engine-build.sh     — one-shot: container + depot_tools + gclient checkout
  build-engine.sh           — configure (gn) + build (ninja): baseline | oscortex
  apply-port.sh             — copy our platform backend + patches INTO the checkout
  patches/                  — diffs against the stock engine (gn target-os, BUILD.gn)
  src/                      — OSCortex platform backend sources (added 1:1 to engine)
    runtime/vm/os_oscortex.cc          (← clones os_linux.cc)
    runtime/vm/os_thread_oscortex.cc   (← clones os_thread_linux.cc)
    fml/platform/oscortex/message_loop_oscortex.cc
    fml/platform/oscortex/paths_oscortex.cc
```
(`patches/` and `src/` are populated during Phase 1–2.)

## Contributor flow (do this once, then iterate)

```bash
# 1. Set up the build environment (Docker container + 22 GB engine checkout).
#    Idempotent — safe to re-run. Takes a while the first time.
engine-port/setup-engine-build.sh

# 2. Prove the toolchain with a stock host build (Phase 0 checkpoint).
engine-port/build-engine.sh baseline
#    -> out/host_debug_unopt_x64/libflutter_engine.so  (377 MB, x64, embedder API)

# 3. Apply the OSCortex port into the checkout, then build the native target.
engine-port/apply-port.sh
engine-port/build-engine.sh oscortex
```

The engine checkout is pinned to **Flutter 3.41.1** (`582a0e7c55`, engine
`cc8e596`) — the exact version this repo's SDK uses. Do not bump it without
re-deriving the port.

## Edit→build loop (during the port)

After the first full build, ninja is **incremental** — editing a port source and
re-running `build-engine.sh oscortex` recompiles only the changed files + relinks
(minutes, not the ~1 h first build). You only pay the full build on a clean
checkout or a global config change.

## Pitfalls already solved (so you don't rediscover them)

- **`.gclient` must use `name: "."`** (per `engine/scripts/standard.gclient`).
  `name: "flutter"` mis-nests the checkout and the hooks fail on wrong paths.
- **Apple Silicon:** the `linux/amd64` container runs under emulation — correct
  but slow (the first full build is ~1 h). Incremental builds are fine.
- **Disk:** checkout ~22 GB + build output ~5–10 GB. Keep ~40 GB free.
- **`--prebuilt-dart-sdk`** saves a large chunk of build time; keep it unless you
  are editing the Dart SDK itself.
