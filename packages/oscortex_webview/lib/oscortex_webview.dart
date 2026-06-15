/// OSCortex native webview for userspace Flutter apps.
///
/// A userspace web-engine service (Servo) renders a page into its own OSCortex
/// compositor surface, which the kernel z-stacks directly beneath the app's chrome
/// and clips to the viewport. This package is the app-side half:
///
///  * [OscWebViewController] drives navigation and receives events over the
///    `oscortex/webview` [MethodChannel].
///  * [OscWebView] is a transparent placeholder widget that reserves the web
///    region, reports its on-screen rect (so the kernel can position + clip the
///    web surface), and forwards pointer/scroll/key input to the engine.
///
/// Flutter's external-texture API is GPU-only and OSCortex is software-rendered, so
/// there is deliberately no texture here — the web pixels are a sibling OS surface
/// showing through a transparent hole in the Flutter scene. The app must therefore
/// keep the [OscWebView] region transparent (don't paint an opaque background over
/// it) so the lower surface is visible.
///
/// The contract is engine-agnostic: a stub service or Servo serve it identically.
library oscortex_webview;

import 'dart:async';

import 'package:flutter/gestures.dart';
import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';

/// Pointer/key event `type` codes sent in [OscWebViewController.dispatchInput] /
/// [OscWebViewController.dispatchKey]. Mirror the kernel's EV_* input vocabulary.
class OscInput {
  OscInput._();
  static const int pointerDown = 0;
  static const int pointerMove = 1;
  static const int pointerUp = 2;
  static const int pointerHover = 3;
  static const int keyDown = 0;
  static const int keyUp = 1;
}

/// Drives one native webview instance over `oscortex/webview`.
///
/// Construct with a unique [viewId]; call [create] once the render size is known,
/// then [loadUrl] / [goBack] / etc. Assign the `on*` callbacks to observe engine
/// events. Call [dispose] to stop receiving events and free the registry slot.
class OscWebViewController {
  OscWebViewController(this.viewId) {
    _instances[viewId] = this;
    if (!_handlerInstalled) {
      _channel.setMethodCallHandler(_dispatch);
      _handlerInstalled = true;
    }
  }

  /// Per-instance id, included in every call/event for multiplexing.
  final int viewId;

  /// The channel name (shared by all instances; events are routed by `viewId`).
  static const String channelName = 'oscortex/webview';
  static const MethodChannel _channel = MethodChannel(channelName);

  static final Map<int, OscWebViewController> _instances =
      <int, OscWebViewController>{};
  static bool _handlerInstalled = false;

  // ── Engine → app event callbacks ────────────────────────────────────────────
  void Function(String url)? onUrlChanged;
  void Function(String title)? onTitleChanged;
  void Function(int progress)? onProgress; // 0..100
  void Function(String url)? onLoadStarted;
  void Function(String url)? onLoadFinished;
  void Function(int code, String description, String url)? onLoadError;
  void Function(bool canGoBack, bool canGoForward)? onNavState;
  void Function(int x, int y)? onScrollChanged;

  Map<String, dynamic> _a([Map<String, dynamic>? extra]) =>
      <String, dynamic>{'viewId': viewId, ...?extra};

  // ── App → engine methods ────────────────────────────────────────────────────

  /// Create the engine's render surface; returns the OSCortex `surfaceId` (or -1).
  Future<int> create({
    required int width,
    required int height,
    double dpr = 1.0,
    String? initialUrl,
  }) async {
    final Map<String, dynamic>? r =
        await _channel.invokeMapMethod<String, dynamic>(
      'create',
      _a(<String, dynamic>{
        'width': width,
        'height': height,
        'dpr': dpr,
        'initialUrl': initialUrl,
      }),
    );
    return (r?['surfaceId'] as int?) ?? -1;
  }

  Future<void> destroy() => _channel.invokeMethod<void>('destroy', _a());

  Future<void> loadUrl(String url, {Map<String, String>? headers}) =>
      _channel.invokeMethod<void>(
          'loadUrl', _a(<String, dynamic>{'url': url, 'headers': headers}));

  Future<void> loadHtml(String html, {String? baseUrl}) =>
      _channel.invokeMethod<void>(
          'loadHtml', _a(<String, dynamic>{'html': html, 'baseUrl': baseUrl}));

  Future<void> reload() => _channel.invokeMethod<void>('reload', _a());
  Future<void> stopLoading() => _channel.invokeMethod<void>('stopLoading', _a());
  Future<void> goBack() => _channel.invokeMethod<void>('goBack', _a());
  Future<void> goForward() => _channel.invokeMethod<void>('goForward', _a());

  Future<bool> canGoBack() async =>
      (await _channel.invokeMethod<bool>('canGoBack', _a())) ?? false;
  Future<bool> canGoForward() async =>
      (await _channel.invokeMethod<bool>('canGoForward', _a())) ?? false;

  Future<String?> currentUrl() =>
      _channel.invokeMethod<String>('currentUrl', _a());
  Future<String?> getTitle() => _channel.invokeMethod<String>('getTitle', _a());

  Future<Object?> evalJs(String source) =>
      _channel.invokeMethod<Object>('evalJs', _a(<String, dynamic>{'source': source}));

  Future<void> resize({required int width, required int height, double dpr = 1.0}) =>
      _channel.invokeMethod<void>('resize',
          _a(<String, dynamic>{'width': width, 'height': height, 'dpr': dpr}));

  /// Tell the engine/kernel where the webview rect lives on screen + its scroll
  /// offset, so the web surface can be positioned and clipped to it.
  Future<void> setViewport({
    required int x,
    required int y,
    required int w,
    required int h,
    int scrollDx = 0,
    int scrollDy = 0,
  }) =>
      _channel.invokeMethod<void>(
          'setViewport',
          _a(<String, dynamic>{
            'x': x,
            'y': y,
            'w': w,
            'h': h,
            'scrollDx': scrollDx,
            'scrollDy': scrollDy,
          }));

  /// Forward a pointer event in webview-local coordinates. [type] is an
  /// `OscInput.pointer*` code; [button] a button bitmask; [modifiers] a key-mod mask.
  Future<void> dispatchInput({
    required int type,
    required double x,
    required double y,
    int button = 0,
    int modifiers = 0,
  }) =>
      _channel.invokeMethod<void>(
          'dispatchInput',
          _a(<String, dynamic>{
            'type': type,
            'x': x,
            'y': y,
            'button': button,
            'modifiers': modifiers,
          }));

  Future<void> dispatchScroll({
    required double x,
    required double y,
    required double dx,
    required double dy,
  }) =>
      _channel.invokeMethod<void>('dispatchScroll',
          _a(<String, dynamic>{'x': x, 'y': y, 'dx': dx, 'dy': dy}));

  Future<void> dispatchKey({
    required int type,
    required int keyCode,
    int modifiers = 0,
    String? text,
  }) =>
      _channel.invokeMethod<void>(
          'dispatchKey',
          _a(<String, dynamic>{
            'type': type,
            'keyCode': keyCode,
            'modifiers': modifiers,
            'text': text,
          }));

  Future<void> setVisible(bool visible) => _channel.invokeMethod<void>(
      'setVisible', _a(<String, dynamic>{'visible': visible}));

  /// Stop receiving events for this instance.
  void dispose() {
    _instances.remove(viewId);
  }

  // ── Event routing (one channel handler fans out to instances by viewId) ──────
  static Future<dynamic> _dispatch(MethodCall call) async {
    final Map<String, dynamic> a =
        (call.arguments as Map?)?.cast<String, dynamic>() ??
            const <String, dynamic>{};
    _instances[a['viewId'] as int?]?._onEvent(call.method, a);
    return null;
  }

  void _onEvent(String method, Map<String, dynamic> a) {
    switch (method) {
      case 'urlChanged':
        onUrlChanged?.call(a['url'] as String? ?? '');
      case 'titleChanged':
        onTitleChanged?.call(a['title'] as String? ?? '');
      case 'loadProgress':
        onProgress?.call(a['progress'] as int? ?? 0);
      case 'loadStarted':
        onLoadStarted?.call(a['url'] as String? ?? '');
      case 'loadFinished':
        onLoadFinished?.call(a['url'] as String? ?? '');
      case 'loadError':
        onLoadError?.call(a['code'] as int? ?? 0,
            a['description'] as String? ?? '', a['url'] as String? ?? '');
      case 'navState':
        onNavState?.call(
            a['canGoBack'] as bool? ?? false, a['canGoForward'] as bool? ?? false);
      case 'scrollChanged':
        onScrollChanged?.call(a['x'] as int? ?? 0, a['y'] as int? ?? 0);
    }
  }
}

/// A transparent placeholder over the web region.
///
/// It paints nothing (the engine's compositor surface shows through beneath the
/// Flutter scene), reports its on-screen rect via [OscWebViewController.setViewport]
/// after layout, and forwards pointer/scroll input transformed to webview-local
/// coordinates. Keyboard text should be forwarded by the host via
/// [OscWebViewController.dispatchKey] while this view holds focus.
class OscWebView extends StatefulWidget {
  const OscWebView({super.key, required this.controller});

  final OscWebViewController controller;

  @override
  State<OscWebView> createState() => _OscWebViewState();
}

class _OscWebViewState extends State<OscWebView> {
  final GlobalKey _key = GlobalKey();
  Rect? _lastViewport;

  RenderBox? get _box =>
      _key.currentContext?.findRenderObject() as RenderBox?;

  Offset _toLocal(Offset global) => _box?.globalToLocal(global) ?? global;

  void _reportViewport() {
    final RenderBox? box = _box;
    if (box == null || !box.hasSize) return;
    final Offset origin = box.localToGlobal(Offset.zero);
    final Rect rect = origin & box.size;
    if (rect == _lastViewport) return; // only on change
    _lastViewport = rect;
    widget.controller.setViewport(
      x: origin.dx.round(),
      y: origin.dy.round(),
      w: box.size.width.round(),
      h: box.size.height.round(),
    );
  }

  @override
  Widget build(BuildContext context) {
    WidgetsBinding.instance
        .addPostFrameCallback((_) => _reportViewport());
    return Listener(
      key: _key,
      behavior: HitTestBehavior.opaque,
      onPointerDown: (PointerDownEvent e) {
        final Offset p = _toLocal(e.position);
        widget.controller.dispatchInput(
            type: OscInput.pointerDown, x: p.dx, y: p.dy, button: e.buttons);
      },
      onPointerMove: (PointerMoveEvent e) {
        final Offset p = _toLocal(e.position);
        widget.controller.dispatchInput(
            type: OscInput.pointerMove, x: p.dx, y: p.dy, button: e.buttons);
      },
      onPointerUp: (PointerUpEvent e) {
        final Offset p = _toLocal(e.position);
        widget.controller
            .dispatchInput(type: OscInput.pointerUp, x: p.dx, y: p.dy);
      },
      onPointerHover: (PointerHoverEvent e) {
        final Offset p = _toLocal(e.position);
        widget.controller
            .dispatchInput(type: OscInput.pointerHover, x: p.dx, y: p.dy);
      },
      onPointerSignal: (PointerSignalEvent e) {
        if (e is PointerScrollEvent) {
          final Offset p = _toLocal(e.position);
          widget.controller.dispatchScroll(
              x: p.dx, y: p.dy, dx: e.scrollDelta.dx, dy: e.scrollDelta.dy);
        }
      },
      // Transparent: the web surface shows through beneath the Flutter scene.
      child: const SizedBox.expand(),
    );
  }
}
