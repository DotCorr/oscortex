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

## Two flows: most people FETCH, maintainers BUILD

The engine is a **pinned, frozen artifact** (Flutter 3.41.1 / engine `cc8e596`,
plus our port). You build it **once per (version × arch × mode)** and **publish**
it; everyone else downloads it. This is exactly how Flutter distributes its own
engine — app developers never build it.

### Consumer flow (devs + CI) — the default, takes seconds
```bash
engine-port/fetch-engine.sh x64 release   # download the pinned prebuilt engine
# ...then the normal OS build (build-kernel-iso-fast.sh etc.) links against it.
```
No `gclient` checkout, no hour-long build. Pin + host URL live in
`engine-port/artifact.config`.

### Maintainer flow — only when you CHANGE the engine port
You only do this if you edit `engine-port/` (the ~1,200 lines of platform
backend) or bump the Flutter pin. Then you rebuild **and re-publish**, bumping
`ARTIFACT_VERSION` so consumers pull fresh:
```bash
engine-port/setup-engine-build.sh          # once: Docker container + 22 GB checkout
engine-port/build-engine.sh baseline       # (optional) prove the toolchain
engine-port/apply-port.sh                   # apply patches/ + src/ into the checkout
engine-port/build-engine.sh oscortex        # build the OSCortex engine
engine-port/publish-engine.sh x64 release   # package + upload to R2
```

## Artifact distribution

| Role | Runs | When |
|---|---|---|
| **Consumer** (every dev, CI) | `fetch-engine.sh` → downloads from R2 | every checkout (seconds) |
| **Maintainer** (engine port owner) | `setup` + `build` + `publish-engine.sh` | rare — port/version change |

- **Host:** Cloudflare R2 (S3-compatible, zero egress, CDN). Layout:
  `$ARTIFACT_BASE_URL/$ARTIFACT_VERSION/oscortex-<arch>-<mode>.tar.gz` (+ `.sha256`).
  Each tarball: `libflutter_engine.so`, `gen_snapshot`, `icudtl.dat`, `MANIFEST.txt`.
- **Pin/version:** `engine-port/artifact.config`. Bump `ARTIFACT_VERSION` on any
  port/Flutter change; set `ARTIFACT_BASE_URL` (or `OSCORTEX_ENGINE_BASE_URL`) to
  your R2 public URL.
- **Multi-arch:** same model, one tarball per ISA (`oscortex-arm64-release`, …) —
  the engine builds per-arch with a flag, so publishing is just more rows.

### One-time R2 host setup (maintainer)
Creating the bucket needs *your* Cloudflare account, so authenticate first, then
run the helper (it creates the bucket, enables the public `r2.dev` URL, and wires
`ARTIFACT_BASE_URL` into `artifact.config`):
```bash
npx wrangler login            # one-time browser OAuth (or set CLOUDFLARE_API_TOKEN)
engine-port/setup-r2.sh       # create bucket + public URL + update artifact.config
```
After that, `publish-engine.sh` uploads via `wrangler` (no install — uses `npx`),
and `fetch-engine.sh` just works for everyone.

The engine checkout is pinned to **Flutter 3.41.1** (`582a0e7c55`, engine
`cc8e596`). Do not bump it without re-deriving the port + re-publishing.

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
