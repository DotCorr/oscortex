import 'package:flutter/material.dart';
import 'dart:async';
import 'dart:convert';
import 'package:flutter/services.dart';

import 'package:flutter/widget_previews.dart';

void main() {
  runApp(const OSCortexApp());
}

@Preview(name: 'Launcher Preview')
Widget launcherPreview() {
  return OSCortexApp();
}

class OSCortexApp extends StatelessWidget {
  const OSCortexApp({super.key});

  @override
  Widget build(BuildContext context) {
    return MaterialApp(
      title: 'OSCortex v1',
      debugShowCheckedModeBanner: false,
      home: const DesktopShell(),
    );
  }
}

class AppDef {
  const AppDef({
    required this.id,
    required this.title,
    required this.bundlePath,
    required this.color,
    required this.icon,
  });

  final String id;
  final String title;
  final String bundlePath;
  final Color color;
  final IconData icon;
}

class DesktopWindow {
  DesktopWindow({
    required this.pid,
    required this.app,
    required this.x,
    required this.y,
    required this.width,
    required this.height,
    required this.z,
  });

  final int pid;
  final AppDef app;
  double x;
  double y;
  double width;
  double height;
  int z;
}

class DesktopShell extends StatefulWidget {
  const DesktopShell({super.key});

  @override
  State<DesktopShell> createState() => _DesktopShellState();
}

class _DesktopShellState extends State<DesktopShell> {
  static const BasicMessageChannel<String> _appsCatalogChannel = BasicMessageChannel<String>('oscortex/apps/catalog', StringCodec());
  static const BasicMessageChannel<String> _appsRequestChannel = BasicMessageChannel<String>('oscortex/apps/request', StringCodec());

  List<AppDef> _apps = <AppDef>[];
  bool _appsReady = false;

  final List<DesktopWindow> _windows = <DesktopWindow>[];
  int _nextPid = 120;
  int _nextZ = 1;

  Offset _cursor = const Offset(180, 120);
  Offset? _dragStart;
  DesktopWindow? _dragWindow;

  late final Timer _clockTick;
  DateTime _now = DateTime.now();

  @override
  void initState() {
    super.initState();
    _appsCatalogChannel.setMessageHandler(_onCatalogMessage);
    _requestSubsystemApps();
    _loadCatalogFallbackFromAssets();
    _clockTick = Timer.periodic(const Duration(seconds: 1), (_) {
      if (!mounted) return;
      setState(() {
        _now = DateTime.now();
      });
    });
  }

  @override
  void dispose() {
    _appsCatalogChannel.setMessageHandler(null);
    _clockTick.cancel();
    super.dispose();
  }

  Future<String> _onCatalogMessage(String? jsonText) async {
    if (jsonText == null || jsonText.isEmpty) {
      return '';
    }
    _ingestCatalogJson(jsonText);
    return '';
  }

  Future<void> _requestSubsystemApps() async {
    try {
      await _appsRequestChannel.send('list');
    } catch (_) {}
  }

  Future<void> _loadCatalogFallbackFromAssets() async {
    try {
      final String raw = await rootBundle.loadString('system_apps_registry.json');
      if (!_appsReady) {
        _ingestCatalogJson(raw);
      }
    } catch (_) {
      if (!_appsReady && mounted) {
        setState(() {
          _appsReady = true;
          _apps = <AppDef>[];
        });
      }
    }
  }

  void _ingestCatalogJson(String raw) {
    try {
      final dynamic parsed = jsonDecode(raw);
      final List<dynamic> rows = (parsed is Map<String, dynamic> ? parsed['apps'] : null) as List<dynamic>? ?? <dynamic>[];
      final List<AppDef> next = rows
          .whereType<Map<String, dynamic>>()
          .map(_fromCatalogEntry)
          .toList(growable: false);

      if (!mounted) return;
      setState(() {
        _appsReady = true;
        _apps = next;
      });
    } catch (_) {
      if (!mounted) return;
      setState(() {
        _appsReady = true;
      });
    }
  }

  AppDef _fromCatalogEntry(Map<String, dynamic> e) {
    final String id = (e['id'] as String?)?.trim().isNotEmpty == true ? (e['id'] as String).trim() : 'unknown';
    final String title = (e['title'] as String?)?.trim().isNotEmpty == true ? (e['title'] as String).trim() : id;
    final String bundlePath = (e['bundle_path'] as String?)?.trim().isNotEmpty == true ? (e['bundle_path'] as String).trim() : '/system/apps/$id/flutter_assets';

    final int seed = id.codeUnits.fold<int>(0, (int a, int b) => (a * 31 + b) & 0x7fffffff);
    final List<Color> palette = <Color>[
      const Color(0xFF1F7A8C),
      const Color(0xFF4B7F52),
      const Color(0xFF4B556D),
      const Color(0xFF8A5A44),
      const Color(0xFF6A4B7C),
      const Color(0xFF8F6C1A),
    ];
    final List<IconData> icons = <IconData>[
      Icons.apps,
      Icons.terminal,
      Icons.folder,
      Icons.code,
      Icons.web,
      Icons.memory,
    ];

    return AppDef(
      id: id,
      title: title,
      bundlePath: bundlePath,
      color: palette[seed % palette.length],
      icon: icons[seed % icons.length],
    );
  }

  void _launchApp(AppDef app) {
    setState(() {
      _windows.add(
        DesktopWindow(
          pid: _nextPid++,
          app: app,
          x: 120 + (_windows.length * 24),
          y: 110 + (_windows.length * 18),
          width: 560,
          height: 360,
          z: _nextZ++,
        ),
      );
    });
  }

  void _focus(DesktopWindow win) {
    setState(() {
      win.z = _nextZ++;
    });
  }

  void _close(DesktopWindow win) {
    setState(() {
      _windows.remove(win);
    });
  }

  @override
  Widget build(BuildContext context) {
    final List<DesktopWindow> ordered = List<DesktopWindow>.from(_windows)
      ..sort((a, b) => a.z.compareTo(b.z));

    return Scaffold(
      body: MouseRegion(
        cursor: SystemMouseCursors.none,
        onHover: (e) => setState(() => _cursor = e.position),
        child: Listener(
          onPointerMove: (e) => setState(() => _cursor = e.position),
          onPointerDown: (e) => setState(() => _cursor = e.position),
          behavior: HitTestBehavior.opaque,
          child: Stack(
            children: <Widget>[
              const _AnimatedBackdrop(),
              Positioned(
                left: 24,
                right: 24,
                top: 18,
                child: _TopBar(now: _now, running: _windows.length),
              ),
              Positioned(
                left: 28,
                top: 96,
                child: _LauncherGrid(apps: _apps, appsReady: _appsReady, onLaunch: _launchApp),
              ),
              for (final DesktopWindow win in ordered)
                _WindowView(
                  key: ValueKey<int>(win.pid),
                  win: win,
                  onFocus: () => _focus(win),
                  onClose: () => _close(win),
                  onDragStart: (Offset global) {
                    _focus(win);
                    _dragStart = global;
                    _dragWindow = win;
                  },
                  onDragUpdate: (Offset global) {
                    if (_dragWindow != win || _dragStart == null) return;
                    final Offset d = global - _dragStart!;
                    setState(() {
                      win.x += d.dx;
                      win.y += d.dy;
                      _dragStart = global;
                    });
                  },
                  onDragEnd: () {
                    _dragStart = null;
                    _dragWindow = null;
                  },
                ),
              Positioned(
                left: 0,
                right: 0,
                bottom: 22,
                child: _Dock(apps: _apps, appsReady: _appsReady, onLaunch: _launchApp),
              ),
              Positioned(
                left: _cursor.dx,
                top: _cursor.dy,
                child: const IgnorePointer(child: _SoftCursor()),
              ),
            ],
          ),
        ),
      ),
    );
  }
}

class _AnimatedBackdrop extends StatelessWidget {
  const _AnimatedBackdrop();

  @override
  Widget build(BuildContext context) {
    return TweenAnimationBuilder<double>(
      tween: Tween<double>(begin: 0, end: 1),
      duration: const Duration(seconds: 7),
      curve: Curves.easeInOut,
      builder: (BuildContext context, double t, Widget? child) {
        return DecoratedBox(
          decoration: BoxDecoration(
            gradient: LinearGradient(
              begin: Alignment(-1 + t, -1),
              end: Alignment(1, 1 - t),
              colors: const <Color>[
                Color(0xFF0C1C26),
                Color(0xFF15354A),
                Color(0xFF1F5D5F),
              ],
            ),
          ),
          child: const SizedBox.expand(),
        );
      },
    );
  }
}

class _TopBar extends StatelessWidget {
  const _TopBar({required this.now, required this.running});

  final DateTime now;
  final int running;

  @override
  Widget build(BuildContext context) {
    final String hh = now.hour.toString().padLeft(2, '0');
    final String mm = now.minute.toString().padLeft(2, '0');
    final String ss = now.second.toString().padLeft(2, '0');

    return Container(
      padding: const EdgeInsets.symmetric(horizontal: 16, vertical: 10),
      decoration: BoxDecoration(
        color: const Color(0xAA0A1118),
        borderRadius: BorderRadius.circular(14),
        border: Border.all(color: const Color(0x446CC9D8)),
      ),
      child: Row(
        children: <Widget>[
          const Icon(Icons.memory, color: Color(0xFF89E2F3)),
          const SizedBox(width: 10),
          const Text('OSCortex Shell', style: TextStyle(color: Colors.white, fontWeight: FontWeight.w600)),
          const Spacer(),
          Text('apps: $running', style: const TextStyle(color: Color(0xFFC9D6DD))),
          const SizedBox(width: 20),
          Text('$hh:$mm:$ss', style: const TextStyle(color: Colors.white)),
        ],
      ),
    );
  }
}

class _LauncherGrid extends StatelessWidget {
  const _LauncherGrid({required this.apps, required this.appsReady, required this.onLaunch});

  final List<AppDef> apps;
  final bool appsReady;
  final ValueChanged<AppDef> onLaunch;

  @override
  Widget build(BuildContext context) {
    return SizedBox(
      width: 560,
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: <Widget>[
          const Text('Installed Apps', style: TextStyle(color: Color(0xFFD8E8EE), fontWeight: FontWeight.w600, fontSize: 14)),
          const SizedBox(height: 10),
          if (!appsReady)
            const Text('Loading subsystem app registry...', style: TextStyle(color: Color(0xFFB0C4CD)))
          else if (apps.isEmpty)
            const Text('No installed subsystem Flutter apps found.', style: TextStyle(color: Color(0xFFB0C4CD)))
          else
            Wrap(
              spacing: 16,
              runSpacing: 16,
              children: apps
                  .map(
                    (AppDef app) => GestureDetector(
                      onDoubleTap: () => onLaunch(app),
                      child: Container(
                        width: 140,
                        padding: const EdgeInsets.symmetric(vertical: 14, horizontal: 10),
                        decoration: BoxDecoration(
                          color: const Color(0xAA0B151D),
                          borderRadius: BorderRadius.circular(14),
                          border: Border.all(color: app.color.withOpacity(0.6)),
                        ),
                        child: Column(
                          mainAxisSize: MainAxisSize.min,
                          children: <Widget>[
                            Icon(app.icon, color: app.color, size: 28),
                            const SizedBox(height: 8),
                            Text(app.title, style: const TextStyle(color: Colors.white, fontSize: 13), textAlign: TextAlign.center),
                            const SizedBox(height: 4),
                            Text(app.id, style: const TextStyle(color: Color(0xFF88A3AE), fontSize: 11), textAlign: TextAlign.center),
                          ],
                        ),
                      ),
                    ),
                  )
                  .toList(),
            ),
        ],
      ),
    );
  }
}

class _Dock extends StatelessWidget {
  const _Dock({required this.apps, required this.appsReady, required this.onLaunch});

  final List<AppDef> apps;
  final bool appsReady;
  final ValueChanged<AppDef> onLaunch;

  @override
  Widget build(BuildContext context) {
    return Center(
      child: Container(
        padding: const EdgeInsets.symmetric(horizontal: 12, vertical: 10),
        decoration: BoxDecoration(
          color: const Color(0xAA081017),
          borderRadius: BorderRadius.circular(16),
          border: Border.all(color: const Color(0x4479B8C8)),
        ),
        child: Row(
          mainAxisSize: MainAxisSize.min,
          children: (appsReady ? apps : const <AppDef>[])
              .map(
                (AppDef app) => Padding(
                  padding: const EdgeInsets.symmetric(horizontal: 6),
                  child: IconButton(
                    onPressed: () => onLaunch(app),
                    icon: Icon(app.icon, color: app.color),
                    tooltip: 'Launch ${app.title}',
                  ),
                ),
              )
              .toList(),
        ),
      ),
    );
  }
}

class _WindowView extends StatelessWidget {
  const _WindowView({
    super.key,
    required this.win,
    required this.onFocus,
    required this.onClose,
    required this.onDragStart,
    required this.onDragUpdate,
    required this.onDragEnd,
  });

  final DesktopWindow win;
  final VoidCallback onFocus;
  final VoidCallback onClose;
  final ValueChanged<Offset> onDragStart;
  final ValueChanged<Offset> onDragUpdate;
  final VoidCallback onDragEnd;

  @override
  Widget build(BuildContext context) {
    return Positioned(
      left: win.x,
      top: win.y,
      width: win.width,
      height: win.height,
      child: GestureDetector(
        onTap: onFocus,
        child: Container(
          decoration: BoxDecoration(
            color: const Color(0xFF0E1822),
            borderRadius: BorderRadius.circular(12),
            border: Border.all(color: win.app.color.withOpacity(0.75), width: 1.2),
            boxShadow: const <BoxShadow>[BoxShadow(color: Color(0x55000000), blurRadius: 18, offset: Offset(0, 8))],
          ),
          child: Column(
            children: <Widget>[
              GestureDetector(
                behavior: HitTestBehavior.opaque,
                onPanStart: (DragStartDetails d) => onDragStart(d.globalPosition),
                onPanUpdate: (DragUpdateDetails d) => onDragUpdate(d.globalPosition),
                onPanEnd: (_) => onDragEnd(),
                child: Container(
                  height: 40,
                  padding: const EdgeInsets.symmetric(horizontal: 12),
                  decoration: BoxDecoration(
                    color: win.app.color.withOpacity(0.2),
                    borderRadius: const BorderRadius.vertical(top: Radius.circular(12)),
                  ),
                  child: Row(
                    children: <Widget>[
                      Icon(win.app.icon, size: 17, color: win.app.color),
                      const SizedBox(width: 8),
                      Expanded(
                        child: Text(
                          '${win.app.title}  (pid ${win.pid})',
                          style: const TextStyle(color: Colors.white, fontSize: 13),
                          overflow: TextOverflow.ellipsis,
                        ),
                      ),
                      IconButton(
                        onPressed: onClose,
                        icon: const Icon(Icons.close, size: 16, color: Color(0xFFEBC7C7)),
                      ),
                    ],
                  ),
                ),
              ),
              Expanded(
                child: Padding(
                  padding: const EdgeInsets.all(14),
                  child: Align(
                    alignment: Alignment.topLeft,
                    child: Text(
                      'OSCortex App Process\n\n'
                      'title: ${win.app.title}\n'
                      'app id: ${win.app.id}\n'
                      'bundle: ${win.app.bundlePath}\n'
                      'pid: ${win.pid}\n'
                      'surface owner: flutter shell\n'
                      'input: pointer+keyboard\n\n'
                      'This window is managed by oscortex_app UI.',
                      style: const TextStyle(color: Color(0xFFD6E3EA), height: 1.35),
                    ),
                  ),
                ),
              ),
            ],
          ),
        ),
      ),
    );
  }
}

class _SoftCursor extends StatelessWidget {
  const _SoftCursor();

  @override
  Widget build(BuildContext context) {
    return SizedBox(
      width: 18,
      height: 24,
      child: CustomPaint(
        painter: _CursorPainter(),
      ),
    );
  }
}

class _CursorPainter extends CustomPainter {
  @override
  void paint(Canvas canvas, Size size) {
    final Path p = Path()
      ..moveTo(1, 1)
      ..lineTo(1, 21)
      ..lineTo(6.5, 16)
      ..lineTo(11, 23)
      ..lineTo(13.5, 21.5)
      ..lineTo(9, 14)
      ..lineTo(16, 14)
      ..close();

    canvas.drawShadow(p, Colors.black, 2.2, false);
    canvas.drawPath(
      p,
      Paint()..color = const Color(0xFFFFFFFF),
    );
    canvas.drawPath(
      p,
      Paint()
        ..style = PaintingStyle.stroke
        ..strokeWidth = 1.4
        ..color = const Color(0xFF0A1016),
    );
  }

  @override
  bool shouldRepaint(covariant CustomPainter oldDelegate) => false;
}

