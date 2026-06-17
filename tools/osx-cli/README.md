# osx — the OSCortex app toolchain

Build and preview **Flutter apps for [OSCortex](https://github.com/DotCorr/oscortex)**,
a Rust microkernel with a Flutter shell. OSCortex apps ship as **`.osx`
bundles**. Flutter's own toolchain doesn't target OSCortex, so this package adds
the missing `init` / `build` / `run` steps and hides the internals.

A single `osx build` produces a **universal bundle that is native for both
`aarch64` and `x86_64`** — app developers never pick an architecture, and never
touch the engine, `gen_snapshot`, or the bundle format by hand.

```
osx init     set up the OSCortex build target for a Flutter app
osx build    compile the app into a universal .osx bundle (both arches)
osx run      boot OSCortex in an emulator to preview (host arch fast, other emulated)
```

---

## Install

```bash
dart pub global activate oscortex_cli
```

This puts an `osx` executable on your path (ensure `~/.pub-cache/bin` is in
`PATH`). To run from a checkout instead:

```bash
cd tools/osx-cli
dart pub get
dart run bin/osx.dart --help
```

### Prerequisites

| Tool | Needed for | Notes |
|------|-----------|-------|
| **Flutter** (3.41.x) | `init`, `build` | Produces the asset bundle + AOT kernel. Must be on `PATH`. |
| **Docker** | `build` | Runs the per-arch `gen_snapshot` (Linux ELF) in a container. |
| **QEMU** | `run` | `qemu-system-aarch64` and/or `qemu-system-x86_64`. |
| **OSCortex source tree** | all | The CLI orchestrates scripts in the repo. See [Locating the OSCortex tree](#locating-the-oscortex-tree). |

`osx init` checks all of these and tells you what's missing.

---

## Commands

### `osx init`

Run from the root of your Flutter app. It:

1. Confirms you're in a Flutter project.
2. Locates the OSCortex source tree (records it for later commands).
3. Verifies the toolchain (Flutter, Docker, QEMU, host arch + accelerator).
4. Fetches the pinned prebuilt engine **and `gen_snapshot` for both
   architectures** (via the repo's `engine-port/fetch-engine.sh`; the pin lives
   in `engine-port/artifact.config`).
5. Writes `.osx/config.json` (display name, engine pin, OSCortex tree, version).

```bash
cd my_flutter_app
osx init
osx init --name "My App"        # override the launcher display name
osx init --skip-fetch           # record config + verify only (no download)
```

### `osx build`

Compiles the current app into `.osx` bundles. The pipeline:

```
host    flutter pub get
host    flutter build bundle                 -> build/flutter_assets/
host    frontend_server --aot --tfa           -> build/app_aot.dill   (arch-neutral)
docker  gen_snapshot[arm64]  (linux/arm64)    -> libapp-arm64.so
docker  gen_snapshot[x64]    (linux/amd64)    -> libapp-x64.so
pack    per-arch  My App-arm64.osx / My App-x64.osx
pack    universal My App.osx   (both snapshots in one bundle)
```

```bash
osx build                       # both arches + the universal bundle
osx build --arch arm64          # one arch only (no universal bundle)
osx build --name "My App"       # override display name
osx build --output-dir dist     # write bundles somewhere else
osx build --no-assets           # skip staging build/flutter_assets
```

Outputs land in `build/osx/` by default:

```
build/osx/
  My App.osx            ← universal (fat) bundle — ship this
  My App-arm64.osx      ← aarch64-only bundle
  My App-x64.osx        ← x86_64-only bundle
  flutter_assets/       ← fonts, manifests, shaders (arch-neutral)
```

> The AOT kernel (`app_aot.dill`) is architecture-neutral and built once on the
> host. Only `gen_snapshot` is per-arch, and it's a Linux binary, so `build`
> runs it in a throwaway `ubuntu:22.04` container with the matching
> `--platform`. On Apple Silicon the `x86_64` snapshot is produced under
> emulation (correct, just slower).

### `osx run` (alias `osx preview`)

Boots OSCortex in QEMU so you can preview across architectures. The **host-native
arch runs with hardware acceleration** (HVF on macOS, KVM on Linux); the **other
arch is emulated** with TCG.

```bash
osx run                         # boot the host arch, accelerated
osx run --arch x64              # boot x86_64 (emulated on an arm64 host)
osx run --arch arm64            # boot aarch64 (emulated on an x86_64 host)
osx run --image path/to.iso     # boot a specific image
osx run --headless              # no window; serial on stdio
osx run --serial-log /tmp/s.log # capture guest serial to a file
osx run --release v0.1.2        # pick the release the ISO is fetched from
```

Image selection, in order:

1. `--image <path>` if given.
2. A local `aarch64` `.kernel` (checked under `~/OSCortex-run/`, or
   `OSCORTEX_KERNEL`) — the fastest, most reliable interactive path on Apple
   Silicon.
3. Otherwise the release **`.iso`** for the target arch, downloaded and cached
   under `<oscortex>/.image-cache/`.

In the window: click to capture the mouse, `Ctrl+Alt+G` to release, close the
window (or `Ctrl-A X` when headless) to quit.

---

## The universal `.osx` bundle

OSCortex `.osx` bundles start with a fixed 112-byte header (`OSCP` magic, name,
version, and a single AOT snapshot length) — see the kernel's app registry. That
v1 layout carries **one** snapshot.

`osx build` emits a **universal bundle** that is a strict, backward-compatible
superset:

```
offset 0    ┌───────────────────────────────┐
            │ v1 header (OSCP, version 1)    │  ← unchanged; aot_len covers the
            │   aot_len = primary snapshot   │     host-arch (primary) snapshot
offset 112  ├───────────────────────────────┤
            │ primary AOT snapshot           │  ← an unmodified OS boots THIS arch
            ├───────────────────────────────┤
            │ OSXM index                     │  ← (arch token, offset, length)×N
            │ other AOT snapshot(s)          │
            └───────────────────────────────┘
```

* An **OS that only understands v1** reads the header and the first snapshot — it
  always boots the primary (host) arch, so a fat bundle never bricks an older
  build.
* An **arch-aware loader** reads the `OSXM` index and picks the matching arch at
  install/run time.

For maximum compatibility today, `build` also writes plain per-arch v1 bundles
(`-arm64.osx` / `-x64.osx`). Their bytes are identical to what the project's
`tools/oscortex-pack.py` produces. See
[Status](#status--known-limitations) for the kernel-side selection that's still
pending.

---

## Locating the OSCortex tree

`osx` orchestrates scripts inside the OSCortex repo. It finds the tree via, in
order:

1. `--oscortex-root <path>`
2. `OSCORTEX_ROOT` environment variable
3. `oscortexRoot` recorded in your app's `.osx/config.json` (written by `init`)
4. The repo this CLI itself lives in (when run from a checkout)

```bash
export OSCORTEX_ROOT=~/src/oscortex
# or
osx init --oscortex-root ~/src/oscortex
```

---

## Status / known limitations

| Area | State |
|------|-------|
| `osx init` (fetch both engines, config, prereqs) | **Works.** |
| `osx build` (both arches + universal bundle) | **Works.** Per-arch v1 bundles are byte-identical to `oscortex-pack.py`; the universal bundle is verified v1-readable. |
| `osx run` (HVF/KVM host arch + TCG other arch) | **Works** for both arches; auto-fetches the release ISO; prefers a local `.kernel` on aarch64. |
| Kernel-side **arch selection from the fat bundle** | **Pending.** Today's kernel reads only the v1 header (one arch). The universal bundle's `OSXM` index is the forward path; until the kernel learns to select, install the per-arch bundle that matches the target. |
| `osx run` **side-loading a freshly built app** on boot | **Not yet.** `run` boots the OS image (shell + core apps). Installing your own `.osx` into a running guest uses the package pipeline. |
| `build` without Docker | Not supported — `gen_snapshot` is a Linux binary. A native macOS/Windows `gen_snapshot` would remove this dependency. |

---

## License

Dual-licensed: AGPL-3.0-or-later **or** a commercial license. See the `LICENSE`
and `LICENSE-COMMERCIAL` files in the OSCortex repository.
