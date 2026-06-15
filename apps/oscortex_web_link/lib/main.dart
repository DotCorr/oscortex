import 'package:flutter/material.dart';
import 'package:oscortex_ui/oscortex_ui.dart';
import 'package:oscortex_webview/oscortex_webview.dart';

void main() {
  runApp(const WebLinkApp());
}

class WebLinkApp extends StatelessWidget {
  const WebLinkApp({super.key});

  @override
  Widget build(BuildContext context) {
    return MaterialApp(
      debugShowCheckedModeBanner: false,
      theme: OscTheme.dark(accent: OscColors.green),
      home: const WebLinkHome(),
    );
  }
}

/// Web Link — the OSCortex system browser. Chrome (address/search bar, nav,
/// progress) wrapping the native [OscWebView], whose page is rendered by the
/// userspace web-engine service into an OSCortex compositor surface beneath this
/// scene. Until that engine service binds, navigation is inert and the web region
/// shows a placeholder (the controller tolerates a missing engine).
class WebLinkHome extends StatefulWidget {
  const WebLinkHome({super.key});

  @override
  State<WebLinkHome> createState() => _WebLinkHomeState();
}

class _WebLinkHomeState extends State<WebLinkHome> {
  static const String _homeUrl = 'https://dotcorr.com/';

  final OscWebViewController _web = OscWebViewController(1);
  final TextEditingController _address = TextEditingController(text: _homeUrl);
  final FocusNode _addressFocus = FocusNode();

  int _progress = 0;
  bool _loading = false;
  bool _canBack = false;
  bool _canForward = false;
  bool _engineReady = false; // a render surface exists
  bool _createRequested = false;

  @override
  void initState() {
    super.initState();
    _web.onUrlChanged = (String url) {
      if (!_addressFocus.hasFocus) _address.text = url;
    };
    // onTitleChanged is wired when tabs land (which display the page title).
    _web.onProgress = (int p) {
      setState(() => _progress = p);
    };
    _web.onLoadStarted = (String _) {
      setState(() => _loading = true);
    };
    _web.onLoadFinished = (String _) {
      setState(() {
        _loading = false;
        _progress = 100;
      });
    };
    _web.onLoadError = (int _, String _, String _) {
      setState(() => _loading = false);
    };
    _web.onNavState = (bool back, bool fwd) {
      setState(() {
        _canBack = back;
        _canForward = fwd;
      });
    };
  }

  @override
  void dispose() {
    _web.dispose();
    _address.dispose();
    _addressFocus.dispose();
    super.dispose();
  }

  /// Turn raw omnibox input into a URL: keep explicit schemes, assume https for
  /// bare hosts, otherwise treat it as a web search (the "search" wrapper).
  String _toUrl(String input) {
    final String s = input.trim();
    if (s.isEmpty) return s;
    if (RegExp(r'^[a-zA-Z][a-zA-Z0-9+.\-]*://').hasMatch(s)) return s;
    if (!s.contains(' ') && RegExp(r'^[^\s/]+\.[^\s/]+').hasMatch(s)) {
      return 'https://$s';
    }
    return 'https://duckduckgo.com/?q=${Uri.encodeQueryComponent(s)}';
  }

  void _navigate() {
    final String url = _toUrl(_address.text);
    if (url.isEmpty) return;
    _addressFocus.unfocus();
    setState(() {
      _loading = true;
      _progress = 0;
    });
    _web.loadUrl(url);
  }

  Future<void> _ensureSurface(Size size, double dpr) async {
    if (_createRequested || size.isEmpty) return;
    _createRequested = true;
    final int sid = await _web.create(
      width: (size.width * dpr).round(),
      height: (size.height * dpr).round(),
      dpr: dpr,
      initialUrl: _toUrl(_address.text),
    );
    if (mounted && sid >= 0) setState(() => _engineReady = true);
  }

  @override
  Widget build(BuildContext context) {
    final double dpr = MediaQuery.of(context).devicePixelRatio;
    return Scaffold(
      backgroundColor: OscColors.bodyBg,
      body: SafeArea(
        child: Column(
          children: <Widget>[
            _toolbar(),
            _progressBar(),
            Expanded(
              child: LayoutBuilder(
                builder: (BuildContext context, BoxConstraints c) {
                  final Size size = Size(c.maxWidth, c.maxHeight);
                  WidgetsBinding.instance.addPostFrameCallback(
                      (_) => _ensureSurface(size, dpr));
                  return _engineReady
                      ? OscWebView(controller: _web)
                      : _placeholder();
                },
              ),
            ),
          ],
        ),
      ),
    );
  }

  Widget _toolbar() {
    return Container(
      padding: const EdgeInsets.symmetric(horizontal: 12, vertical: 8),
      decoration: const BoxDecoration(
        color: OscColors.canvasBg,
        border: Border(bottom: BorderSide(color: OscColors.border)),
      ),
      child: Row(
        children: <Widget>[
          OscIconButton(
            icon: Icons.arrow_back_rounded,
            tooltip: 'Back',
            onPressed: _canBack ? _web.goBack : null,
          ),
          const SizedBox(width: 4),
          OscIconButton(
            icon: Icons.arrow_forward_rounded,
            tooltip: 'Forward',
            onPressed: _canForward ? _web.goForward : null,
          ),
          const SizedBox(width: 4),
          OscIconButton(
            icon: _loading ? Icons.close_rounded : Icons.refresh_rounded,
            tooltip: _loading ? 'Stop' : 'Reload',
            onPressed: _loading ? _web.stopLoading : _web.reload,
          ),
          const SizedBox(width: 10),
          Expanded(child: _addressBar()),
        ],
      ),
    );
  }

  Widget _addressBar() {
    return TextField(
      controller: _address,
      focusNode: _addressFocus,
      textInputAction: TextInputAction.go,
      onSubmitted: (_) => _navigate(),
      style: OscTypography.monoBody.copyWith(
        color: OscColors.textPrimary,
        fontSize: 13,
      ),
      decoration: InputDecoration(
        isDense: true,
        contentPadding:
            const EdgeInsets.symmetric(horizontal: 14, vertical: 11),
        hintText: 'Search or enter address',
        prefixIcon: const Icon(Icons.travel_explore_rounded, size: 16),
        suffixIcon: IconButton(
          icon: const Icon(Icons.subdirectory_arrow_left_rounded, size: 16),
          color: OscColors.textMuted,
          tooltip: 'Go',
          onPressed: _navigate,
        ),
        filled: true,
        fillColor: OscColors.surface,
        border: OutlineInputBorder(
          borderRadius: BorderRadius.circular(10),
          borderSide: const BorderSide(color: OscColors.border),
        ),
        enabledBorder: OutlineInputBorder(
          borderRadius: BorderRadius.circular(10),
          borderSide: const BorderSide(color: OscColors.border),
        ),
        focusedBorder: OutlineInputBorder(
          borderRadius: BorderRadius.circular(10),
          borderSide: const BorderSide(color: OscColors.green),
        ),
      ),
    );
  }

  Widget _progressBar() {
    // A 2px accent line while loading; invisible otherwise (keeps layout stable).
    return SizedBox(
      height: 2,
      child: (_loading && _progress > 0 && _progress < 100)
          ? LinearProgressIndicator(
              value: _progress / 100.0,
              minHeight: 2,
              backgroundColor: Colors.transparent,
              valueColor:
                  const AlwaysStoppedAnimation<Color>(OscColors.green),
            )
          : const SizedBox.shrink(),
    );
  }

  Widget _placeholder() {
    // Shown until the web-engine service binds and a render surface exists.
    return DecoratedBox(
      decoration: BoxDecoration(
        gradient: RadialGradient(
          center: const Alignment(-0.4, -0.7),
          radius: 1.3,
          colors: <Color>[
            OscColors.green.withValues(alpha: 0.05),
            OscColors.bodyBg.withValues(alpha: 0.0),
          ],
          stops: const <double>[0.0, 0.7],
        ),
      ),
      child: Center(
        child: Column(
          mainAxisSize: MainAxisSize.min,
          children: <Widget>[
            const Icon(Icons.public_rounded, size: 44, color: OscColors.textDim),
            const SizedBox(height: 16),
            Text(
              'Web Link',
              style: OscTypography.heading2.copyWith(
                color: OscColors.textPrimary,
                letterSpacing: -0.4,
              ),
            ),
            const SizedBox(height: 8),
            Row(
              mainAxisSize: MainAxisSize.min,
              children: <Widget>[
                Container(
                  width: 6,
                  height: 6,
                  decoration: const BoxDecoration(
                    color: OscColors.amber,
                    shape: BoxShape.circle,
                  ),
                ),
                const SizedBox(width: 8),
                Text(
                  'web engine starting…',
                  style: OscTypography.caption.copyWith(
                    color: OscColors.textMuted,
                    letterSpacing: 0.2,
                  ),
                ),
              ],
            ),
          ],
        ),
      ),
    );
  }
}

// ─── Previews ──────────────────────────────────────────────────────────

@OscPreview(name: 'Web Link App', group: 'Apps')
Widget webLinkAppPreview() {
  return const WebLinkApp();
}

@OscPreview(name: 'Web Link Home', group: 'Apps')
Widget webLinkHomePreview() {
  return const WebLinkHome();
}
