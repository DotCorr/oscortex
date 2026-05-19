# OSCortex Kernel — Changelog

All notable changes to the OSCortex kernel are recorded here.  
Entries link to the exact files modified and describe what changed and why.
s
---

## [Milestone 23] — Phase 34: Flutter Embedder Callbacks, liboscortex_embedder.so, Dart AOT Loader

### Summary
Three slices that wire the Flutter engine's outbound callback layer to real
OSCortex services, introduce the `liboscortex_embedder.so` platform ABI
library, and add kernel-side Dart AOT snapshot loading.

* **Slice 34-A** — Flutter platform embedder callbacks
  (`tools/flutter-embedder/src/main.rs`):
  Added full Flutter ABI types — `FlutterWindowMetricsEvent`,
  `FlutterPointerEvent`, `FlutterKeyEvent`, `FlutterPlatformMessage`.
  Added fn-pointer types `SendWindowMetricsFn`, `SendPointerEventFn`,
  `SendKeyEventFn`, `OnVsyncFn`.  Fixed `vsync_callback` signature to
  `(user_data, baton: usize)` per Flutter ABI; it now calls
  `engine_vsync_baton_post(baton)` so the APIC-ISR vsync path correctly
  round-trips the baton through `EV_VSYNC` → `FlutterEngineOnVsync`.
  Fixed `platform_message_callback` to parse `FlutterPlatformMessage` and
  forward payload via `platform_msg_post`.  After `FlutterEngineRun` the
  engine handle is stored in `ENGINE` static and an initial
  `FlutterWindowMetricsEvent` is sent.  Event-loop handlers for
  `EV_VSYNC`, `EV_POINTER`, and `EV_KEY` now build the matching Flutter
  event structs and call the engine via the proctable.  Added `rdtsc_ns()`
  helper for microsecond/millisecond timestamps.  Removed duplicate
  `dlsym` call.

* **Slice 34-B** — `liboscortex_embedder.so` platform-callback library
  (`kernel/build.rs`):
  Added `generate_liboscortex_embedder_shim()` which synthesises a
  minimal ELF64 ET_DYN with 8 named stubs
  (`oscortex_surface_present`, `oscortex_vsync_callback`,
  `oscortex_platform_msg_callback`, `oscortex_log_callback`,
  `oscortex_task_post`, `oscortex_embedder_init`,
  `oscortex_get_display_size`, `oscortex_embedder_version`).
  Extracted shared `generate_ret_stub_elf()` helper used by both this and
  the existing flutter-engine stub generator.  The library is staged to
  `system/lib/liboscortex_embedder.so` (656 B).  Also added
  `oscortex_aot_snapshot_load` (syscall 0x366) to `liboscortex.so`
  (now 2320 B, 32 symbols).

* **Slice 34-C** — Dart AOT snapshot loader kernel subsystem
  (`kernel/src/embedder/abi.rs`, `kernel/src/syscall/mod.rs`,
  `tools/flutter-embedder/src/sys.rs`):
  New syscall `0x366 SYS_AOT_SNAPSHOT_LOAD` — accepts a VFS path, validates
  ELF or raw Dart snapshot magic (`\xDC\xDC\xDC\xDC`), allocates anonymous
  pages with `PROT_READ|PROT_EXEC`, copies the snapshot, and writes the
  mapped VA + size to output pointers.  The embedder calls
  `aot_snapshot_load("/system/flutter/app.aot", …)` at startup and logs
  whether a snapshot was found.

### Files Modified
| File | Change |
|------|--------|
| `tools/flutter-embedder/src/main.rs` | ABI types, fn-ptrs, callbacks, event loop, AOT load |
| `tools/flutter-embedder/src/sys.rs` | `SYS_AOT_SNAPSHOT_LOAD` + `aot_snapshot_load()` |
| `kernel/src/embedder/abi.rs` | `SYS_AOT_SNAPSHOT_LOAD = 0x366` |
| `kernel/src/syscall/mod.rs` | `sys_aot_snapshot_load` + dispatch |
| `kernel/build.rs` | `generate_liboscortex_embedder_shim()`, `oscortex_aot_snapshot_load` in shim |

### Build
```
cargo +nightly build --package oscortex-kernel --target x86_64-unknown-none \
    -Z build-std=core,compiler_builtins,alloc \
    -Z build-std-features=compiler-builtins-mem
./scripts/build-iso.sh   # → oscortex.iso  32 MB
```
Initramfs `system/lib/` contents:
- `libflutter_engine.so` — 1344 B (19-symbol stub, unchanged)
- `liboscortex.so` — 2320 B (32 syscall stubs including `oscortex_aot_snapshot_load`)
- `liboscortex_embedder.so` — 656 B (8 platform-callback stubs, new)

---

## [Milestone 22] — Phase 33: Hardware Vsync, liboscortex.so Shim, Double-Buffered Compositor

### Summary
Three slices that promote OSCortex from a single-buffered, best-effort render
loop to a properly timed, tear-free compositing kernel with a real Dart FFI
syscall library.

* **Slice 33-A** — Hardware vsync at 60/120 Hz via TSC gating:
  `kernel/src/arch/x86_64/apic.rs` gains `VSYNC_HZ`, `TSC_HZ`,
  `VSYNC_TSC_PERIOD`, `VSYNC_LAST_TSC` atomics, plus `vsync_due()`,
  `set_vsync_hz()`, and `set_tsc_hz()` helpers.  The APIC timer ISR
  (`kernel/src/arch/x86_64/idt.rs`) calls `vsync_due()` on every tick and
  fires `compositor::tick()` + `wm::tick()` only when a full vsync period has
  elapsed (default 60 Hz).  `kernel/src/cortex/mod.rs` removes the old
  unconditional idle-loop render calls.  New syscall `0x365`
  (`SYS_VSYNC_SET_HZ`) lets userspace reconfigure the rate at runtime;
  `kernel/src/embedder/abi.rs` and `kernel/src/syscall/mod.rs` register it.
  `tools/flutter-embedder/src/sys.rs` + `main.rs` export the helper and call
  `vsync_set_hz(60)` on startup.

* **Slice 33-B** — `liboscortex.so` Dart FFI syscall shim:
  `kernel/build.rs` adds `generate_liboscortex_shim()` which synthesizes a
  2256-byte ELF64 ET_DYN shared object with 31 real working syscall stubs
  (each 16 bytes: `mov r10,rcx` + `mov eax,nr` + `syscall` + `ret` + nop
  padding).  The library exposes every OSCortex syscall under a stable C name
  (`oscortex_write`, `oscortex_surface_create`, …, `oscortex_vsync_set_hz`)
  and is staged to `system/lib/liboscortex.so` in the initramfs so Dart FFI
  code can `DynamicLibrary.open("system/lib/liboscortex.so")` and call kernel
  services with zero overhead.

* **Slice 33-C** — Double-buffered compositor to eliminate tearing:
  `kernel/src/compositor/mod.rs` adds a `back_pending` boolean array to
  `CompositorState`.  `gpu_submit_for()` now writes pixel data to the back
  buffer and sets `back_pending` without touching the front buffer or the
  framebuffer.  At the start of each `render_frame()` call (driven by the
  vsync-gated ISR from 33-A) the back→front swap is performed atomically for
  all pending surfaces before compositing, ensuring the display never shows a
  partially-uploaded frame.

### Files Modified
| File | Change |
|------|--------|
| `kernel/src/arch/x86_64/apic.rs` | TSC vsync gating atomics + helpers |
| `kernel/src/arch/x86_64/idt.rs` | APIC ISR fires compositor at vsync cadence |
| `kernel/src/cortex/mod.rs` | Removed idle-loop render calls |
| `kernel/src/embedder/abi.rs` | `SYS_VSYNC_SET_HZ = 0x365` |
| `kernel/src/syscall/mod.rs` | `sys_vsync_set_hz` dispatch |
| `kernel/src/compositor/mod.rs` | Back-buffer + render_frame flip logic |
| `kernel/build.rs` | `generate_liboscortex_shim()` → 31-symbol ELF |
| `tools/flutter-embedder/src/sys.rs` | `vsync_set_hz()` FFI stub |
| `tools/flutter-embedder/src/main.rs` | Call `vsync_set_hz(60)` on init |

### Build
```
cargo +nightly build --package oscortex-kernel --target x86_64-unknown-none \
    -Z build-std=core,compiler_builtins,alloc \
    -Z build-std-features=compiler-builtins-mem
./scripts/build-iso.sh   # → oscortex.iso  32 MB
```
`system/lib/liboscortex.so` (2256 B) and `system/lib/libflutter_engine.so`
(1344 B) are both present in the initramfs.

---

## [Milestone 21] — Phase 32: Flutter VFS, Platform Channels, Stride GPU, Embedder Binary

### Summary
Four interlocking subsystems that complete the Phase 32 Flutter embedding
milestone:

* **Slice 32-A** — VFS packaging: `kernel/build.rs` now auto-builds a USTAR
  initramfs tar from the `initramfs/` directory and injects a kernel-generated
  stub `libflutter_engine.so` ELF (19 symbols, `ret`-stub code) into
  `system/lib/libflutter_engine.so` so `dlopen` resolves Flutter API symbols
  without needing the 100 MiB real engine binary at build time.
* **Slice 32-B** — Flutter embedder userspace binary
  (`tools/flutter-embedder/`): standalone `no_std` / `no_main` x86_64 ELF
  that registers as engine host, dlopen's the engine library, fills
  `FlutterEngineProcTable`, creates a compositor surface, calls
  `FlutterEngineRun`, and runs a vsync/pointer/key/platform-channel event loop.
  Staged automatically into `initramfs/bin/flutter-embedder` by `build-iso.sh`.
* **Slice 32-C** — Platform channel kernel bridge
  (`kernel/src/platform_channel/`): fixed-capacity message queue with
  `post / recv_into / reply / ack` API; `EV_PLATFORM_MSG = 6` WM event;
  `SYS_PLATFORM_MSG_POST` (0x360), `SYS_PLATFORM_MSG_RECV` (0x361),
  `SYS_PLATFORM_MSG_REPLY` (0x362), `SYS_PLATFORM_MSG_ACK` (0x363).
* **Slice 32-D** — Stride-aware GPU blit: `gpu_submit_strided_for` in the
  compositor extracts each scanline from a strided RGBA32 buffer and delegates
  to the tight-packed fast path; `SYS_GPU_SUBMIT_STRIDED` (0x364).

### What changed

#### `kernel/build.rs`
- Replaced the minimal-placeholder tar generator with a full USTAR tar
  builder: recursively walks `initramfs/` and packs all non-hidden files.
- Added `generate_flutter_engine_stub()` — pure-Rust ELF64 ET_DYN generator
  that produces a ~1400-byte minimal shared object exporting 19 Flutter Engine
  API symbols as `xor eax,eax; ret` stubs.
- The stub ELF is always injected as `system/lib/libflutter_engine.so` in the
  embedded tar (overrides any directory copy), keeping the kernel binary
  build-time self-contained.

#### `kernel/src/platform_channel/mod.rs` (new)
- `post(sender_pid, channel, payload) → Result<u64, &str>` — enqueue message,
  return sequence number.
- `recv_into(buf) → usize` — serialize oldest pending message into a user
  buffer using `[seq:u64][ch_len:u16][data_len:u32][channel][payload]` wire
  format.
- `reply(seq, data) → Result<()>` — attach a reply to an existing message.
- `ack(seq, buf) → usize` — copy reply out and remove message from queue.
- `notify_engine_host(seq, channel_hash)` — broadcasts `EV_PLATFORM_MSG` into
  the WM event queue so the engine host's poll loop wakes up.
- `reap_pid(pid)` — prune messages from dead processes.

#### `kernel/src/embedder/abi.rs`
- Added `EV_PLATFORM_MSG = 6`.
- Added `FOCUS_GAINED = 2` and `FOCUS_MIRROR = 3` (previously missing, fixed
  compile errors in wm/mod.rs).
- Added `SYS_PLATFORM_MSG_POST` (0x360), `SYS_PLATFORM_MSG_RECV` (0x361),
  `SYS_PLATFORM_MSG_REPLY` (0x362), `SYS_PLATFORM_MSG_ACK` (0x363),
  `SYS_GPU_SUBMIT_STRIDED` (0x364).
- Added `FlutterProjectArgs` repr(C) struct (assets_path, icu_data_path,
  dart_entrypoint args, vsync_callback, platform_message_callback,
  log_message_callback, reserved).
- Added `FlutterSoftwareRendererConfig` repr(C) struct (surface_present_callback).
- Added `FlutterRendererConfig` repr(C) struct (renderer_type + software union).

#### `kernel/src/syscall/mod.rs`
- `dispatch_fast` extended from 3 args to 4 (`arg3: u64`).
- Added `sys_platform_msg_post`, `sys_platform_msg_recv`,
  `sys_platform_msg_reply`, `sys_platform_msg_ack`.
- Added `sys_gpu_submit_strided`.
- All five wired into the match dispatch table.

#### `kernel/src/arch/x86_64/syscall.rs`
- Syscall entry stub: added `mov r8, r10` to forward R10 (Linux syscall arg3)
  as the 5th SysV argument (r8) to `dispatch_fast`.

#### `kernel/src/compositor/mod.rs`
- Added `pub fn gpu_submit_strided_for(caller_pid, id, payload, row_bytes)`:
  reconstructs tight-packed rows from strided input then delegates to the
  existing `gpu_submit_for` fast path.

#### `kernel/src/main.rs`
- Added `mod platform_channel;`.

#### `scripts/build-iso.sh`
- Added step `[0/4]`: builds `tools/flutter-embedder` before the kernel and
  stages the resulting ELF to `initramfs/bin/flutter-embedder`.

#### `scripts/make-initramfs.sh` (new)
- Manual helper to pack `initramfs/` → `initramfs.tar` using the host's `tar`.

#### `scripts/embed-flutter-engine.sh` (new)
- Copies a real `libflutter_engine.so` into `initramfs/system/lib/` and
  prints rebuild instructions.

#### `tools/flutter-embedder/` (new standalone crate)
- `Cargo.toml` — `[workspace]` to opt out of the root workspace, `no_std`.
- `build.rs` — passes `-T user.ld` and `-z noexecstack` linker flags.
- `user.ld` — userspace ELF linker script; load address `0x400000`.
- `.cargo/config.toml` — default target `x86_64-unknown-none`.
- `src/sys.rs` — typed syscall wrappers for all OSCortex ABI numbers
  (syscall0–syscall4 inline asm), `WmEvent` repr(C).
- `src/main.rs` — `_start` naked entry, full embedder logic:
  `engine_host_register → dlopen → dlsym → proctable_set → surface_create →
  FlutterEngineRun → event loop (vsync/pointer/key/platform-channel)`.

### Build
```
# Kernel (includes initramfs with stub libflutter_engine.so):
cargo +nightly build --package oscortex-kernel --target x86_64-unknown-none \
    -Z build-std=core,compiler_builtins,alloc \
    -Z build-std-features=compiler-builtins-mem

# Full ISO (builds embedder + kernel + stages everything):
./scripts/build-iso.sh
```

### Next syscall slot
`0x365` — next available Phase 33+.

### ISO size
~32 MiB (up from 29 MiB; +2.7 MiB flutter-embedder debug binary staged in initramfs).

---

## [Milestone 20] — Phase 31: FlutterEngineProcTable Bridge + GPU Fast-Path + Shared-Address-Space Threads

### Summary
Three interlocking subsystems that complete the Flutter engine embedding
contract:

* **Slice A** — `FlutterEngineProcTable` kernel bridge: ABI struct definition,
  `sys_engine_proctable_set / _get`, and the vsync-baton feedback loop that
  lets the kernel carry Flutter's `FlutterEngineOnVsync` baton inside every
  `EV_VSYNC` event.
* **Slice B** — `sys_gpu_submit`: single-call upload-and-blit path for the
  engine's software renderer `present_callback`, bypassing the double-buffer
  for minimum display latency.
* **Slice C** — Shared-address-space threads: `spawn_thread`,
  `clone_thread` (POSIX `clone(2)` compat), per-thread kernel stacks, PML4
  sharing, and thread-aware `exit`.

### What changed

#### `kernel/src/arch/x86_64/syscall.rs`
- Added `static mut SYSCALL_USER_RIP: u64` saved in the SYSCALL naked stub
  alongside `SYSCALL_USER_RSP`.
- Added `pub fn user_rip() -> u64` and `pub fn user_rsp() -> u64` accessors
  so the `clone(2)` implementation can fork register state at the syscall site.

#### `kernel/src/embedder/abi.rs`
- Added `FlutterEngineProcTable` repr(C) struct (8 function-pointer fields +
  8 reserved u64 slots) that mirrors the embedder-side proc-table layout.
- New syscall constants (0x356–0x35C):
  - `SYS_ENGINE_PROCTABLE_SET` (0x356)
  - `SYS_ENGINE_PROCTABLE_PTR_GET` (0x357)
  - `SYS_ENGINE_VSYNC_BATON_POST` (0x358)
  - `SYS_GPU_SUBMIT` (0x359)
  - `SYS_THREAD_CREATE` (0x35A)
  - `SYS_THREAD_EXIT` (0x35B)
  - `SYS_THREAD_JOIN` (0x35C)

#### `kernel/src/syscall/mod.rs`
- Added `ENGINE_PROC_TABLE_PTR: AtomicU64` for the registered proc-table VA.
- **Slice A implementations**: `sys_engine_proctable_set`, `sys_engine_proctable_ptr_get`, `sys_engine_vsync_baton_post`.
- **Slice B implementation**: `sys_gpu_submit(surface_id, pixel_ptr, pixel_len)` delegates to `compositor::gpu_submit_for`.
- **Slice C implementations**: `sys_thread_create`, `sys_thread_exit`, `sys_thread_join`, `sys_clone` (POSIX clone(2) — CLONE_VM only).
- **Linux compat**: syscall 56 (clone) → `sys_clone`; syscall 186 (gettid) → `sys_getpid`.
- All 7 new syscalls wired into `dispatch_fast`.

#### `kernel/src/wm/mod.rs`
- Added `VSYNC_BATON: AtomicU64`.
- `push_vsync(frame)` now atomically consumes the pending baton and places it
  in `WmEvent.b` — the embedder reads this to call `FlutterEngineOnVsync` with
  the correct baton value.
- New `pub fn set_vsync_baton(baton: u64)` — called by `sys_engine_vsync_baton_post`.

#### `kernel/src/compositor/mod.rs`
- New `pub fn gpu_submit_for(caller_pid, surface_id, payload)`:
  - Writes `payload` (RGBA32, tight-packed) directly to the surface front
    buffer (bypasses double-buffering).
  - Immediately blits the surface region to the physical framebuffer via
    `drivers::fb::blit_rgba32` — no full compositor re-render needed.

#### `kernel/src/process/mod.rs`
- `Process` struct: added `is_thread: bool` and `parent_pid: u32` fields.
- `Process::empty()` and `spawn()` initialize them to `false`/`0`.
- `exit(pid, code)`: skips `paging::free_user_pml4` when `p.is_thread == true` — threads share the parent's PML4.
- New `pub fn spawn_thread(parent_pid, entry_fn, arg, stack_size)`:
  allocates user stack via `dl::mmap_anon`, separate syscall stack + xstate,
  shares parent `pml4_phys`, sets `is_thread=true`.
- New `pub fn clone_thread(parent_pid, child_rip, child_rsp)`:
  clones parent register snapshot, overrides RIP/RSP/RAX=0 for POSIX
  `clone(2)` semantics.

### ABI version
`EMBEDDER_ABI_VERSION` remains 8 (struct layout additions are additive).  
Next available syscall slot: **0x35D**.

---

### Summary
Completed the actual runtime loader/linker path that consumes the Phase 30
Slice 2 kernel contract.  The Flutter engine host can now call `sys_dlopen`
to load `libflutter_engine.so` from the VFS directly into its own address
space, resolve symbols with `sys_dlsym`, and allocate executable JIT code
regions with `sys_mmap`.

### What changed
- **kernel/src/process/dl.rs** *(new)*:
  - Full ELF64 ET_DYN parser: walks PT_LOAD, PT_DYNAMIC, and section tables.
  - Per-process bump VA allocator starting at `LIB_VA_BASE` (16 MiB).
  - `PT_LOAD` segment mapper through `paging::map_user_page_with_flags`.
  - Relocation engine: `R_X86_64_RELATIVE`, `R_X86_64_64`,
    `R_X86_64_GLOB_DAT`, `R_X86_64_JUMP_SLOT`.  Undefined externals left at
    zero (Flutter fills mandatory callbacks via `FlutterEngineProcTable`).
  - `harvest_exports`: scans `.dynsym`/`.dynstr` to build a per-handle symbol
    table for `dlsym` lookups.
  - Public API: `dlopen(pid, pml4_phys, elf_bytes) -> Result<u32, &str>`,
    `dlsym(handle, name) -> Option<u64>`, `dlclose(handle, pid)`.
  - `mmap_anon(pid, pml4_phys, hint_va, pages, prot) -> u64` for anonymous
    memory allocation (used by Dart VM JIT code heap).
- **kernel/src/process/mod.rs**:
  - Added `pub mod dl;` declaration.
- **kernel/src/embedder/abi.rs**:
  - Added `SYS_DLOPEN` (`0x350`), `SYS_DLSYM` (`0x351`), `SYS_DLCLOSE`
    (`0x352`), `SYS_MMAP` (`0x353`), `SYS_MUNMAP` (`0x354`),
    `SYS_MPROTECT` (`0x355`).
- **kernel/src/syscall/mod.rs**:
  - Added `sys_dlopen`, `sys_dlsym`, `sys_dlclose` handlers that read path/
    name from user space, look up the file in the VFS, and call into
    `process::dl`.
  - Added `sys_mmap` (allocates anonymous pages via `dl::mmap_anon`),
    `sys_munmap` (stub), `sys_mprotect` (stub — permissions set at map time).
  - Wired all six into `dispatch_fast` at their private ABI numbers.
  - Also wired Linux-compatible numbers 9 (mmap), 10 (mprotect), 11 (munmap),
    12 (brk stub) so standard toolchain-linked Flutter binaries work without
    a separate shim.
  - Updated syscall-number comment header.

### Design rationale
Rather than a separate userspace `ld.so`, the kernel itself acts as a minimal
dynamic linker.  This avoids bootstrapping a second privileged process and
keeps the TCB small.  The relocation engine handles the dominant Flutter engine
reloc types; unsatisfied externals stay zero since the engine's platform
binding is entirely table-driven (no ELF-linked external symbols required at
load time).  `sys_mmap` / `sys_mprotect` give the Dart VM the memory regions
it needs for JIT code generation.

### Build
- 0 errors, 41 warnings (all pre-existing, unchanged from Milestone 18).
- ISO: `oscortex.iso` 29 MiB.

---

### Summary
Continued Phase 30 by adding kernel-side runtime host binding and engine
library-path discovery syscalls so a Flutter launcher/runtime process can
self-identify as the engine host and coordinate app launches.

### What changed
- kernel/src/embedder/abi.rs:
  - Added `SYS_ENGINE_HOST_REGISTER` (`0x345`).
  - Added `SYS_ENGINE_HOST_PID_GET` (`0x346`).
  - Added `SYS_ENGINE_LIBRARY_PATH_READ` (`0x347`).
- kernel/src/syscall/mod.rs:
  - Added `ENGINE_HOST_PID` kernel state.
  - Added `sys_engine_host_register(flags)` to bind current PID as runtime host.
  - Added `sys_engine_host_pid_get()` to query current runtime host PID.
  - Added `sys_engine_library_path_read(dst_ptr, dst_len)` returning
    `/system/lib/libflutter_engine.so` bytes.
  - Updated `sys_app_launch_path()` to also emit a targeted `EV_APP/APP_LAUNCH`
    notification to the registered engine host with launched app PID in `a`.
  - Wired all new syscalls into fast dispatcher.

### Notes
- This slice establishes runtime contract APIs, not full shared-object loading.
  Actual `libflutter_engine.so` dynamic loader/runtime linker execution is the
  next Phase 30 slice.

### Build result
- 0 errors, 41 warnings (pre-existing), ISO 29 M — validated ✓

### Files changed
- kernel/src/embedder/abi.rs
- kernel/src/syscall/mod.rs

---

## [Milestone 17] — Flutter Runtime Orchestration ABI Bootstrap (Phase 30, Slice 1)

### Summary
Started Phase 30 implementation by stabilizing launcher/embedder orchestration
syscalls so a Flutter-based shell can discover engine policy and spawn app
processes through a canonical kernel ABI.

### What changed
- kernel/src/embedder/abi.rs:
  - ABI version bumped to `8`.
  - Added `SYS_APP_LAUNCH_PATH` (`0x342`).
  - Added `SYS_ENGINE_POLICY_GET` (`0x343`).
  - Added `SYS_ENGINE_VERSION_PACKED` (`0x344`).
  - Added engine policy constants:
    - `ENGINE_LOADER_DYNAMIC`
    - `ENGINE_LOADER_STATIC`
    - `ENGINE_TARGET_FLUTTER_3_29`
- kernel/src/syscall/mod.rs:
  - Added `sys_app_launch_path(path_ptr, path_len, flags)`.
  - Added `sys_engine_policy_get()` packed policy return.
  - Added `sys_engine_version_packed()` packed version return.
  - Wired all three into `dispatch_fast`.

### Notes
- Input routing is already WM-managed and process-aware:
  pointer events hit-test top surface owner first, then fallback to focus.
- App launch syscall currently spawns ELF from VFS/initramfs and emits
  `APP_LAUNCH` event to the target PID for userspace runtime coordination.

### Build result
- 0 errors, 41 warnings (pre-existing), ISO 29 M — validated ✓

### Files changed
- kernel/src/embedder/abi.rs
- kernel/src/syscall/mod.rs

---

## [Milestone 16] — Multi-Process User Scheduling (Phase 29) + Phase 30 Defaults

### Summary
Phase 29 is complete: user processes now support round-robin preemptive
scheduling with per-process syscall kernel stacks and address-space (`CR3`)
switching on timer preemption.

This milestone also pins Phase 30 defaults so work can continue autonomously
without waiting on external decisions.

### Phase 29 details
1. **Per-process syscall kernel stacks**:
   each process now allocates its own 8 KiB syscall stack at spawn and frees it
   on exit; the SYSCALL fastpath stack pointer is switched on process switch.
2. **Per-process xstate image**:
   each process stores its own XSAVE/FXSAVE context image.
3. **Timer preemption process switch**:
   APIC timer preemption now saves the interrupted process GPR+xstate, selects
   the next runnable PID, switches `CR3`, restores next regs+xstate, and returns
   with `iretq` into the selected process.
4. **Exit handoff**:
   `sys_exit` now transitions directly to the next runnable process instead of
   halting indefinitely with interrupts masked.

### Phase 30 defaults selected (autonomous pin)
- **Engine target**: Flutter engine stable family `3.29.x` (host integration ABI baseline).
- **Loader strategy**: runtime dynamic load path first (`/system/lib/libflutter_engine.so`).
- **First app target**: minimal shell app (single-window smoke app) before external apps.

### What changed
- kernel/src/process/mod.rs:
  - Added per-process syscall stack allocation/free.
  - Added per-process xstate storage and save/restore helpers.
  - Added user-context query + round-robin next PID helpers.
  - Added `enter_user_by_pid_noreturn(pid)` shared launch path.
- kernel/src/arch/x86_64/idt.rs:
  - APIC timer handler now performs user process round-robin switching with
    `CR3` update + per-process register/xstate restoration.
- kernel/src/arch/x86_64/syscall.rs:
  - Added switchable `ACTIVE_SYSCALL_STACK_TOP` and setter used on process switch.
- kernel/src/arch/x86_64/cpu.rs:
  - Added generic `save_xstate_to` / `restore_xstate_from` helpers.
- kernel/src/syscall/mod.rs:
  - Updated `sys_exit` to hand off to next runnable userspace process.

### Build result
- 0 errors, 41 warnings (pre-existing), ISO 29 M — validated ✓

### Files changed
- kernel/src/process/mod.rs
- kernel/src/arch/x86_64/idt.rs
- kernel/src/arch/x86_64/syscall.rs
- kernel/src/arch/x86_64/cpu.rs
- kernel/src/syscall/mod.rs

---

## [Milestone 15] — Full User Preemption Context Save/Restore (Phase 28)

### Summary
Phase 28 is now complete: APIC timer preemption preserves full userspace CPU
execution state across scheduler switches.

Before this change, timer preemption could switch tasks while only the six
callee-saved kernel registers were persisted by `context_switch`, which is not
sufficient for interrupted ring-3 code. The APIC timer path now:
1. Saves all general-purpose registers (RAX-R15) in a naked ISR entry stub.
2. Detects ring-3 preemption (`CS.RPL == 3`) and snapshots FPU/SIMD state via
   `XSAVEOPT`/`XSAVE` (with `FXSAVE64` fallback when XSAVE is unavailable).
3. Performs normal scheduler tick/preemption.
4. Restores FPU/SIMD state and all GPRs before `iretq`.

This makes timer-driven preemption transparent to userspace register state,
which is the critical unblocking requirement for real embedder workloads.

### What changed
- kernel/src/arch/x86_64/idt.rs:
  - APIC timer vector now points to a dedicated naked preemption entry stub.
  - Added full GPR save/restore path around timer scheduling.
  - Added ring-3 preemption detection and userspace register snapshot to
    process state (`save_regs`).
- kernel/src/arch/x86_64/cpu.rs:
  - Added XSAVE capability detection/configuration (`CR4.OSXSAVE`, `XCR0`).
  - Added preemption xstate save/restore helpers using `XSAVEOPT`/`XSAVE` and
    `XRSTOR` (or `FXSAVE64`/`FXRSTOR64` fallback).

### Build result
- 0 errors, 41 warnings (pre-existing), ISO 28 M — validated ✓

### Files changed
- kernel/src/arch/x86_64/idt.rs
- kernel/src/arch/x86_64/cpu.rs

---

## [Milestone 14] — Scheduler Integration + Userspace SYSRET (Phases 25–27)

### Summary
Phases 25–27 complete the path from kernel initialisation to an actual
ring-3 user process running under the preemptive scheduler.

**Phase 25 — APIC timer → scheduler** (already wired): The APIC timer ISR
already calls `sched::tick()`, and `cortex::run()` already registers the BSP
as task-0 and spawns the cortex-bg heartbeat task. No code changes were needed;
this phase is confirmed complete.

**Phase 26 — VFS + initramfs ELF loader** (already wired): `fs::init()` mounts
the embedded USTAR initramfs, `fs::lookup("/init")` returns ELF bytes, and
`process::spawn` loads them into a fresh PML4 with a 64 KiB user stack. No code
changes were needed; this phase is confirmed complete.

**Phase 27 — Userspace context switch (SYSRET path)**: Three bugs blocked actual
ring-3 execution:
1. **GDT not SYSRET64-compliant**: `SYSRET64` loads CS and SS from fixed offsets
   of `STAR[63:48]` (CS = base+16|3, SS = base+8|3). The GDT lacked a dedicated
   `user_cs_64` slot at the correct position. Fixed by inserting a `code64(ring3)`
   descriptor at 0x28 and moving the TSS descriptors to 0x30/0x38. `USER_CS` is
   now `0x2B` and `TSS_SELECTOR` is `0x30`.
2. **Incorrect STAR MSR value**: The old code set `STAR[63:48] = USER_CS − 8`
   which resolved to wrong segment selectors on SYSRET. Fixed to use the constant
   base `0x18` (`USER_SYSRET_BASE`).
3. **Syscall entry ran on user stack with wrong arg ordering**: The old naked stub
   ran on the user RSP and called `dispatch_fast` with `rdi = rax` (clobbering
   arg0). Fixed: the new entry point switches to a dedicated 8 KiB
   `SYSCALL_KERNEL_STACK`, saves/restores user RSP in `SYSCALL_USER_RSP`, and
   correctly rearranges `(rax, rdi, rsi, rdx) → (rdi, rsi, rdx, rcx)` for the
   `dispatch_fast(number, a0, a1, a2)` SysV ABI.
4. **`process::spawn` never executed the process**: spawn created the descriptor
   but nothing ever SYSRET'd into it. Added `schedule_user_launch(pid)` and the
   `user_launch_task` kernel-sched trampoline: when scheduled it switches CR3 to
   the user PML4 and executes `sysretq`, transferring execution to ring 3.
   `main.rs` now calls `process::schedule_user_launch(pid)` after a successful
   `/init` spawn.

### What changed
- kernel/src/arch/x86_64/gdt.rs:
  - Added `USER_CS_BASE = 0x18` constant (SYSRET base).
  - `USER_CS` changed from `0x1B` to `0x2B` (user CS64 at 0x28 | RPL3).
  - `TSS_SELECTOR` changed from `0x28` to `0x30`.
  - GDT entries[3] = user_cs32 placeholder (0x18), entries[4] = user_ds (0x20),
    entries[5] = user_cs64 code64 ring3 (0x28), entries[6/7] = TSS (0x30/0x38).
- kernel/src/arch/x86_64/syscall.rs:
  - Added `SyscallStack` struct + `SYSCALL_KERNEL_STACK` (8 KiB, 16-byte aligned).
  - Added `SYSCALL_STACK_TOP` (cached pointer) and `SYSCALL_USER_RSP` scratch slot.
  - `init()`: caches stack top, uses `USER_SYSRET_BASE = 0x18` for STAR[63:48].
  - `syscall_entry`: switches to kernel stack, saves user RSP/RIP/RFLAGS,
    rearranges regs for `dispatch_fast`, restores user state, `sysretq`.
- kernel/src/process/mod.rs:
  - Added `PENDING_INIT_PID: AtomicU32` scratch static.
  - Added `schedule_user_launch(pid)`: stores PID, spawns "user-init" sched task.
  - Added `user_launch_task()`: reads PTABLE, sets CR3, `sysretq` to ring 3 (noreturn).
- kernel/src/main.rs:
  - After successful `process::spawn`, now calls `process::schedule_user_launch(pid)`.

### Build result
- 0 errors, 41 warnings (pre-existing), ISO 28 M — validated ✓

### Files changed
- kernel/src/arch/x86_64/gdt.rs
- kernel/src/arch/x86_64/syscall.rs
- kernel/src/process/mod.rs
- kernel/src/main.rs

---

## [Milestone 13] — Double-Buffered Surfaces (Phase 24)

### Summary
Added opt-in double-buffering per surface. `SYS_SURFACE_FLIP` atomically swaps
the back buffer to front and triggers a render frame. Upload always targets the
back buffer once double-buffering is active (lazy allocation on first flip),
eliminating visible tearing for app-driven compositing.

### What changed
- kernel/src/compositor/mod.rs:
  - CompositorState: added back_buffers array (one per surface slot).
  - destroy_surface_for: clears back_buffers slot on destroy.
  - upload_surface_rgba32_for: writes to back buffer when allocated.
  - surface_flip_for: lazy back-buffer alloc, take()-based atomic swap, triggers render.
- kernel/src/embedder/abi.rs: added SYS_SURFACE_FLIP = 0x312.
- kernel/src/syscall/mod.rs: added sys_surface_flip handler, wired dispatcher.
- scripts/make-init-elf.py: SYS_SURFACE_FLIP added to compositor contiguity check.

### Files changed
- kernel/src/compositor/mod.rs
- kernel/src/embedder/abi.rs
- kernel/src/syscall/mod.rs
- scripts/make-init-elf.py

---

## [Milestone 13] — Surface Damage Tracking (Phase 23)

### Summary
Added per-surface dirty-rectangle tracking. Apps mark regions modified
(`SYS_SURFACE_DAMAGE_SET`) and query the current dirty state
(`SYS_SURFACE_DAMAGE_GET`). Damage coordinates are clamped to surface bounds.
This is the foundation for partial-refresh optimisations in the compositor.

### What changed
- kernel/src/compositor/mod.rs:
  - Surface struct: damage_x/y (i32), damage_w/h (u32), has_damage (bool) fields.
  - Surface::empty: initialises damage fields to zero/false.
  - create_surface_internal: initialises damage fields in new surfaces.
  - surface_damage_set_for: owner-enforced, clamps to surface bounds.
  - surface_damage_get: returns (x, y, w, h, has_damage).
- kernel/src/embedder/abi.rs: SYS_SURFACE_DAMAGE_SET = 0x310, SYS_SURFACE_DAMAGE_GET = 0x311.
- kernel/src/syscall/mod.rs: sys_surface_damage_set / sys_surface_damage_get handlers.
- scripts/make-init-elf.py: new constants added; compositor range now covers 0x300–0x312.

### Files changed
- kernel/src/compositor/mod.rs
- kernel/src/embedder/abi.rs
- kernel/src/syscall/mod.rs
- scripts/make-init-elf.py

---

## [Milestone 13] — Process Resource Limits (Phase 22)

### Summary
Enforces a per-PID surface cap (MAX_SURFACES_PER_PID = 8) inside the compositor
allocator so no single process can exhaust the 32-slot global surface table.
Exposed via SYS_PROC_SURFACE_COUNT for userspace introspection.

### What changed
- kernel/src/compositor/mod.rs:
  - Added MAX_SURFACES_PER_PID = 8 constant.
  - create_surface_internal: counts owner's surfaces before allocation; returns
    Err("per-pid surface limit reached") when at cap.
  - surface_count_for(pid): public query of owned surface count.
- kernel/src/embedder/abi.rs: SYS_PROC_SURFACE_COUNT = 0x341.
- kernel/src/syscall/mod.rs: sys_proc_surface_count handler, dispatcher entry.
- scripts/make-init-elf.py: SYS_PROC_SURFACE_COUNT in app/process contiguity range.

### Files changed
- kernel/src/compositor/mod.rs
- kernel/src/embedder/abi.rs
- kernel/src/syscall/mod.rs
- scripts/make-init-elf.py

---

## [Milestone 13] — App Lifecycle Events (Phase 21)

### Summary
Establishes the EV_APP lifecycle contract: APP_LAUNCH, APP_TERMINATE, APP_PAUSE,
APP_RESUME subkinds delivered as targeted WM events. SYS_APP_NOTIFY lets the WM
or kernel send lifecycle events to any PID. ABI version bumped to 7.

### What changed
- kernel/src/embedder/abi.rs:
  - EMBEDDER_ABI_VERSION bumped 6 → 7.
  - APP_LAUNCH=1, APP_TERMINATE=2, APP_PAUSE=3, APP_RESUME=4 constants.
  - SYS_APP_NOTIFY = 0x340.
- kernel/src/wm/mod.rs: push_app_event(target_pid, subkind, surface_id) helper.
- kernel/src/syscall/mod.rs: sys_app_notify handler, dispatcher entry.
- scripts/make-init-elf.py: APP_* constants validated; new app/process range
  0x340–0x341 checked for contiguity.

### Files changed
- kernel/src/embedder/abi.rs
- kernel/src/wm/mod.rs
- kernel/src/syscall/mod.rs
- scripts/make-init-elf.py

---

## [Milestone 13] — Surface Visibility and Clipping (Phase 20)

### Summary
Added surface visibility and clip-region control for efficient partial rendering,
window occlusion optimization, and clean hide/show semantics. Clip regions are
clamped to surface bounds and default to full surface on creation.

### What changed
- kernel/src/compositor/mod.rs:
  - extended Surface struct with visible, clip_x/y/w/h fields.
  - updated create_surface_internal to initialize visibility=true, clip=full_surface.
  - added surface_visibility_get() and surface_visibility_set_for().
  - added surface_clip_set_for() with automatic clamping to surface bounds.
- kernel/src/embedder/abi.rs:
  - added SYS_SURFACE_VISIBILITY_GET = 0x30D.
  - added SYS_SURFACE_VISIBILITY_SET = 0x30E.
  - added SYS_SURFACE_CLIP_SET = 0x30F.
- kernel/src/syscall/mod.rs:
  - added sys_surface_visibility_get / sys_surface_visibility_set syscall handlers.
  - added sys_surface_clip_set syscall handler.
  - dispatcher wired to new visibility/clip syscalls.
- scripts/make-init-elf.py:
  - strict ABI validator requires visibility/clip syscall constants.
  - compositor syscall contiguity check extended through 0x30F.

### Files changed
- kernel/src/compositor/mod.rs
- kernel/src/embedder/abi.rs
- kernel/src/syscall/mod.rs
- scripts/make-init-elf.py

---

## [Milestone 13] — Surface Geometry Introspection (Phase 19)

### Summary
Added dedicated syscalls for surface geometry query and modification,
enabling apps to control their window position and size independently
from the create/move split.

### What changed
- kernel/src/embedder/abi.rs:
  - added SYS_SURFACE_GEOMETRY_GET = 0x30B.
  - added SYS_SURFACE_GEOMETRY_SET = 0x30C.
- kernel/src/compositor/mod.rs:
  - added surface_geometry_get() to query position/dimensions.
  - added surface_geometry_set_for() for safe geometry mutation with owner checks.
- kernel/src/syscall/mod.rs:
  - added sys_surface_geometry_get / sys_surface_geometry_set syscall handlers.
  - arg packing: xy_packed=((x<<32)|y), wh_packed=((w<<32)|h).
  - dispatcher wired to new geometry syscalls.
- scripts/make-init-elf.py:
  - strict ABI validator requires geometry syscall constants.
  - compositor syscall contiguity check extended through 0x30C.

### Files changed
- kernel/src/embedder/abi.rs
- kernel/src/compositor/mod.rs
- kernel/src/syscall/mod.rs
- scripts/make-init-elf.py

---

## [Milestone 13] — Input Routing Through Surface Stack (Phase 18)

### Summary
Pointer events now route to the topmost (highest z-order) surface at the cursor
coordinates via surface hit-testing, enabling intuitive multi-window input
behavior. Fallback to focus-based routing if no surface is hit.

### What changed
- kernel/src/embedder/abi.rs:
  - bumped EMBEDDER_ABI_VERSION from 5 to 6 (reflects input routing semantic change).
- kernel/src/compositor/mod.rs:
  - added surface_at_point(x, y) helper for z-order-aware hit-testing.
  - returns (surface_id, owner_pid) for topmost surface at point.
- kernel/src/wm/mod.rs:
  - updated push_pointer() to route via surface_at_point hit-test.
  - preserves fallback to focus-based or broadcast routing if no surface hit.
  - key events remain focus-routed for now.

### Files changed
- kernel/src/embedder/abi.rs
- kernel/src/compositor/mod.rs
- kernel/src/wm/mod.rs

---

## [Milestone 13] — Surface Z-Order Management (Phase 17)

### Summary
Added explicit z-order (stacking) query and manipulation syscalls for proper
surface composition and window layering control.

### What changed
- kernel/src/embedder/abi.rs:
  - added SYS_SURFACE_Z_GET = 0x309.
  - added SYS_SURFACE_Z_SET = 0x30A.
- kernel/src/compositor/mod.rs:
  - added surface_z_get() to query current z-order of a surface.
  - added surface_z_set_for() to manipulate z-order with owner enforcement.
- kernel/src/syscall/mod.rs:
  - added sys_surface_z_get / sys_surface_z_set syscall handlers.
  - dispatcher wired to new surface z-order syscalls.
  - updated syscall documentation.
- scripts/make-init-elf.py:
  - strict ABI validator now requires z-order syscall constants.
  - compositor syscall contiguity check extended through 0x30A.

### Files changed
- kernel/src/embedder/abi.rs
- kernel/src/compositor/mod.rs
- kernel/src/syscall/mod.rs
- scripts/make-init-elf.py

---

## [Milestone 13] — Optional Focus Mirror Broadcast (Phase 16)

### Summary
Added an optional global focus-transition mirror event channel for shell/taskbar
observers while preserving targeted focus events for app processes.

### What changed
- kernel/src/embedder/abi.rs:
  - added SYS_WM_FOCUS_MIRROR_GET = 0x335.
  - added SYS_WM_FOCUS_MIRROR_SET = 0x336.
  - added FOCUS_MIRROR focus-event flag.
- kernel/src/wm/mod.rs:
  - added runtime mirror toggle state.
  - set_focus_pid now optionally emits broadcast EV_FOCUS mirror event
    (`flags=FOCUS_MIRROR`, `a=old_focus_pid`, `b=new_focus_pid`).
  - targeted FOCUS_LOST/FOCUS_GAINED events remain unchanged.
- kernel/src/syscall/mod.rs:
  - added wm_focus_mirror_get / wm_focus_mirror_set syscalls.
- scripts/make-init-elf.py:
  - strict ABI validator now requires focus mirror syscall constants and
    contiguous introspection range through 0x336.

### Files changed
- kernel/src/embedder/abi.rs
- kernel/src/wm/mod.rs
- kernel/src/syscall/mod.rs
- scripts/make-init-elf.py

---

## [Milestone 13] — Focus Transition Events (Phase 15)

### Summary
Added deterministic focus-loss/focus-gain WM event emission so apps can react
to foreground changes explicitly rather than inferring focus state indirectly.

### What changed
- kernel/src/embedder/abi.rs:
  - added EV_FOCUS = 5.
  - added focus transition flags:
    - FOCUS_LOST = 1
    - FOCUS_GAINED = 2
- kernel/src/wm/mod.rs:
  - set_focus_pid now emits targeted EV_FOCUS events on transitions:
    - old focused PID gets FOCUS_LOST with new PID in payload
    - new focused PID gets FOCUS_GAINED with previous PID in payload
  - no-op transition (same PID) emits no events.
- scripts/make-init-elf.py:
  - strict ABI validator now requires EV_FOCUS as part of the stable contract.

### Files changed
- kernel/src/embedder/abi.rs
- kernel/src/wm/mod.rs
- scripts/make-init-elf.py

---

## [Milestone 13] — WM Focus + Foreground Policy Primitives (Phase 14)

### Summary
Added kernel-side focus primitives and input routing policy so key/pointer
events are explicitly routed to the focused owner PID instead of being only
globally visible.

### What changed
- kernel/src/embedder/abi.rs:
  - added SYS_WM_FOCUS_PID_GET = 0x333.
  - added SYS_WM_FOCUS_SURFACE_SET = 0x334.
  - bumped EMBEDDER_ABI_VERSION from 4 to 5.
- kernel/src/wm/mod.rs:
  - added WM focus state (`focus_pid`, `set_focus_pid`).
  - pointer/key event producers now route events to focused PID when set,
    with broadcast fallback when focus is 0.
- kernel/src/syscall/mod.rs:
  - added wm_focus_pid_get syscall.
  - added wm_focus_surface_set syscall with ownership enforcement:
    caller can only focus a surface they own.
- scripts/make-init-elf.py:
  - strict ABI validator now requires/fixes contiguous introspection range
    through 0x334.

### Files changed
- kernel/src/embedder/abi.rs
- kernel/src/wm/mod.rs
- kernel/src/syscall/mod.rs
- scripts/make-init-elf.py

---

## [Milestone 13] — Surface Owner Introspection (Phase 13)

### Summary
Added a stable compositor owner-query syscall so runtime layers can inspect
surface ownership and enforce higher-level window management policy.

### What changed
- kernel/src/embedder/abi.rs:
  - added SYS_SURFACE_OWNER = 0x308.
  - bumped EMBEDDER_ABI_VERSION from 3 to 4.
- kernel/src/compositor/mod.rs:
  - added surface_owner(id) -> Option<pid> helper.
- kernel/src/syscall/mod.rs:
  - added surface_owner_pid syscall handler and dispatch wiring.
  - returns owner PID on success, ESRCH (-3) when surface id is unknown.
- scripts/make-init-elf.py:
  - strict ABI validator now requires SYS_SURFACE_OWNER.
  - compositor syscall contiguity guard now includes 0x308.

### Files changed
- kernel/src/embedder/abi.rs
- kernel/src/compositor/mod.rs
- kernel/src/syscall/mod.rs
- scripts/make-init-elf.py

---

## [Milestone 13] — Surface Ownership Enforcement (Phase 12)

### Summary
Enforced PID ownership for compositor surfaces so userspace processes cannot
move, upload, present, or destroy surfaces they do not own.

### What changed
- `kernel/src/compositor/mod.rs`:
  - added per-surface owner table (`owners: [u32; MAX_SURFACES]`).
  - introduced owner-aware APIs:
    - `create_surface_for(owner_pid, ...)`
    - `move_surface_for(caller_pid, ...)`
    - `upload_surface_rgba32_for(caller_pid, ...)`
    - `present_surface_for(caller_pid, ...)`
    - `destroy_surface_for(caller_pid, ...)`
  - preserved compatibility wrappers for internal kernel callers.
- `kernel/src/syscall/mod.rs`:
  - surface syscalls now call owner-aware compositor APIs using caller PID.
  - cross-process surface access now returns `EPERM` (`-1`).
- `kernel/src/embedder/abi.rs`:
  - bumped `EMBEDDER_ABI_VERSION` from `2` to `3` for ownership-enforcement
    semantic change.

### Files changed
- `kernel/src/compositor/mod.rs`
- `kernel/src/syscall/mod.rs`
- `kernel/src/embedder/abi.rs`

---

## [Milestone 13] — PID-Scoped WM Event Streams (Phase 11)

### Summary
Added groundwork for per-process WM event stream isolation by introducing
internal event ownership metadata and filtering syscall reads/polls to the
current userspace PID.

### What changed
- `kernel/src/wm/mod.rs`:
  - event queue now tracks owner PID per entry (`0` = broadcast).
  - added targeted push API: `push_event_for(owner_pid, ...)`.
  - added per-consumer operations:
    - `pending_count_for(pid)`
    - `pop_event_for(pid)`
  - dequeue path now returns first event visible to the caller PID while
    preserving existing ABI event struct layout.
- `kernel/src/syscall/mod.rs`:
  - WM poll/read/wait now operate on caller-visible events only.
  - `wm_event_stats_packed` now reports caller-visible pending count plus
    global dropped counter.
  - `EV_APP` injection is now targeted to the caller PID; broadcast behavior
    remains for `EV_VSYNC`, pointer, and key paths.
- `kernel/src/embedder/abi.rs`:
  - bumped `EMBEDDER_ABI_VERSION` from `1` to `2` for stream-visibility
    semantic change.

### Files changed
- `kernel/src/wm/mod.rs`
- `kernel/src/syscall/mod.rs`
- `kernel/src/embedder/abi.rs`

---

## [Milestone 13] — WM Queue Telemetry ABI (Phase 10)

### Summary
Added a stable WM event-queue telemetry syscall so embedder runtimes can read
queue pressure (pending + dropped) and adapt polling/consumption strategy.

### What changed
- `kernel/src/embedder/abi.rs`:
  - added `SYS_WM_EVENT_STATS = 0x332`.
- `kernel/src/syscall/mod.rs`:
  - added `wm_event_stats_packed` syscall handler.
  - return format is packed as `(dropped << 32) | pending`.
  - wired dispatch for `SYS_WM_EVENT_STATS`.
- `scripts/make-init-elf.py`:
  - strict ABI validator now requires/validates `SYS_WM_EVENT_STATS`.
  - introspection syscall block contiguity now includes `0x332`.
  - generated `/init` flow now issues `SYS_WM_EVENT_STATS` probe syscall.

### Files changed
- `kernel/src/embedder/abi.rs`
- `kernel/src/syscall/mod.rs`
- `scripts/make-init-elf.py`

---

## [Milestone 13] — ABI Drift Guardrails for Init Generator (Phase 9)

### Summary
Hardened userspace init ELF generation with strict embedder ABI contract
validation so CI fails loudly on ABI type/format/layout drift, not just
missing constants.

### What changed
- `scripts/make-init-elf.py` now enforces strict ABI contract checks when
  reading `kernel/src/embedder/abi.rs`:
  - validates required constants exist
  - validates constant Rust types (`u64`/`u32`) match expected contract
  - validates literal format rules (syscalls in hex, event/version IDs in
    decimal)
  - validates numeric range for typed constants
  - validates contiguous syscall blocks for compositor, WM, and ABI
    introspection ranges
- Added structural ABI checks:
  - requires `#[repr(C)]` on `WmEvent`
  - validates canonical `WmEvent` field order/types
  - validates `WM_EVENT_SIZE` remains canonical
    `core::mem::size_of::<WmEvent>()`
- Failure mode is now explicit `RuntimeError` with precise drift cause and
  source file path, improving CI diagnosis.

### Files changed
- `scripts/make-init-elf.py`

---

## [Milestone 13] — Userspace ABI Handshake in Init Generator (Phase 8)

### Summary
Updated the generated `/init` userspace test flow to consume the centralized
embedder ABI contract and perform explicit ABI/runtime handshake syscalls
before WM event waiting begins.

### What changed
- `scripts/make-init-elf.py` now loads constants directly from
  `kernel/src/embedder/abi.rs` instead of hardcoding private bridge syscall
  numbers and event-kind values.
- Added startup ABI handshake sequence in generated machine code:
  - calls `SYS_EMBEDDER_ABI_VERSION` and compares against
    `EMBEDDER_ABI_VERSION`
  - calls `SYS_WM_EVENT_SIZE` and uses the returned size for
    `wm_event_wait` length arguments
- Replaced hardcoded `wm_event_wait(..., 32, ...)` lengths with runtime ABI
  event-size value.
- Increased event-buffer reservation in generated payload to provide headroom
  while still relying on runtime event-size for syscall arguments.

### Files changed
- `scripts/make-init-elf.py`

---

## [Milestone 13] — Stable Embedder ABI Contract (Phase 7)

### Summary
Centralized embedder-facing event/syscall ABI into a single in-kernel contract
module so external runtimes can bind against stable constants and layouts.

### What changed
- Added new module:
  - `kernel/src/embedder/mod.rs`
  - `kernel/src/embedder/abi.rs`
- `abi.rs` now defines:
  - `EMBEDDER_ABI_VERSION`
  - centralized private syscall numbers (`0x300+`, `0x320+`)
  - WM event kind constants
  - canonical `#[repr(C)] WmEvent` layout and `WM_EVENT_SIZE`
- `kernel/src/wm/mod.rs` now consumes ABI constants/types from
  `crate::embedder::abi` instead of local duplicated definitions.
- `kernel/src/syscall/mod.rs`:
  - dispatch now uses centralized ABI syscall constants
  - event kind matching uses centralized constants
  - added ABI introspection syscalls:
    - `0x330` → `embedder_abi_version`
    - `0x331` → `wm_event_size`
- `kernel/src/main.rs`: registers `mod embedder;`.

### Files changed
- `kernel/src/embedder/mod.rs`
- `kernel/src/embedder/abi.rs`
- `kernel/src/wm/mod.rs`
- `kernel/src/syscall/mod.rs`
- `kernel/src/main.rs`

---

## [Milestone 13] — Hardware Input Polling (Phase 6)

### Summary
Upgraded WM input producers from synthetic-only to a hybrid model:
PS/2 keyboard/mouse hardware polling on x86_64 with synthetic fallback when
no real hardware events are present.

### What changed
- `kernel/src/wm/mod.rs`:
  - added PS/2 controller polling path:
    - keyboard bytes via ports `0x64/0x60`
    - mouse packet decode (3-byte packets)
  - keyboard scancode events are pushed to WM queue via `push_key()`
  - mouse motion/buttons are pushed via `push_pointer()`
  - synthetic generators remain active only when no hardware input is seen
    on a tick (stable fallback for bring-up/CI).

### Files changed
- `kernel/src/wm/mod.rs`

---

## [Milestone 13] — Synthetic Input Producers (Phase 5)

### Summary
Added kernel-side synthetic keyboard/mouse event producers and wired them into
the WM queue tick path so userspace receives pointer/key traffic alongside
vsync/app events.

### What changed
- `kernel/src/wm/mod.rs`:
  - added synthetic input state machine (`SYNTH`) and `wm::tick()`
  - generates bouncing pointer motion events every 2 ticks
  - toggles synthetic key press/release (scancode `0x39`) every 120 ticks
- `kernel/src/cortex/mod.rs`:
  - Cortex maintenance now calls `crate::wm::tick()` after compositor tick.
- `scripts/make-init-elf.py`:
  - userspace test now performs two blocking `wm_event_wait` calls to validate
    streaming event consumption, not just one-shot event fetch.

### Files changed
- `kernel/src/wm/mod.rs`
- `kernel/src/cortex/mod.rs`
- `scripts/make-init-elf.py`

---

## [Hotfix] — Limine UEFI Config Discovery + WM Blocking Wait

### Summary
Fixed removable-UEFI boot config discovery and added a minimal blocking event
wait syscall so userspace event loops can avoid busy polling.

### What changed
- `scripts/build-iso.sh`:
  - now mirrors `limine.conf` to:
    - `/boot/limine/limine.conf`
    - `/limine.conf`
    - `/EFI/BOOT/limine.conf`
  - improves compatibility with firmware paths that fail to search
    `/boot/limine/` on USB UEFI boot.
- `kernel/src/syscall/mod.rs`:
  - added `0x323 wm_event_wait(ev_ptr, ev_len, max_halts)`
  - bounded blocking wait using `sti; hlt; cli` on x86_64 to reduce userspace
    busy-loop polling pressure.
- `scripts/make-init-elf.py`:
  - userspace test now uses `0x323` instead of non-blocking single read.

### Files changed
- `scripts/build-iso.sh`
- `kernel/src/syscall/mod.rs`
- `scripts/make-init-elf.py`

---

## [Milestone 13] — WM Event Bridge (Phase 4)

### Summary
Added a minimal window-manager event queue and syscall bridge so userspace
can poll/read events in parallel with frame upload/present.

### What changed
- `kernel/src/wm/mod.rs`:
  - new fixed-size event ring (`cap=256`) with sequence numbers
  - event kinds: `EV_VSYNC`, `EV_POINTER`, `EV_KEY`, `EV_APP`
  - helpers: `pending_count()`, `pop_event()`, `push_*()`
- `kernel/src/main.rs`:
  - initializes WM event bridge during boot (`wm::init()`).
- `kernel/src/compositor/mod.rs`:
  - emits vsync events on every composed frame (`wm::push_vsync(frame)`).
- `kernel/src/syscall/mod.rs`:
  - added private WM syscalls:
    - `0x320` `wm_event_poll()`
    - `0x321` `wm_event_read(ptr, len)`
    - `0x322` `wm_event_inject(kind, arg1, arg2)`
  - added safe user write helper for event copy-out.
- `scripts/make-init-elf.py`:
  - test ELF now injects one `EV_APP` event and reads one WM event via `0x321`.

### Files changed
- `kernel/src/wm/mod.rs`
- `kernel/src/main.rs`
- `kernel/src/compositor/mod.rs`
- `kernel/src/syscall/mod.rs`
- `scripts/make-init-elf.py`

---

## [Milestone 13] — Flutter Bridge Primitives (Phase 3)

### Summary
Added the first embedder-facing bridge primitives so userspace can stream
pixel buffers into compositor surfaces and synchronize on compositor frames.

### What changed
- `kernel/src/compositor/mod.rs`:
  - added per-surface backbuffers (`Vec<u32>`) and presented flags
  - added `upload_surface_rgba32(id, payload)`
  - added `present_surface(id)`
  - added frame sync helpers:
    - `framebuffer_size_packed()`
    - `frame_counter()`
    - `wait_vsync(last_seen)` (non-blocking)
  - render path now blits uploaded RGBA buffers when present, else falls back
    to generated fill color.
- `kernel/src/drivers/fb.rs`:
  - added `blit_rgba32(x, y, w, h, src)` to copy user/compositor RGBA buffers
    into the XRGB framebuffer.
- `kernel/src/syscall/mod.rs`:
  - added private compositor bridge syscalls:
    - `0x303` `surface_upload_rgba32(id, ptr, len)`
    - `0x304` `surface_present(id)`
    - `0x305` `fb_size_packed()`
    - `0x306` `vsync_counter()`
    - `0x307` `vsync_wait_nonblock(last_seen)`
  - increased user copy ceiling to 2 MiB for frame payload ingestion.

### Files changed
- `kernel/src/compositor/mod.rs`
- `kernel/src/drivers/fb.rs`
- `kernel/src/syscall/mod.rs`
- `scripts/make-init-elf.py`

### Validation path
- Added `scripts/make-init-elf.py` to generate a userspace `/init` ELF that:
  - creates a compositor surface via syscall `0x300`
  - positions it via `0x301`
  - uploads a 200x120 RGBA gradient via `0x303`
  - presents via `0x304`
  - writes an init marker to stdout and halts
- Repacked `initramfs.tar` with the generated `/init` and rebuilt ISO.

---

## [Milestone 13] — Compositor Render Path (Phase 2)

### Summary
Upgraded M13 from control-plane scaffold to visible framebuffer composition.
The compositor now renders actual surfaces every kernel tick and includes a
boot-time animated demo surface to verify drawing/animation on real hardware.

### What changed
- `kernel/src/drivers/fb.rs`:
  - added framebuffer helper API for compositor use:
    - `is_ready()`
    - `size_px()`
    - `fill_rect(x, y, w, h, color)`
  - stores framebuffer width atomically (`FB_WIDTH`) during init.
- `kernel/src/compositor/mod.rs`:
  - added `render_frame()`:
    - snapshots active surfaces
    - stable sorts by `z`
    - clears framebuffer background
    - draws surfaces + title strips
  - added `tick()` animation loop with horizontal bounce demo.
  - `init()` now spawns a visual self-test surface when framebuffer is ready.
- `kernel/src/cortex/mod.rs`:
  - Cortex maintenance `tick()` now calls `crate::compositor::tick()` so
    composition runs continuously.

### Files changed
- `kernel/src/drivers/fb.rs`
- `kernel/src/compositor/mod.rs`
- `kernel/src/cortex/mod.rs`

---

## [Milestone 13] — Compositor Scaffold (Phase 1)

### Summary
Started M13 with a fixed-size compositor surface table and private syscall
plumbing for surface lifecycle operations.

### What changed
- `kernel/src/compositor/mod.rs`: new module with `create_surface()`,
  `move_surface()`, `destroy_surface()`, `active_surfaces()` and a static
  `MAX_SURFACES=32` table.
- `kernel/src/main.rs`: compositor initialised during boot (`compositor::init()`).
- `kernel/src/syscall/mod.rs`: added private compositor syscall ABI:
  - `0x300` surface_create(width, height)
  - `0x301` surface_move(id, packed_xy, z)
  - `0x302` surface_destroy(id)

### Files changed
- `kernel/src/compositor/mod.rs`
- `kernel/src/main.rs`
- `kernel/src/syscall/mod.rs`

---

## [Milestone 12] — Runtime Hardening (PID/Wait/Kill)

### Summary
Replaced placeholder PID handling in syscall paths with process-runtime
tracking and added wait/kill primitives.

### What changed
- `kernel/src/process/mod.rs`:
  - added `CURRENT_PID` tracking (`set_current_pid()`, `current_pid()`)
  - added `waitpid()` reaping support for zombie processes
- `kernel/src/main.rs`: after spawning `/init`, binds active userspace PID via
  `process::set_current_pid(pid)`.
- `kernel/src/syscall/mod.rs`:
  - `getpid`, `exit`, `ipc_recv` now use runtime current PID (not hardcoded `1`)
  - added Linux-compatible syscall handlers:
    - `61` `wait4` → `process::waitpid()`
    - `62` `kill` (SIGKILL=9 wired)
  - fixed x86 poweroff path unreachable warning by keeping halt fallback only
    on non-x86 targets.

### Files changed
- `kernel/src/process/mod.rs`
- `kernel/src/main.rs`
- `kernel/src/syscall/mod.rs`

---

## [Hotfix] — Early Boot Visibility Probe (Mac EFI)

### Summary
Added a pre-banner framebuffer probe pattern to make early boot visibility
reliable on old Mac EFI systems with inconsistent GOP pixel formats.

### What changed
- `kernel/src/drivers/fb.rs`: added `early_probe_pattern()` that draws
  grayscale gradient bars + checkerboard in the top of the screen.
- `kernel/src/main.rs`: probe is called before `early_banner()` when
  framebuffer `bpp >= 24`.

### Why
If the screen stays totally black, we now know either framebuffer mapping is
invalid or execution did not reach kernel entry. If the probe appears, early
boot and framebuffer writes are confirmed before deeper init.

### Files changed
- `kernel/src/drivers/fb.rs`
- `kernel/src/main.rs`

---

## [Hotfix] — Mac EFI Black Screen (Framebuffer Format Compatibility)

### Summary
Fixed early visible output on older Mac EFI systems where Limine exposes a
non-32bpp framebuffer mode. The early boot banner path previously hard-gated
on 32bpp and would skip drawing entirely, causing apparent black-screen boot.

### What changed
- `kernel/src/main.rs`: early banner call now accepts any framebuffer with
  `bpp >= 24` instead of requiring `bpp == 32`.
- `kernel/src/drivers/fb.rs`: `early_banner()` now writes pixels via byte
  addressing for both 24bpp and 32bpp layouts.

### Files changed
- `kernel/src/main.rs`
- `kernel/src/drivers/fb.rs`

---

## [Milestone 12] — Core POSIX Syscalls + ACPI Shutdown + Real IPC

### Summary
Wired the full POSIX syscall layer, ACPI soft-off, and message-passing IPC.

### Syscalls added (`kernel/src/syscall/mod.rs`)
| Nr  | Name        | Notes |
|-----|-------------|-------|
| 0   | read        | returns EOF (no terminal yet) |
| 1   | write       | fd 1/2 → framebuffer console |
| 2   | open        | looks up VFS; no fd table yet |
| 3   | close       | no-op |
| 39  | getpid      | returns placeholder PID 1 |
| 59  | execve      | loads ELF from VFS, returns new PID |
| 60  | exit        | calls `process::exit()`, halts core |
| 231 | exit_group  | same as exit |
| 0x200 | ipc_send  | route to `ipc::send()` |
| 0x201 | ipc_recv  | route to `ipc::recv()` |
| 0xC0  | poweroff  | ACPI S5 via `arch::acpi_shutdown()` |

### ACPI shutdown (`kernel/src/arch/x86_64/acpi.rs`)
- Added `pub fn shutdown() -> !`.
- Walks RSDP → XSDT/RSDT → FADT, reads `PM1a_CNT_BLK`, writes `SLP_TYPa|SLP_EN`.
- Falls back to QEMU (0x604:0x2000), Bochs, and VirtualBox ports.

### IPC (`kernel/src/ipc/mod.rs`)
- Replaced stub with real message-passing: 64-byte messages, 16-deep ring-buffer
  inbox per process (indexed by PID), protected by `spin::Mutex`.
- `send(dst_pid, data)` / `recv(src_pid) -> Option<&[u8]>` public API.

### Files changed
- `kernel/src/syscall/mod.rs` — full POSIX + IPC + poweroff handlers
- `kernel/src/arch/x86_64/acpi.rs` — `shutdown()` + `fadt_pm1a_cnt()` added
- `kernel/src/arch/mod.rs` — `pub fn acpi_shutdown() -> !` wrapper
- `kernel/src/ipc/mod.rs` — full ring-buffer message-passing implementation

---

## [Milestone 11] — Virtual Filesystem + Embedded Initramfs

### Summary
Added a minimal VFS layer and a USTAR tar reader. The kernel embeds an `initramfs.tar`
at compile time (built by `kernel/build.rs`). At boot, the archive is mounted at `/`
and the kernel attempts to spawn `/init` as the first userspace process.

### Files changed
- `kernel/src/fs/mod.rs` — VFS trait, mount table, `lookup()`, `init()`
- `kernel/src/fs/initramfs.rs` — USTAR parser, `EmbeddedRamFs` singleton, `mount_embedded()`
- `kernel/build.rs` — generates a minimal placeholder `initramfs.tar` in `OUT_DIR` if the
  real archive is absent; real archive at `initramfs.tar` repo-root takes precedence
- `kernel/src/main.rs` — added `mod fs; mod process;`; calls `fs::init()` at step 7b;
  spawns `/init` at step 9b

---

## [Milestone 10] — Process Isolation + ELF Loader + User Page Tables

### Summary
Implemented process descriptors, an ELF64 loader, and the user address-space
page-table infrastructure needed to run Ring-3 code.

### Files changed
- `kernel/src/process/mod.rs` — process table (256 slots), `spawn()`, `exit()`, `kill()`,
  `get_regs()`, `save_regs()`. Allocates a fresh PML4 per process, loads ELF, maps stack.
- `kernel/src/process/elf.rs` — ELF64 header/phdr parser; walks PT_LOAD segments,
  allocates frames, copies data, zeroes BSS, maps each page into the process PML4.
- `kernel/src/mm/paging.rs` — added user page-table API:
  `alloc_user_pml4()`, `map_user_page()`, `map_user_page_with_flags()`,
  `free_user_pml4()`, and internal `map_page_in()` / `ensure_next_table_flags()`.

---

## [Milestone 9] — SMP Multi-Core (AP Wake via Limine Protocol)

### Summary
Brought all Application Processors (APs) online using the Limine SMP protocol.
With `-smp 2` in QEMU, both CPUs now boot: the BSP completes full kernel init and then
wakes the AP via `MpInfo::bootstrap()`, which jumps to a dedicated `ap_entry` entry
point on the AP.  The AP runs its own GDT/IDT/APIC/FPU/SYSCALL init, marks itself
online, and enters the scheduler idle loop.  Serial output confirms:
`[SMP] AP cpu=1 lapic_id=1 online` → `[Sched] AP cpu=1 entering idle loop` →
`[SMP] 2/2 CPU(s) online` → BSP enters Cortex-managed loop.

### Root causes fixed
- **`SMP_REQUEST` placement**: Limine scans the kernel binary for request magic bytes.
  Requests defined in sub-modules can be missed; moving `SMP_REQUEST` to `main.rs`
  alongside all other working requests ensures reliable detection.
- **Busy TSS on AP**: `gdt::init_ap()` previously called `ltr TSS_SELECTOR`.  The TSS
  descriptor's "busy" bit is set by the BSP's `ltr`; a second `ltr` on a busy TSS
  raises #GP before the AP IDT is installed → triple fault → silent reset.  Removed
  `ltr` from `init_ap()` (no ring-3 syscalls yet, so TSS is not needed on APs).

### Files changed
- `kernel/src/main.rs` — added `MpRequest` import; added `static SMP_REQUEST`;
  passes `SMP_REQUEST.response()` to `arch::smp_init()`.
- `kernel/src/arch/x86_64/smp.rs` — new module: `PerCpuData`, `CPU_COUNT`,
  `this_cpu()`, `ap_entry`, `init(smp_resp)`.  `SMP_REQUEST` removed from here.
- `kernel/src/arch/x86_64/mod.rs` — `smp_init(resp)` signature updated; `ap_init()`
  reorder and export.
- `kernel/src/arch/x86_64/gdt.rs` — `init_ap()` no longer calls `ltr`; reloads
  segment registers for AP.
- `kernel/src/arch/x86_64/apic.rs` — added `local_apic_id()` reading x2APIC ID MSR.
- `kernel/src/arch/x86_64/acpi.rs` — new module: RSDP + MADT lookup via Limine.
- `kernel/src/arch/aarch64/mod.rs` — `smp_init()` stub updated to accept response.
- `kernel/src/arch/riscv64/mod.rs` — `smp_init()` stub updated to accept response.
- `kernel/src/sched/mod.rs` — added `ap_start(cpu_idx: u32) -> !` idle loop.

---

## [Milestone 8] — Real Hardware USB Boot

### Summary
Added `scripts/flash-usb.sh` — a self-contained shell script that writes
`oscortex.iso` to a USB drive for real-hardware boot.  The ISO is already a hybrid
BIOS+UEFI image (Limine + xorriso `--protective-msdos-label`), so a plain `dd` is
sufficient.

The script:
- Auto-detects removable disks on **macOS** (`diskutil list`) and **Linux** (`lsblk`)
- Refuses to write to internal disks (`disk0` on macOS, `nvme0n1`/`sda` on Linux)
- Uses the raw device path on macOS (`/dev/rdiskN`) for maximum write speed
- Requires typing `YES` to confirm before any destructive write
- Prints progress via `dd` status

### Files changed
- `scripts/flash-usb.sh` — new script, `chmod +x` applied.

---

## [Milestone 7] — Built-in AI Model Capsule + Integer-Only NN Inference

### Summary
Replaced the placeholder "heuristic mode" inference engine with a **fully embedded
2-layer feed-forward neural network** baked directly into the kernel binary as the
`BUILTIN_CAPSULE` static byte array.  The network is an **anomaly detector** with
architecture **4-input → 8-hidden (ReLU) → 1-output (clamp to u8)** running entirely
on integer arithmetic — no FPU, safe on the interrupt stack.

A new `CortexCapsule` binary format (`b"CRTX"` magic, version byte, kind byte, three
`le u16` dimension fields, then raw `i8` weights and `i32` biases) is defined.
`load_builtin_capsule()` validates the magic, logs dimensions, then sets
`model_loaded = true` and `heuristic_mode = false`.  `init()` calls it immediately, so
the kernel boots with NN inference active — no external file or boot-time loading step
required.

The new `model_infer()` method executes:
1. Read 4 u8 inputs (zero-padded if input is shorter)
2. Hidden layer: `h[j] = clamp(ReLU(Σ W1[i][j]·x[i] >> 4 + B1[j]), 0, 255)`
3. Output layer: `score = clamp(Σ W2[j]·h[j] >> 4 + B2, 0, 255)`
4. Return `InferenceResult::AnomalyScore { source: input[0] as u64, score }`

`infer()` now dispatches to `model_infer()` when `model_loaded && !heuristic_mode`,
falling back to heuristics only if the capsule failed to load.

**Verified in QEMU** — serial output confirms both lines appear at boot:
```
[Cortex::Inference] Model capsule loaded (version=1 kind=0x01 inputs=4 hidden=8 outputs=1)
[Cortex::Inference] Built-in model capsule loaded — NN inference ACTIVE (4→8→1 i8-quantised)
```

### Files changed

| File | Change |
|------|--------|
| [kernel/src/cortex/inference.rs](kernel/src/cortex/inference.rs) | Added `model_loaded` field to `InferenceEngine`. New `BUILTIN_CAPSULE: [u8; 88]` static (CRTX header + W1/B1/W2/B2). New `load_builtin_capsule()` — validates magic, logs metadata, sets `model_loaded=true`. New `model_infer()` — 2-layer integer NN. `infer()` dispatches to `model_infer()` when model loaded. `init()` now calls `load_builtin_capsule()` and logs the ACTIVE message. |

---

## [Milestone 6] — WASM Driver Sandbox (Cortex Driver Protocol)

### Summary
Implemented a **minimal WASM 1.0 interpreter** embedded in the kernel for sandboxed
driver execution. Drivers are distributed as WebAssembly 1.0 binaries conforming to the
**Cortex Driver Protocol (CDP)**: they export `cdp_version() -> i32` and
`cdp_init() -> i32`, and interact with the kernel only via imported host functions
(index 0 = `host::klog_write`). Memory and table instructions are explicitly rejected
(SFI — drivers use only host-call imports and pure computation).

A built-in `NULL_DRIVER_WASM` byte array (hand-assembled WASM) is loaded at kernel
init to validate the sandbox is functional on every boot.  The `DriverRegistry` now
stores `sandbox: Option<Box<WasmSandbox>>` on each `DriverInstance`.  Hot-replace
(`replace()`) also uses the new WASM path.

**Verified in QEMU** — serial output confirms both lines appear at boot:
```
[Cortex::DriverGen] Driver 'null-driver' loaded via WASM sandbox (id=0)
[Cortex::DriverGen] Built-in null driver loaded (id=0) — WASM sandbox OK
```

### Files changed

| File | Change |
|------|--------|
| [kernel/src/drivers/wasm_sandbox.rs](kernel/src/drivers/wasm_sandbox.rs) | **New file.** ~250-line minimal WASM 1.0 interpreter. `WasmSandbox::new(bytes)` parses and validates a WASM module. `call(name, args)` finds an export and executes it. `call_func()` is the stack-machine interpreter with `MAX_STACK=256`, `MAX_LOCALS=64`, `MAX_CALL_DEPTH=16`. Supported opcodes: control flow (block/loop/if/else/end/br/br_if/return/call), locals, i32 arithmetic/comparison. Memory/table ops are rejected. `dispatch_host(idx)` stub dispatches host imports. |
| [kernel/src/drivers/mod.rs](kernel/src/drivers/mod.rs) | Added `pub mod wasm_sandbox;` |
| [kernel/src/cortex/driver_gen.rs](kernel/src/cortex/driver_gen.rs) | `DriverInstance` gets `sandbox: Option<Box<WasmSandbox>>` field. `NULL_DRIVER_WASM` static hand-assembled bytes. `load_wasm_driver(wasm, vtable)` — creates sandbox, calls `cdp_version`/`cdp_init`. `init()` loads null driver via sandbox. `load()` uses `load_wasm_driver()`. `replace()` now uses `load_wasm_driver()` (was calling dead `jit_compile` stub). |

---

## [Milestone 5] — Preemptive Round-Robin Scheduler + x86_64 Context Switch

### Summary
Replaced the stub scheduler with a **fully functional preemptive kernel scheduler**.
Each kernel task owns a heap-allocated 32 KiB stack. `spawn_kernel_task()` builds an
initial stack frame so that the first `context_switch` into a new task transparently
falls through `arch::task_entry`, enables interrupts via `sti`, and calls the task
function. `context_switch` is a `#[naked]` x86_64 function that saves/restores the
six callee-saved registers (rbx, rbp, r12-r15), stores RSP into `*old_sp`, and loads
`new_sp` as the new RSP. The APIC timer ISR now sends EOI **before** calling
`sched::tick()` so the APIC can queue the next timer interrupt while we're in a new
task. `cortex::run()` registers the BSP idle loop as task-0 (`cortex-idle`), spawns a
background Cortex worker as task-1 (`cortex-bg`), then enters `sti; schedule; tick;
hlt` loop — preemption also fires every 5 APIC ticks (~50 ms).

**Verified in QEMU** — serial output confirms both cooperative and preemptive paths:
```
[Sched] registered 'cortex-idle' as task-0 (pid=0)
[Sched] spawned 'cortex-bg' pid=1 stack=0xffff800000072fc0
[Cortex::BG] background worker started — scheduler ALIVE
```

### Files changed

| File | Change |
|------|--------|
| [kernel/src/sched/mod.rs](kernel/src/sched/mod.rs) | Full rewrite. `Task` struct with `kernel_sp`, `stack_base`, `name`. `MAX_TASKS=64`, `STACK_SIZE=32 KiB`, `SCHED_SLICE=5`. `register_current_as_task0()`, `spawn_kernel_task()`, `tick()`, `schedule()` with lock-release-before-switch pattern. `AtomicU64` PID counter. |
| [kernel/src/arch/x86_64/mod.rs](kernel/src/arch/x86_64/mod.rs) | Added `context_switch(old_sp: *mut u64, new_sp: u64)` (naked asm: push rbx/rbp/r12-r15, save RSP, load RSP, pop, ret) and `task_entry()` (naked asm: pop rdi, sti, call rdi, park). |
| [kernel/src/arch/aarch64/mod.rs](kernel/src/arch/aarch64/mod.rs) | Added no-op `context_switch` and spin-loop `task_entry` stubs. |
| [kernel/src/arch/riscv64/mod.rs](kernel/src/arch/riscv64/mod.rs) | Same no-op stubs. |
| [kernel/src/arch/x86_64/idt.rs](kernel/src/arch/x86_64/idt.rs) | `apic_timer_handler`: moved `eoi()` before `sched::tick()` so APIC unblocks while new task runs. |
| [kernel/src/cortex/mod.rs](kernel/src/cortex/mod.rs) | `run()` now calls `register_current_as_task0("cortex-idle")` and `spawn_kernel_task("cortex-bg", cortex_background)` before entering the idle loop. Added `cortex_background()` heartbeat task. |

---

## [Milestone 4] — Proper 4-Level Page Tables + `map_mmio()` + Multi-Arch Build

### Summary
Replaced the stub `mm/paging.rs` with a fully functional **4-level page table walker** for
x86_64, and made the entire kernel build cleanly on all three supported architectures
(**x86_64**, **aarch64**, **riscv64gc**). Limine's existing CR3 is preserved (Option A —
no CR3 replacement). `map_page()` walks PML4→PDPT→PD→PT, allocating intermediate zeroed
frames on demand via the bitmap frame allocator. `map_mmio()` maps an arbitrary physical
MMIO window with write-through, cache-disable, no-execute flags. All x86_64-specific
paging, serial port I/O, and ISA feature declarations are now guarded by `#[cfg]` so
aarch64 and riscv64gc compile cleanly (they call stub implementations).

### Files changed

| File | Change |
|------|--------|
| [kernel/src/mm/paging.rs](kernel/src/mm/paging.rs) | Full rewrite with arch split. `PageFlags` bitflags (shared). `#[cfg(target_arch = "x86_64")] mod x86_64_impl` contains real CR3 walker, `invlpg`, `alloc_page_table`, `ensure_next_table`, `map_page`, `map_mmio`, `init`. `#[cfg(not(...))] mod stub_impl` contains no-op stubs for other arches. Public API functions dispatch via cfg. |
| [kernel/src/mm/mod.rs](kernel/src/mm/mod.rs) | Added `pub use paging::{map_mmio, map_page, PageFlags};` re-exports. |
| [kernel/build.rs](kernel/build.rs) | Made arch-aware: reads `CARGO_CFG_TARGET_ARCH` and selects `x86_64.ld`, `aarch64.ld`, or `riscv64.ld` automatically. |
| [kernel/linker/riscv64.ld](kernel/linker/riscv64.ld) | **New file.** RISC-V 64 linker script (`OUTPUT_ARCH(riscv)`, entry `kernel_main`, higher-half at `0xFFFFFFFF80000000`, `.text`/`.rodata`/`.data`/`.bss` sections). |
| [kernel/src/arch/aarch64/mod.rs](kernel/src/arch/aarch64/mod.rs) | Added `pub struct InterruptFrame { ip, spsr, esr, far }` stub (used by `cortex::interrupt_hook`). `ip` field mirrors x86_64 naming. |
| [kernel/src/arch/riscv64/mod.rs](kernel/src/arch/riscv64/mod.rs) | Added `pub struct InterruptFrame { ip, scause, stval, sstatus }` stub. Same rationale. |
| [kernel/src/logger.rs](kernel/src/logger.rs) | Wrapped `SerialPort`, `cpu_out8`, `cpu_in8` in `#[cfg(target_arch = "x86_64")]`. Non-x86 serial is a no-op stub — framebuffer console still receives all output on those targets. |
| [kernel/src/main.rs](kernel/src/main.rs) | Changed `#![feature(abi_x86_interrupt)]` and `#![feature(naked_functions)]` to `#![cfg_attr(target_arch = "x86_64", ...)]` to avoid unused-feature errors on aarch64/riscv64. |
| [.cargo/config.toml](.cargo/config.toml) | Added `relocation-model=static` for `aarch64-unknown-none` and `riscv64gc-unknown-none-elf` targets (same reason as x86_64: Limine needs ET_EXEC). |

### Details

**Page table design:**
```
Virtual address bits  →  Table level
  [47:39]  (9 bits)  →  PML4 index
  [38:30]  (9 bits)  →  PDPT index
  [29:21]  (9 bits)  →  PD   index
  [20:12]  (9 bits)  →  PT   index
  [11:0]   (12 bits) →  Page offset (4 KiB)
```

**`PageFlags::mmio()`:** `PRESENT | WRITABLE | CACHE_DISABLE | WRITE_THROUGH | NO_EXECUTE`  
**`PageFlags::kernel_rw()`:** `PRESENT | WRITABLE | NO_EXECUTE`  
**Physical address mask:** `0x000f_ffff_ffff_f000` (bits 51:12, strips all flag bits)

**Strategy — Option A (extend Limine's CR3):**  
We never write to CR3. We simply walk the already-active PML4 and insert/update entries.  
Intermediate tables that are absent get freshly allocated zeroed frames. `invlpg` flushes
the TLB for each page immediately after mapping.

**Usage example (future milestones):**
```rust
// Map a 4 KiB MMIO device register window:
unsafe { crate::mm::map_mmio(0xFEE0_0000, 0xFFFF_8000_FEE0_0000, 0x1000); }
```

**Confirmed serial output (✅ verified):**
```
[OSCORTEX] arch::early_init done
[MM::Paging] Virtual memory manager online (CR3=0x000000001ff85000)
```

Full boot to Cortex idle loop remains clean — no regressions.

**Multi-arch build status:**
| Target | Build | QEMU runnable |
|--------|-------|---------------|
| `x86_64-unknown-none` | ✅ clean | ✅ runs (ISO + USB boot) |
| `aarch64-unknown-none` | ✅ clean | stubs only — `early_init` is `todo!()` |
| `riscv64gc-unknown-none-elf` | ✅ clean | stubs only — `early_init` is `todo!()` |

---

## [Milestone 3] — x2APIC + Timer Interrupts

### Summary
Replaced the MMIO-based xAPIC implementation with a fully MSR-based **x2APIC** driver.  
This eliminated the need for a post-`mm::init` MMIO page-table mapping and unblocked APIC
initialisation at `early_init` time. Hardware interrupts are now enabled and the APIC
periodic timer fires on vector `0x30` (~10 ms), incrementing the scheduler tick counter.
Panic messages now show their full formatted text.

### Files changed

| File | Change |
|------|--------|
| [kernel/src/arch/x86_64/apic.rs](kernel/src/arch/x86_64/apic.rs) | Full rewrite: MMIO xAPIC → x2APIC via `rdmsr`/`wrmsr`. No MMIO mapping needed. Detects x2APIC via CPUID leaf 1 ECX[21]. `init_bsp()` enables x2APIC (IA32_APIC_BASE bits 10+11), programs spurious vector, ÷16 periodic timer (vector `0x30`). `eoi()` guarded by `X2APIC_READY` atomic. |
| [kernel/src/arch/x86_64/mod.rs](kernel/src/arch/x86_64/mod.rs) | Re-enabled `apic::init_bsp()` in `early_init` (was skipped with TODO). Removed stale comment about needing post-MM mapping. |
| [kernel/src/cortex/mod.rs](kernel/src/cortex/mod.rs) | `run()` now calls `crate::arch::enable_interrupts()` (`sti`) before entering the idle loop so APIC timer and other vectors can fire. |
| [kernel/src/panic.rs](kernel/src/panic.rs) | Replaced `info.message().as_str().unwrap_or("(no message)")` with a stack-allocated `PanicBuf` (512 bytes) that formats the message via `core::fmt::Write`. All `panic!()` calls with format arguments now display their full message over serial and framebuffer. |
| [scripts/build-iso.sh](scripts/build-iso.sh) | Added `-cpu qemu64,+x2apic` flag to the `--run` QEMU invocation. QEMU's default CPU does not advertise x2APIC; the flag is required. |
| [tools/xtask/src/main.rs](tools/xtask/src/main.rs) | Added `-cpu qemu64,+x2apic` to the `run()` QEMU args for the same reason. |

### Details

**x2APIC MSR mapping used:**
```
xAPIC offset  →  x2APIC MSR   (formula: 0x800 + offset/0x10)
    0xB0      →   0x80B   EOI (write-only)
    0xF0      →   0x80F   Spurious Interrupt Vector
    0x320     →   0x832   LVT Timer
    0x380     →   0x838   Initial Count
    0x3E0     →   0x83E   Divide Configuration
```

**Timer configuration:** ÷16 divider, periodic mode, vector `0x30`, initial count `0x10_0000`
(≈ 10 ms at 2 GHz). Fires `idt::apic_timer_handler` → `sched::tick()` → tick counter +1.

**QEMU command to test (requires `+x2apic` CPU flag):**
```
bash scripts/build-iso.sh && \
gtimeout 6 qemu-system-x86_64 -cdrom oscortex.iso -cpu qemu64,+x2apic \
  -m 512M -smp 1 -serial file:/tmp/ss.txt -display none && \
cat /tmp/ss.txt
```

**Confirmed serial output (✅ verified):**
```
[ARCH] apic::init_bsp
[APIC] x2APIC online — periodic timer armed (vector 0x30)
[ARCH] cpu::enable_fpu_simd
[ARCH] syscall::init
[ARCH] early_init complete
...
[INFO] kernel: OSCortex kernel init complete — entering Cortex-managed loop
```

> **Note:** QEMU's default CPU does not expose x2APIC; you must pass `-cpu qemu64,+x2apic`
> (already added to `scripts/build-iso.sh --run` and `tools/xtask/src/main.rs`).

---

## [Milestone 2] — Framebuffer Text Console

### Summary
Implemented an 8×8 bitmap font framebuffer console, giving the kernel a visual text output
channel in addition to COM1 serial. The logger was rewritten to write to both outputs
simultaneously. Limine reports a 1280×800 32bpp framebuffer at virtual `0xffff8000fd000000`.

### Files changed

| File | Change |
|------|--------|
| [kernel/src/drivers/fb.rs](kernel/src/drivers/fb.rs) | **New file.** 8×8 IBM VGA font (96 glyphs, `0x20`–`0x7F`). Atomic statics for FB address, pitch, dimensions, cursor position. `init()` stores FB parameters and clears screen. `write_str()` iterates chars, calls `blit_char()` for each, handles `\n` and scroll. `scroll_up()` uses `core::ptr::copy` + row clear. `blit_char()` uses `write_volatile`. |
| [kernel/src/drivers/mod.rs](kernel/src/drivers/mod.rs) | Added `pub mod fb;` to expose the new driver. |
| [kernel/src/logger.rs](kernel/src/logger.rs) | Full rewrite. Added stack-allocated `FmtBuf` (512 bytes) for formatting without heap. `KernelLogger::log()` writes formatted string to both `SERIAL` and `crate::drivers::fb::write_str()`. `init()` accepts `Option<&FramebufferResponse>` and calls `fb::init()` if present. |
| [kernel/src/main.rs](kernel/src/main.rs) | Passes `FB_REQUEST.response()` to `logger::init()`. Logs framebuffer geometry (`info!`) after init for diagnostic confirmation. |

### Details

**Font**: 96-entry public-domain 8×8 bitmap covering ASCII `0x20`–`0x7F`.  
**Colours**: foreground `0x00FF_FFFF` (white), background `0x0000_0000` (black), 32bpp XRGB.  
**Confirmed framebuffer**: `1280×800 bpp=32 pitch=5120 addr=0xffff8000fd000000`

---

## [Milestone 1] — Kernel Boots + Serial Console

### Summary
Established the complete build pipeline and verified the kernel reaches `kernel_main`,
initialises all architecture subsystems, and prints structured log output over COM1 serial.

### Files changed

| File | Change |
|------|--------|
| [.cargo/config.toml](.cargo/config.toml) | Added `rustflags = ["-C", "relocation-model=static"]` for `x86_64-unknown-none`. Without this, Cargo produces an ET_DYN ELF which Limine refuses to load. |
| [iso_root/limine.conf](iso_root/limine.conf) | Created Limine config using correct `limine.conf` filename (not `limine.cfg`). Includes `serial: yes`, `verbose: yes`, `protocol: limine`, `path: boot():/boot/kernel`, `kaslr: no`. |
| [kernel/src/logger.rs](kernel/src/logger.rs) | Initial logger: COM1 UART (115 200 baud 8N1), `early_print` for pre-init output, `log` crate integration. |
| [kernel/src/arch/x86_64/cpu.rs](kernel/src/arch/x86_64/cpu.rs) | `assert_required_features` checks SSE + SSE2 only (SSE3/4.x removed — default QEMU CPU lacks them). Exports `cpuid()` helper. |
| [kernel/src/mm/frame_allocator.rs](kernel/src/mm/frame_allocator.rs) | Added `HHDM_OFFSET: AtomicU64`, `set_hhdm_offset()`, `hhdm_offset()`. HHDM is set before `arch::early_init()` so the frame allocator can convert physical↔virtual from the very first interrupt. |
| [kernel/src/main.rs](kernel/src/main.rs) | Boot sequence: `early_print` → set HHDM → `arch::early_init` → `mm::init` → `logger::init` → subsystems → `cortex::run`. |
| [scripts/build-iso.sh](scripts/build-iso.sh) | Build script: compiles kernel ELF, creates ISO with Limine, places kernel at `/boot/kernel`. |

### Details

**HHDM offset**: `0xffff800000000000`  
**Kernel virtual entry**: `0xffffffff800003b0` (ET_EXEC ELF)  
**Limine version**: 12.2.0 at `/opt/homebrew/share/limine/`  
**QEMU**: 11.0.0, `-m 512M -smp 1 -serial file:/tmp/ss.txt -display none`

**Final serial log tail (confirmed working):**
```
[INFO ] kernel: OSCortex kernel init complete — entering Cortex-managed loop
```

