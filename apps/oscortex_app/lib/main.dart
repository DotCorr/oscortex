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
        colorScheme: const ColorScheme.dark(primary: _accent, surface: _bg),
        useMaterial3: true,
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
  String _status = 'Booting shell...';
  bool _loading = false;
  bool _installingSeed = false;

  static const Map<String, dynamic> _emptyApps = <String, dynamic>{
    'apps': <dynamic>[],
  };

  static void _trace(String message) {
    print('[shell-ui] $message');
  }

  bool get _demoInstalled => _apps.any((app) => app.name == 'Demo');

  void _setStatus(String message, {bool snack = false}) {
    _trace('status $message');
    if (mounted) {
      setState(() {
        _status = message;
      });
      if (snack) {
        ScaffoldMessenger.of(context)
          ..clearSnackBars()
          ..showSnackBar(SnackBar(content: Text(message)));
      }
    }
  }

  @override
  void initState() {
    super.initState();
    WidgetsBinding.instance.addPostFrameCallback((_) {
      _trace('ready');
    });
    _refreshApps();
  }

  Future<void> _refreshApps({bool showSpinner = false}) async {
    _trace('refresh_start');
    if (showSpinner) {
      _setStatus('Refresh tapped: asking host for app list...', snack: true);
    }
    if (showSpinner) {
      setState(() {
        _loading = true;
        _error = null;
      });
    }
    try {
      final raw = await _safeSend('list');
      final decoded = _parseJsonObject(raw, fallback: _emptyApps);
      final items = (decoded['apps'] as List<dynamic>? ?? [])
          .map((e) => _AppTile.fromJson(e as Map<String, dynamic>))
          .toList();
      if (!mounted) return;
      setState(() {
        _apps = items;
        _loading = false;
        _error = null;
      });
      _trace('refresh_ok count=${items.length}');
      _setStatus(
        'Refresh complete: ${items.length} app(s).',
        snack: showSpinner,
      );
    } catch (e) {
      _trace('refresh_error err=$e');
      if (!mounted) return;
      setState(() {
        _error = '$e';
        _loading = false;
      });
      _setStatus('Refresh failed: $e', snack: showSpinner);
    }
  }

  Future<void> _launchApp(int id) async {
    _trace('launch_tap id=$id');
    try {
      final raw = await _safeSend('launch:$id');
      if (!_isOkReply(raw)) {
        _trace('launch_failed id=$id');
        if (mounted) {
          ScaffoldMessenger.of(
            context,
          ).showSnackBar(SnackBar(content: Text('Launch failed for app $id')));
        }
      } else {
        _trace('launch_ok id=$id');
      }
    } catch (e) {
      _trace('launch_error id=$id err=$e');
      if (mounted) {
        ScaffoldMessenger.of(
          context,
        ).showSnackBar(SnackBar(content: Text('Launch error: $e')));
      }
    }
  }

  Future<void> _installSeed() async {
    if (_installingSeed || _demoInstalled) {
      _trace('install_skip');
      _setStatus('Install skipped: Demo is already installed.', snack: true);
      return;
    }
    setState(() {
      _installingSeed = true;
    });
    _trace('install_tap');
    _setStatus('Install tapped: sending request to host...', snack: true);
    try {
      final raw = await _safeSend('install:/system/seed/demo.osx');
      if (_isOkReply(raw)) {
        _trace('install_ok');
        _setStatus('Install complete: Demo is ready.', snack: true);
        await _refreshApps();
      } else if (mounted) {
        _trace('install_failed');
        _setStatus('Install failed: host rejected the request.', snack: true);
        ScaffoldMessenger.of(context).showSnackBar(
          const SnackBar(content: Text('Install failed (seed missing?)')),
        );
      }
    } catch (e) {
      _trace('install_error err=$e');
      _setStatus('Install error: $e', snack: true);
      if (mounted) {
        ScaffoldMessenger.of(
          context,
        ).showSnackBar(SnackBar(content: Text('Install error: $e')));
      }
    } finally {
      if (mounted) {
        setState(() {
          _installingSeed = false;
        });
      }
    }
  }

  Future<String?> _safeSend(String command) async {
    try {
      return await _shell.send(command);
    } catch (_) {
      return null;
    }
  }

  static bool _isOkReply(String? raw) {
    final json = _parseJsonObject(
      raw,
      fallback: const <String, dynamic>{'ok': false},
    );
    return json['ok'] == true;
  }

  static Map<String, dynamic> _parseJsonObject(
    String? raw, {
    required Map<String, dynamic> fallback,
  }) {
    if (raw == null || raw.isEmpty) return fallback;
    final trimmed = raw.trim();
    if (trimmed.isEmpty) return fallback;

    try {
      final decoded = jsonDecode(trimmed);
      if (decoded is Map<String, dynamic>) return decoded;
      if (decoded is Map) return decoded.cast<String, dynamic>();
    } catch (_) {
      // Some host replies may include wrappers/noise around JSON payload.
    }

    final start = trimmed.indexOf('{');
    final end = trimmed.lastIndexOf('}');
    if (start >= 0 && end > start) {
      final slice = trimmed.substring(start, end + 1);
      try {
        final decoded = jsonDecode(slice);
        if (decoded is Map<String, dynamic>) return decoded;
        if (decoded is Map) return decoded.cast<String, dynamic>();
      } catch (_) {
        return fallback;
      }
    }

    return fallback;
  }

  @override
  Widget build(BuildContext context) {
    return Scaffold(
      body: SizedBox.expand(
        child: SafeArea(
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
                      style: ButtonStyle(
                        backgroundColor: MaterialStateProperty.all(
                          Colors.pink.withValues(alpha: 0.6),
                        ),
                        minimumSize: MaterialStateProperty.all(
                          const Size(168, 52),
                        ),
                        padding: MaterialStateProperty.all(
                          const EdgeInsets.symmetric(
                            horizontal: 20,
                            vertical: 14,
                          ),
                        ),
                      ),
                      onPressed: (_installingSeed || _demoInstalled)
                          ? null
                          : _installSeed,
                      icon: const Icon(Icons.download_outlined),
                      label: Text(
                        _demoInstalled ? 'Installed' : 'Install demo',
                      ),
                    ),
                    const SizedBox(width: 12),
                    IconButton(
                      style: ButtonStyle(
                        minimumSize: MaterialStateProperty.all(
                          const Size.square(56),
                        ),
                        backgroundColor: MaterialStateProperty.all(
                          Colors.white.withValues(alpha: 0.08),
                        ),
                      ),
                      onPressed: _loading
                          ? null
                          : () => _refreshApps(showSpinner: true),
                      icon: _loading
                          ? const SizedBox.square(
                              dimension: 24,
                              child: CircularProgressIndicator(strokeWidth: 2),
                            )
                          : const Icon(Icons.refresh),
                      tooltip: 'Refresh',
                    ),
                  ],
                ),
              ),
              Padding(
                padding: const EdgeInsets.symmetric(horizontal: 24),
                child: Row(
                  children: [
                    Text(
                      'Installed apps',
                      style: TextStyle(
                        color: Colors.white.withValues(alpha: 0.7),
                        fontSize: 14,
                      ),
                    ),
                    const SizedBox(width: 16),
                    Expanded(
                      child: Text(
                        _status,
                        textAlign: TextAlign.right,
                        overflow: TextOverflow.ellipsis,
                        style: TextStyle(
                          color: Colors.white.withValues(alpha: 0.85),
                          fontSize: 13,
                        ),
                      ),
                    ),
                  ],
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
