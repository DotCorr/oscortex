import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:oscortex_webview/oscortex_webview.dart';

/// Verifies the `oscortex/webview` MethodChannel contract from the app side:
/// methods send the right name/args, replies decode, engine→app events fire the
/// matching callbacks, and events route to the correct instance by viewId. The
/// engine is mocked, so these run without a device.
void main() {
  TestWidgetsFlutterBinding.ensureInitialized();
  const String name = 'oscortex/webview';
  const MethodChannel channel = MethodChannel(name);
  const StandardMethodCodec codec = StandardMethodCodec();
  final List<MethodCall> calls = <MethodCall>[];

  TestDefaultBinaryMessenger m() =>
      TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger;

  // Simulate an engine→app event by delivering an encoded MethodCall.
  Future<void> emit(String method, Map<String, dynamic> args) async {
    final ByteData data = codec.encodeMethodCall(MethodCall(method, args));
    await m().handlePlatformMessage(name, data, (_) {});
  }

  setUp(() {
    calls.clear();
    m().setMockMethodCallHandler(channel, (MethodCall c) async {
      calls.add(c);
      switch (c.method) {
        case 'create':
          return <String, dynamic>{'surfaceId': 42};
        case 'canGoBack':
          return true;
        case 'canGoForward':
          return false;
        case 'currentUrl':
          return 'https://example.com/';
        case 'getTitle':
          return 'Example';
        case 'evalJs':
          return 7;
        default:
          return null;
      }
    });
  });

  tearDown(() => m().setMockMethodCallHandler(channel, null));

  test('create sends size/dpr/url and returns the surfaceId', () async {
    final OscWebViewController c = OscWebViewController(1);
    final int sid =
        await c.create(width: 800, height: 600, dpr: 2.0, initialUrl: 'https://a.test/');
    expect(sid, 42);
    final MethodCall call = calls.single;
    expect(call.method, 'create');
    expect(call.arguments['viewId'], 1);
    expect(call.arguments['width'], 800);
    expect(call.arguments['height'], 600);
    expect(call.arguments['dpr'], 2.0);
    expect(call.arguments['initialUrl'], 'https://a.test/');
    c.dispose();
  });

  test('navigation methods send method + viewId', () async {
    final OscWebViewController c = OscWebViewController(1);
    await c.loadUrl('https://x.test/');
    await c.goBack();
    await c.goForward();
    await c.reload();
    expect(calls.map((MethodCall e) => e.method).toList(),
        <String>['loadUrl', 'goBack', 'goForward', 'reload']);
    expect(calls.first.arguments['url'], 'https://x.test/');
    expect(calls.every((MethodCall e) => e.arguments['viewId'] == 1), isTrue);
    c.dispose();
  });

  test('query methods round-trip their replies', () async {
    final OscWebViewController c = OscWebViewController(1);
    expect(await c.canGoBack(), true);
    expect(await c.canGoForward(), false);
    expect(await c.currentUrl(), 'https://example.com/');
    expect(await c.getTitle(), 'Example');
    expect(await c.evalJs('1+6'), 7);
    c.dispose();
  });

  test('dispatchInput forwards type/coords/button', () async {
    final OscWebViewController c = OscWebViewController(1);
    await c.dispatchInput(
        type: OscInput.pointerDown, x: 12.5, y: 34.0, button: 1);
    final MethodCall call = calls.single;
    expect(call.method, 'dispatchInput');
    expect(call.arguments['type'], OscInput.pointerDown);
    expect(call.arguments['x'], 12.5);
    expect(call.arguments['y'], 34.0);
    expect(call.arguments['button'], 1);
    c.dispose();
  });

  test('engine events fire the matching callbacks', () async {
    final OscWebViewController c = OscWebViewController(1);
    String? url;
    String? title;
    int? progress;
    bool? back;
    bool? fwd;
    c.onUrlChanged = (String u) => url = u;
    c.onTitleChanged = (String t) => title = t;
    c.onProgress = (int p) => progress = p;
    c.onNavState = (bool b, bool f) {
      back = b;
      fwd = f;
    };

    await emit('urlChanged', <String, dynamic>{'viewId': 1, 'url': 'https://e.test/'});
    await emit('titleChanged', <String, dynamic>{'viewId': 1, 'title': 'E'});
    await emit('loadProgress', <String, dynamic>{'viewId': 1, 'progress': 55});
    await emit('navState',
        <String, dynamic>{'viewId': 1, 'canGoBack': true, 'canGoForward': false});

    expect(url, 'https://e.test/');
    expect(title, 'E');
    expect(progress, 55);
    expect(back, true);
    expect(fwd, false);
    c.dispose();
  });

  test('events route to the correct instance by viewId', () async {
    final OscWebViewController c1 = OscWebViewController(1);
    final OscWebViewController c2 = OscWebViewController(2);
    String? u1;
    String? u2;
    c1.onUrlChanged = (String u) => u1 = u;
    c2.onUrlChanged = (String u) => u2 = u;

    await emit('urlChanged',
        <String, dynamic>{'viewId': 2, 'url': 'https://two.test/'});

    expect(u2, 'https://two.test/');
    expect(u1, isNull); // instance 1 untouched
    c1.dispose();
    c2.dispose();
  });
}
