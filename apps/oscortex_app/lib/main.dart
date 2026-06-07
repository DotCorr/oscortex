import 'dart:convert';

import 'package:flutter/material.dart';
import 'package:flutter/services.dart';

const _shell = BasicMessageChannel<String>('oscortex/shell', StringCodec());

const _bg = Color(0xFF0C1C26);
const _accent = Color(0xFF2DD4BF);

void main() {
  WidgetsFlutterBinding.ensureInitialized();
  runApp(const OscortexShellApp());
}

class OscortexShellApp extends StatelessWidget {
  const OscortexShellApp({super.key});

  @override
  Widget build(BuildContext context) {
    return MaterialApp(
      debugShowCheckedModeBanner: false,
      theme: ThemeData(
        brightness: Brightness.dark,
        scaffoldBackgroundColor: _bg,
        colorScheme: const ColorScheme.dark(
          primary: _accent,
          surface: _bg,
        ),
        useMaterial3: true,
        // CRITICAL on bare metal: there is NO system font provider and the bundle
        // ships no default/Roboto family, so Material's default Text font fallback
        // livelocks the first frame. Pin every text style to the bundled NotoSans.
        fontFamily: 'NotoSans',
      ),
      home: const ShellDesktop(),
    );
  }
}

class ShellDesktop extends StatefulWidget {
  const ShellDesktop({super.key});

  @override
  State<ShellDesktop> createState() => _ShellDesktopState();
}

class _ShellDesktopState extends State<ShellDesktop> {
  List<_AppTile> _apps = const [];
  String? _error;
  bool _loading = false;
  _AppTile? _launching;

  @override
  void initState() {
    super.initState();
    _refreshApps();
  }

  Future<void> _refreshApps({bool showSpinner = false}) async {
    if (showSpinner) {
      setState(() {
        _loading = true;
        _error = null;
      });
    }
    try {
      final raw = await _shell.send('list');
      final decoded = jsonDecode(raw ?? '{"apps":[]}') as Map<String, dynamic>;
      final items = (decoded['apps'] as List<dynamic>? ?? [])
          .map((e) => _AppTile.fromJson(e as Map<String, dynamic>))
          .toList();
      if (!mounted) return;
      setState(() {
        _apps = items;
        _loading = false;
        _error = null;
      });
    } catch (e) {
      if (!mounted) return;
      setState(() {
        _error = '$e';
        _loading = false;
      });
    }
  }

  Future<void> _launchApp(_AppTile app) async {
    // Show the launching overlay and force it to paint BEFORE we ask the kernel
    // to launch: foreground-exclusive scheduling pauses the shell the moment the
    // child gets focus, so if we don't paint first the user sees a frozen screen
    // with no feedback while the new app's engine warms up.
    setState(() => _launching = app);
    await WidgetsBinding.instance.endOfFrame;

    final raw = await _shell.send('launch:${app.id}');
    final ok = raw != null && raw.contains('"ok":true');
    if (!ok && mounted) {
      setState(() => _launching = null);
      ScaffoldMessenger.of(context).showSnackBar(
        SnackBar(content: Text('Launch failed for ${app.name}')),
      );
    }
    // On success we leave the overlay up: the shell is about to be paused and
    // the compositor will switch to the app's surface once it presents.
  }

  Future<void> _installSeed() async {
    final raw = await _shell.send('install:/system/seed/demo.osx');
    if (raw != null && raw.contains('"ok":true')) {
      await _refreshApps();
    } else if (mounted) {
      ScaffoldMessenger.of(context).showSnackBar(
        const SnackBar(content: Text('Install failed (seed missing?)')),
      );
    }
  }

  @override
  Widget build(BuildContext context) {
    return Scaffold(
      body: Stack(
        children: [
          SafeArea(
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            Padding(
              padding: const EdgeInsets.fromLTRB(24, 20, 24, 8),
              child: Row(
                children: [
                  const Text(
                    'OSCortex',
                    style: TextStyle(
                      fontSize: 28,
                      fontWeight: FontWeight.w600,
                      letterSpacing: -0.5,
                    ),
                  ),
                  const Spacer(),
                  TextButton.icon(
                    onPressed: _installSeed,
                    icon: const Icon(Icons.download_outlined),
                    label: const Text('Install demo'),
                  ),
                  IconButton(
                    onPressed: () => _refreshApps(showSpinner: true),
                    icon: const Icon(Icons.refresh),
                    tooltip: 'Refresh',
                  ),
                  IconButton(
                    onPressed: () {
                      Navigator.of(context).push(
                        MaterialPageRoute<void>(
                          builder: (_) => const SettingsPage(),
                        ),
                      );
                    },
                    icon: const Icon(Icons.settings_outlined),
                    tooltip: 'Settings',
                  ),
                ],
              ),
            ),
            Padding(
              padding: const EdgeInsets.symmetric(horizontal: 24),
              child: Text(
                'Installed apps',
                style: TextStyle(
                  color: Colors.white.withValues(alpha: 0.7),
                  fontSize: 14,
                ),
              ),
            ),
            const SizedBox(height: 12),
            Expanded(
              child: _loading
                  ? const Center(child: CircularProgressIndicator())
                  : _error != null
                      ? Center(child: Text(_error!))
                      : _apps.isEmpty
                          ? Center(
                              child: Text(
                                'No apps installed.\nUse Install demo or drop a .osx bundle.',
                                textAlign: TextAlign.center,
                                style: TextStyle(
                                  color: Colors.white.withValues(alpha: 0.6),
                                ),
                              ),
                            )
                          : GridView.builder(
                              padding: const EdgeInsets.all(24),
                              gridDelegate:
                                  const SliverGridDelegateWithFixedCrossAxisCount(
                                crossAxisCount: 4,
                                mainAxisSpacing: 16,
                                crossAxisSpacing: 16,
                                childAspectRatio: 0.9,
                              ),
                              itemCount: _apps.length,
                              itemBuilder: (context, index) {
                                final app = _apps[index];
                                return _AppCard(
                                  app: app,
                                  onLaunch: () => _launchApp(app),
                                );
                              },
                            ),
            ),
          ],
        ),
          ),
          if (_launching != null) _buildLaunchOverlay(_launching!),
        ],
      ),
    );
  }

  Widget _buildLaunchOverlay(_AppTile app) {
    return Positioned.fill(
      child: Container(
        color: _bg.withValues(alpha: 0.92),
        child: Center(
          child: Column(
            mainAxisSize: MainAxisSize.min,
            children: [
              const SizedBox(
                width: 48,
                height: 48,
                child: CircularProgressIndicator(color: _accent, strokeWidth: 3),
              ),
              const SizedBox(height: 24),
              Text(
                'Launching ${app.name}…',
                style: const TextStyle(fontSize: 18, fontWeight: FontWeight.w600),
              ),
              const SizedBox(height: 8),
              Text(
                'first launch warms up the engine — a few moments',
                style: TextStyle(
                  fontSize: 12,
                  color: Colors.white.withValues(alpha: 0.5),
                ),
              ),
            ],
          ),
        ),
      ),
    );
  }
}

/// System Settings. Today it exposes input/driver preferences (scroll
/// direction + speed) which are applied live in the native embedder. New driver
/// settings plug in as additional `config:*` commands over the shell channel.
class SettingsPage extends StatefulWidget {
  const SettingsPage({super.key});

  @override
  State<SettingsPage> createState() => _SettingsPageState();
}

class _SettingsPageState extends State<SettingsPage> {
  bool _naturalScroll = false; // maps to embedder scroll_invert
  double _scrollSpeed = 100; // percent, 10..500
  bool _loaded = false;

  @override
  void initState() {
    super.initState();
    _loadConfig();
  }

  Future<void> _loadConfig() async {
    try {
      final raw = await _shell.send('config:get');
      final m = jsonDecode(raw ?? '{}') as Map<String, dynamic>;
      if (!mounted) return;
      setState(() {
        _naturalScroll = m['scroll_invert'] as bool? ?? false;
        _scrollSpeed = ((m['scroll_speed'] as num?)?.toDouble() ?? 100)
            .clamp(10, 500);
        _loaded = true;
      });
    } catch (_) {
      if (mounted) setState(() => _loaded = true);
    }
  }

  Future<void> _setNaturalScroll(bool v) async {
    setState(() => _naturalScroll = v);
    await _shell.send('config:scroll_invert:${v ? 1 : 0}');
  }

  Future<void> _setSpeed(double v) async {
    setState(() => _scrollSpeed = v);
    await _shell.send('config:scroll_speed:${v.round()}');
  }

  @override
  Widget build(BuildContext context) {
    return Scaffold(
      appBar: AppBar(
        title: const Text('Settings'),
        backgroundColor: _bg,
      ),
      body: !_loaded
          ? const Center(child: CircularProgressIndicator())
          : ListView(
              padding: const EdgeInsets.symmetric(horizontal: 24, vertical: 16),
              children: [
                _sectionLabel('Input · Mouse & Trackpad'),
                SwitchListTile(
                  value: _naturalScroll,
                  onChanged: _setNaturalScroll,
                  activeColor: _accent,
                  title: const Text('Natural scrolling'),
                  subtitle: Text(
                    _naturalScroll
                        ? 'Content tracks finger/wheel direction'
                        : 'Classic: wheel up scrolls up',
                    style: TextStyle(
                      color: Colors.white.withValues(alpha: 0.55),
                      fontSize: 12,
                    ),
                  ),
                ),
                Padding(
                  padding: const EdgeInsets.fromLTRB(16, 8, 16, 0),
                  child: Column(
                    crossAxisAlignment: CrossAxisAlignment.start,
                    children: [
                      Text(
                        'Scroll speed · ${_scrollSpeed.round()}%',
                        style: const TextStyle(fontSize: 14),
                      ),
                      Slider(
                        value: _scrollSpeed,
                        min: 10,
                        max: 500,
                        divisions: 49,
                        activeColor: _accent,
                        label: '${_scrollSpeed.round()}%',
                        onChanged: (v) => setState(() => _scrollSpeed = v),
                        onChangeEnd: _setSpeed,
                      ),
                    ],
                  ),
                ),
                const SizedBox(height: 16),
                _sectionLabel('Try it · scroll this list'),
                Container(
                  height: 220,
                  margin: const EdgeInsets.symmetric(horizontal: 16),
                  decoration: BoxDecoration(
                    color: Colors.white.withValues(alpha: 0.04),
                    borderRadius: BorderRadius.circular(12),
                  ),
                  child: ListView.builder(
                    padding: const EdgeInsets.all(8),
                    itemCount: 40,
                    itemBuilder: (context, i) => ListTile(
                      dense: true,
                      leading: Icon(Icons.drag_indicator,
                          color: Colors.white.withValues(alpha: 0.35)),
                      title: Text('Scrollable row ${i + 1}'),
                    ),
                  ),
                ),
              ],
            ),
    );
  }

  Widget _sectionLabel(String s) => Padding(
        padding: const EdgeInsets.fromLTRB(0, 8, 0, 8),
        child: Text(
          s.toUpperCase(),
          style: TextStyle(
            color: _accent.withValues(alpha: 0.9),
            fontSize: 12,
            fontWeight: FontWeight.w700,
            letterSpacing: 0.8,
          ),
        ),
      );
}

class _AppTile {
  const _AppTile({required this.id, required this.name});

  final int id;
  final String name;

  factory _AppTile.fromJson(Map<String, dynamic> json) {
    return _AppTile(
      id: json['id'] as int? ?? 0,
      name: json['name'] as String? ?? 'app',
    );
  }
}

class _AppCard extends StatelessWidget {
  const _AppCard({required this.app, required this.onLaunch});

  final _AppTile app;
  final VoidCallback onLaunch;

  @override
  Widget build(BuildContext context) {
    return Material(
      color: Colors.white.withValues(alpha: 0.06),
      borderRadius: BorderRadius.circular(16),
      child: InkWell(
        borderRadius: BorderRadius.circular(16),
        onTap: onLaunch,
        child: Padding(
          padding: const EdgeInsets.all(16),
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              Icon(Icons.widgets_outlined, color: _accent, size: 36),
              const Spacer(),
              Text(
                app.name,
                maxLines: 2,
                overflow: TextOverflow.ellipsis,
                style: const TextStyle(
                  fontSize: 15,
                  fontWeight: FontWeight.w600,
                ),
              ),
              Text(
                'id ${app.id}',
                style: TextStyle(
                  fontSize: 12,
                  color: Colors.white.withValues(alpha: 0.45),
                ),
              ),
            ],
          ),
        ),
      ),
    );
  }
}
