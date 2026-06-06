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

  Future<void> _launchApp(int id) async {
    final raw = await _shell.send('launch:$id');
    if (raw == null || !raw.contains('"ok":true')) {
      if (mounted) {
        ScaffoldMessenger.of(context).showSnackBar(
          SnackBar(content: Text('Launch failed for app $id')),
        );
      }
    }
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
      body: SafeArea(
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
                                  onLaunch: () => _launchApp(app.id),
                                );
                              },
                            ),
            ),
          ],
        ),
      ),
    );
  }
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
