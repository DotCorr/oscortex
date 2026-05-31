import 'package:flutter/material.dart';
import '../models/app_tile.dart';
import '../services/shell_service.dart';
import 'app_card.dart';

class ShellDesktop extends StatefulWidget {
  const ShellDesktop({super.key, required this.accentColor});

  final Color accentColor;

  @override
  State<ShellDesktop> createState() => _ShellDesktopState();
}

class _ShellDesktopState extends State<ShellDesktop> {
  List<AppTile> _apps = const [];
  String? _error;
  String _status = 'Booting shell...';
  bool _loading = false;
  bool _installingSeed = false;

  bool get _demoInstalled => _apps.any((app) => app.name == 'Demo');

  void _setStatus(String message, {bool snack = false}) {
    ShellService.trace('status $message');
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
      ShellService.trace('ready');
    });
    _refreshApps();
  }

  Future<void> _refreshApps({bool showSpinner = false}) async {
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
      final items = await ShellService.refreshApps();
      if (!mounted) return;
      setState(() {
        _apps = items;
        _loading = false;
        _error = null;
      });
      ShellService.trace('refresh_ok count=${items.length}');
      _setStatus(
        'Refresh complete: ${items.length} app(s).',
        snack: showSpinner,
      );
    } catch (e) {
      if (!mounted) return;
      setState(() {
        _error = '$e';
        _loading = false;
      });
      _setStatus('Refresh failed: $e', snack: showSpinner);
    }
  }

  Future<void> _launchApp(int id) async {
    final success = await ShellService.launchApp(id);
    if (!success && mounted) {
      ScaffoldMessenger.of(context).showSnackBar(
        SnackBar(content: Text('Launch failed for app $id')),
      );
    }
  }

  Future<void> _installSeed() async {
    if (_installingSeed || _demoInstalled) {
      _setStatus('Install skipped: Demo is already installed.', snack: true);
      return;
    }
    setState(() {
      _installingSeed = true;
    });
    _setStatus('Install tapped: sending request to host...', snack: true);
    try {
      final success = await ShellService.installSeed();
      if (success) {
        _setStatus('Install complete: Demo is ready.', snack: true);
        await _refreshApps();
      } else if (mounted) {
        _setStatus('Install failed: host rejected the request.', snack: true);
        ScaffoldMessenger.of(context).showSnackBar(
          const SnackBar(content: Text('Install failed (seed missing?)')),
        );
      }
    } catch (e) {
      _setStatus('Install error: $e', snack: true);
      if (mounted) {
        ScaffoldMessenger.of(context).showSnackBar(
          SnackBar(content: Text('Install error: $e')),
        );
      }
    } finally {
      if (mounted) {
        setState(() {
          _installingSeed = false;
        });
      }
    }
  }

  @override
  Widget build(BuildContext context) {
    return Listener(
      onPointerDown: (event) {
        ShellService.trace('pointer_down x=${event.position.dx} y=${event.position.dy} btns=${event.buttons}');
      },
      onPointerMove: (event) {
        ShellService.trace('pointer_move x=${event.position.dx} y=${event.position.dy} btns=${event.buttons}');
      },
      onPointerUp: (event) {
        ShellService.trace('pointer_up x=${event.position.dx} y=${event.position.dy} btns=${event.buttons}');
      },
      onPointerHover: (event) {
        ShellService.trace('pointer_hover x=${event.position.dx} y=${event.position.dy}');
      },
      child: Scaffold(
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
                          backgroundColor: WidgetStateProperty.resolveWith<Color?>((states) {
                            if (states.contains(WidgetState.hovered)) {
                              return Colors.pink.withValues(alpha: 0.85);
                            }
                            if (states.contains(WidgetState.pressed)) {
                              return Colors.pink.withValues(alpha: 1.0);
                            }
                            return Colors.pink.withValues(alpha: 0.6);
                          }),
                          minimumSize: WidgetStateProperty.all(
                            const Size(168, 52),
                          ),
                          padding: WidgetStateProperty.all(
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
                          minimumSize: WidgetStateProperty.all(
                            const Size.square(56),
                          ),
                          backgroundColor: WidgetStateProperty.resolveWith<Color?>((states) {
                            if (states.contains(WidgetState.hovered)) {
                              return Colors.white.withValues(alpha: 0.18);
                            }
                            if (states.contains(WidgetState.pressed)) {
                              return Colors.white.withValues(alpha: 0.28);
                            }
                            return Colors.white.withValues(alpha: 0.08);
                          }),
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
                            return AppCard(
                              app: app,
                              onLaunch: () => _launchApp(app.id),
                              accentColor: widget.accentColor,
                            );
                          },
                        ),
                ),
              ],
            ),
          ),
        ),
      ),
    );
  }
}
