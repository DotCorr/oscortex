# OSCortex Web Browser — Architecture & Plan

Status: **Phase 0 (foundation + scaffold).** The browser is the project's single
largest piece — realistically multiple person-months even reusing maximally. This
doc is the grounded plan from two research passes; it is the authoritative spec
for the effort.

## Decision: Servo, not Chromium

The directive was "embed prebuilt Chromium so we don't rebuild." That premise does
not survive contact with a microkernel, and the alternatives were evaluated:

- **Prebuilt Chromium/CEF — impossible.** The Linux binaries are glibc-linked,
  `dlopen` ~20+ desktop libraries (NSS, dbus, fontconfig, Mesa/GBM, …), require an
  Ozone X11/Wayland display backend, and assume Chromium's multi-process + zygote +
  seccomp-bpf sandbox model. OSCortex has none of that; the binary fails at
  relocation before any code runs. "Don't rebuild" is the part that cannot work.
- **Chromium from source — not solo-feasible.** Requires a custom Ozone backend +
  satisfying/stubbing the whole process/sandbox/GPU-process model. Google brought
  Chromium to Fuchsia (their own from-scratch OS) with a first-party team over
  ~3 years, and even then it is an embedded WebView, not a desktop browser.
  Honest solo estimate: 3–5+ person-years, not realistically completable alone.
- **Cobalt/Starboard — wrong ceiling.** Most designed-for-porting (Blink-lineage,
  BSD, POSIX-friendly), but it is a YouTube/TV-class HTML5 *app* runtime, not a
  browser — it cannot browse arbitrary sites.
- **WPEWebKit — full but heavy.** A full WebKit engine that renders real sites
  today, but porting onto a glibc-less microkernel drags in a heavy C/C++ tree
  (ICU, EGL, GStreamer) + a custom libwpe backend. ~1.5–3 person-years.
- **Servo (chosen).** Rust (matches the kernel + toolchain + FFI), embeds as a
  *single-process library* (`libservo`) — sidestepping Chromium's entire
  zygote/sandbox/multi-process burden — and now has a **CPU/software rendering
  path** (Vello CPU backend) suited to OSCortex's software-composited framebuffer.
  Renders real sites, though compat is still maturing (Servo is officially a v0.1
  "tech demo"). Smallest porting surface, best OS-fit. ~9–18 person-months to first
  real-site render solo; partial-but-improving compat thereafter.

## Architecture: web engine as a userspace service rendering into a compositor surface

The engine does **not** live in the kernel ("lower levels") — that would break the
microkernel model. It is a **userspace web-engine service** exposed to apps as a
native webview capability.

**Critical finding: Flutter's external-texture API is GPU-only.** `embedder.h`'s
`FlutterEngineRegisterExternalTexture` + `TextureFrameCallback` hand back a
`FlutterOpenGLTexture`; there is no software/pixel-buffer variant, and OSCortex
runs the software renderer — so external-texture registration is rejected by the
engine. We do **not** use it.

**Instead, the engine renders into its own OSCortex compositor surface.** OSCortex
already is a multi-surface compositor (`kernel/src/compositor/mod.rs`: per-surface
geometry, z-order, clip rect, visibility, owner-PID, `surface_at_point` hit-test).
That is the OSCortex equivalent of Android's `SurfaceProducer`/texture registry.

```
Web Link (Flutter app)                          web-engine service (libservo)
  ├─ chrome surface (z=N): address bar,           └─ renders page → RGBA →
  │  search, back/fwd, tabs                           SYS_GPU_SUBMIT_STRIDED(webSurfaceId)
  └─ OscWebView widget = transparent hit-rect
     over the web region; forwards input
                                                  web surface (z=N-1), clipped to
        oscortex/webview MethodChannel  <───────>  the viewport rect, composited
        (control + events)                         directly beneath the chrome by
                                                    the OSCortex compositor
```

- Pixel path: service calls `create_surface_for(owner=WebLink, w, h)` → renders →
  `gpu_submit_strided_for(surfaceId, rgba, row_bytes)`. Web Link places its chrome
  surface at z=N and the web surface at z=N−1, clipped to the viewport
  (`surface_clip_set_for`). No Flutter texture; no engine patch.
- Input: `OscWebView` wraps a `Listener` + `Focus`; hit-test in Flutter,
  `globalToLocal` transform to web-local coords, forward via
  `dispatchInput`/`dispatchScroll`/`dispatchKey`. (Kernel-routed input via
  `surface_at_point`+owner-PID is a later optimization.)

This mirrors webview_flutter's *texture mode* (off-screen surface + id + Texture
widget + manual gesture forwarding), substituting the OSCortex compositor surface
for the Flutter texture registry. It is **engine-agnostic**: the same interface
serves a stub today and Servo later.

## The `oscortex/webview` contract (MethodChannel, StandardMethodCodec)

Per-instance multiplexing via a `viewId` int in every call/event.

**Methods (app → engine):** `create{viewId,w,h,dpr,initialUrl?}→{surfaceId}`,
`destroy`, `loadUrl{url,headers?,method?,body?}`, `loadHtml{html,baseUrl?}`,
`reload`, `stopLoading`, `goBack`, `goForward`, `canGoBack→bool`,
`canGoForward→bool`, `currentUrl→String?`, `getTitle→String?`,
`evalJs{source}→Object?`, `resize{w,h,dpr}`, `setViewport{x,y,w,h,scrollDx,scrollDy}`,
`dispatchInput{type,x,y,button,modifiers}`, `dispatchScroll{x,y,dx,dy}`,
`dispatchKey{type,keyCode,modifiers,text?}`, `setVisible{visible}`.

**Events (engine → app):** `urlChanged`, `titleChanged`, `loadProgress{0..100}`,
`loadStarted`, `loadFinished`, `loadError{code,description,url}`,
`navState{canGoBack,canGoForward}`, `scrollChanged{x,y}`, `frameReady{surfaceId}`.

Input type/button/modifier enums reuse the existing `EV_POINTER`/`EV_KEY`/`EV_SCROLL`
vocabulary in `kernel/src/embedder/abi.rs`. Transport reuses the platform-channel
mailbox (`kernel/src/platform_channel/mod.rs`).

## Phased plan

Two tracks. Track B (scaffold) is engine-agnostic and de-risks the integration
before the expensive Track A (engine) investment; they proceed in parallel.

**Track B — integration scaffold (tractable, weeks):**
1. `packages/oscortex_webview/` — `OscWebViewController` (the channel API above) +
   `OscWebView` widget (placeholder + input forwarding) + unit tests. ← *first code*
2. Wire **Web Link** (`apps/oscortex_web_link`, today a pure mockup) into a real
   browser chrome: address bar → `loadUrl`, back/fwd buttons gated by `navState`,
   progress/title from events, the `OscWebView` over the web region.
3. Embedder handler for `oscortex/webview` (StandardMethodCodec; mirrors the
   mousecursor/clipboard handlers) routing to the web-engine service.
4. **Stub web-engine service** — a userspace process that creates the surface and
   renders a placeholder (URL text / solid fill) so the *entire pipeline*
   (app → channel → surface → composite → input → events) is provable end-to-end
   without Servo. This validates the design.

**Track A — Servo bring-up (the long pole, months):**
1. Cross-compile `libservo` for the OSCortex target with the **Vello CPU / software
   render path** + WebRender on a software-GL or CPU surface.
2. Implement the minimal embedder glue: window/surface = the OSCortex compositor
   surface; input = the forwarded events; reuse the timer/thread/epoll/fs/net shims
   the Flutter embedder already provides.
3. **Make-or-break milestone:** render a single static HTML page to the surface,
   headless, and diff against a reference PNG (the same way the Flutter shell is
   validated). Everything after is incremental web-compat + interactivity.
4. Replace the stub service with libservo behind the same `oscortex/webview`
   contract.

**Final step (already ready):** compile Web Link with the `osx` toolchain
(`osx build` → universal `.osx`) and install it as the system browser app. This is
the easy end — it works today; it is gated entirely on the engine underneath.

## Honest effort

Track B: weeks. Track A (Servo to first real-site render): ~9–18 person-months
solo; genuinely usable compat is multi-year and tracks upstream Servo's own
maturation. This is the largest single undertaking in the project.

## Sources

Feasibility: Chromium sysroot/glibc baseline & Linux deps, Ozone/GBM, Linux
sandbox/zygote ([chromium.googlesource.com](https://chromium.googlesource.com/chromium/src/+/main/docs/linux/build_instructions.md));
Cobalt/Starboard ([github.com/youtube/cobalt](https://github.com/youtube/cobalt));
Servo `libservo` + Vello CPU backend + servo-shot ([servo.org](https://servo.org/blog/2025/08/22/this-month-in-servo/));
WPEWebKit ([wpewebkit.org](https://wpewebkit.org/blog/02-overview-of-wpe.html));
Chromium-on-Fuchsia ([chromium.googlesource.com/.../fuchsia_web](https://chromium.googlesource.com/chromium/src/+/HEAD/fuchsia_web/)).
Integration: Flutter `embedder.h` external-texture (GPU-only) & compositor/platform-view
([github.com/flutter/engine](https://github.com/flutter/engine/blob/main/shell/platform/embedder/embedder.h));
webview_flutter texture-mode vs hybrid-composition & `SurfaceProducer`
([docs.flutter.dev](https://docs.flutter.dev/release/breaking-changes/android-surface-plugins)).
