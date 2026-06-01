import 'dart:async';

import 'package:flutter/material.dart';
import 'package:oscortex_ui/oscortex_ui.dart';

import '../models/app_tile.dart';
import '../services/shell_service.dart';
import 'app_card.dart';

// ─── Shell Desktop ─────────────────────────────────────────────────────
class ShellDesktop extends StatefulWidget {
  const ShellDesktop({super.key, required this.accentColor});

  final Color accentColor;

  @override
  State<ShellDesktop> createState() => _ShellDesktopState();
}

class _ShellDesktopState extends State<ShellDesktop> {
  List<AppTile> _apps = const [];
  List<AppTile> _catalogApps = const []; // remote catalog entries
  String? _error;
  String _status = 'Scanning /Applications...';
  bool _loading = false;
  CoreAppRole _activeHub = CoreAppRole.canvas;
  late final Timer _clockTimer;
  String _clockLabel = '';

  @override
  void initState() {
    super.initState();
    _tickClock();
    _clockTimer = Timer.periodic(const Duration(seconds: 1), (_) {
      _tickClock();
    });
    WidgetsBinding.instance.addPostFrameCallback((_) {
      ShellService.trace('ready');
    });
    _refreshApps();
  }

  @override
  void dispose() {
    _clockTimer.cancel();
    super.dispose();
  }

  void _tickClock() {
    final now = DateTime.now();
    var hour = now.hour % 12;
    if (hour == 0) hour = 12;
    final minute = now.minute.toString().padLeft(2, '0');
    final suffix = now.hour >= 12 ? 'PM' : 'AM';
    final label = '$hour:$minute $suffix';
    if (mounted) {
      setState(() => _clockLabel = label);
    } else {
      _clockLabel = label;
    }
  }

  void _setStatus(String message, {bool snack = false}) {
    ShellService.trace('status $message');
    if (!mounted) return;
    setState(() => _status = message);
    if (snack) {
      ScaffoldMessenger.of(context)
        ..clearSnackBars()
        ..showSnackBar(SnackBar(content: Text(message)));
    }
  }

  Future<void> _refreshApps({bool showSpinner = false}) async {
    if (showSpinner) {
      _setStatus('Refreshing app registry...', snack: true);
    }
    setState(() {
      _loading = showSpinner;
      _error = null;
    });
    try {
      // Fetch both local registry AND remote catalog in parallel.
      final results = await Future.wait([
        ShellService.refreshApps(),
        ShellService.fetchCatalog(),
      ]);
      final localApps = results[0];
      final catalogApps = results[1];

      if (!mounted) return;

      // Merge: catalog apps that aren't already local.
      final localNames = localApps.map((a) => a.name).toSet();
      final remoteOnly = catalogApps
          .where((a) => !localNames.contains(a.name))
          .toList();

      setState(() {
        _apps = localApps;
        _catalogApps = remoteOnly;
        _loading = false;
      });

      final total = localApps.length + remoteOnly.length;
      final label = remoteOnly.isEmpty
          ? '${localApps.length} app(s) available.'
          : '${localApps.length} local · ${remoteOnly.length} on-demand · $total total';
      _setStatus(label);
    } catch (e) {
      if (!mounted) return;
      setState(() {
        _error = '$e';
        _loading = false;
      });
      _setStatus('Refresh failed: $e', snack: true);
    }
  }

  Future<void> _launchApp(AppTile app) async {
    // On-demand resolution for remote apps.
    if (app.isRemote) {
      await _resolveAndLaunch(app);
      return;
    }

    _setStatus('Launching ${app.name} as a separate app PID...', snack: true);
    final success = await ShellService.launchApp(app.id);
    if (!success) {
      _setStatus('Launch failed for ${app.name}.', snack: true);
      return;
    }
    _setStatus('${app.name} launch requested.');
  }

  Future<void> _resolveAndLaunch(AppTile app) async {
    // Set the app to resolving state.
    setState(() {
      final idx = _catalogApps.indexWhere((a) => a.name == app.name);
      if (idx >= 0) {
        _catalogApps = List.of(_catalogApps);
        _catalogApps[idx] = app.copyWith(source: AppSource.resolving);
      }
    });
    _setStatus('Fetching ${app.name} from package server...', snack: true);

    final appId = await ShellService.resolvePackage(app.name);

    if (!mounted) return;

    if (appId < 0) {
      // Resolve failed — revert to remote state.
      setState(() {
        final idx = _catalogApps.indexWhere((a) => a.name == app.name);
        if (idx >= 0) {
          _catalogApps = List.of(_catalogApps);
          _catalogApps[idx] = app.copyWith(source: AppSource.remote);
        }
      });
      _setStatus('Failed to fetch ${app.name}.', snack: true);
      return;
    }

    // Move from catalog to local apps.
    final localApp = app.copyWith(source: AppSource.local, id: appId);
    setState(() {
      _catalogApps = _catalogApps
          .where((a) => a.name != app.name)
          .toList();
      _apps = [..._apps, localApp];
    });
    _setStatus('${app.name} cached — launching...', snack: true);

    // Now launch.
    final success = await ShellService.launchApp(appId);
    if (!success) {
      _setStatus('Launch failed for ${app.name}.', snack: true);
      return;
    }
    _setStatus('${app.name} launched.');
  }

  void _setActiveHub(CoreAppRole role) {
    setState(() => _activeHub = role);
  }

  AppTile? _core(CoreAppRole role) {
    for (final app in _apps) {
      if (app.coreRole == role) return app;
    }
    return null;
  }

  /// All apps shown in the workspace: local non-core first, then remote catalog.
  List<AppTile> get _nonCoreApps => [
        for (final app in _apps)
          if (!app.isCore) app,
        ..._catalogApps,
      ];

  @override
  Widget build(BuildContext context) {
    return Listener(
      onPointerDown: (event) {
        ShellService.trace(
          'pointer_down x=${event.position.dx} y=${event.position.dy} btns=${event.buttons}',
        );
      },
      child: OscScaffold(
        accent: OscColors.violet,
        showFrame: true,
        body: Column(
          children: [
            _buildToolbar(),
            Expanded(
              child: Row(
                children: [
                  _buildAppRail(),
                  Expanded(
                    child: _Workspace(
                      apps: _nonCoreApps,
                      error: _error,
                      loading: _loading,
                      onLaunch: _launchApp,
                    ),
                  ),
                ],
              ),
            ),
            _buildHubDock(),
          ],
        ),
      ),
    );
  }

  Widget _buildToolbar() {
    return OscToolbar(
      title: 'Personal Canvas',
      status: _status,
      trailing: [
        _SettingsButton(),
        const SizedBox(width: 2),
        Text(_clockLabel, style: OscTypography.clock),
        SizedBox(
          width: 32,
          height: 32,
          child: IconButton(
            onPressed: _loading ? null : () => _refreshApps(showSpinner: true),
            tooltip: 'Refresh installed apps',
            padding: EdgeInsets.zero,
            iconSize: 16,
            icon: _loading
                ? const SizedBox.square(
                    dimension: 14,
                    child: CircularProgressIndicator(
                      strokeWidth: 1.5,
                      color: OscColors.violetLight,
                    ),
                  )
                : Icon(
                    Icons.refresh_rounded,
                    size: 16,
                    color: Colors.white.withValues(alpha: 0.50),
                  ),
          ),
        ),
      ],
    );
  }

  Widget _buildAppRail() {
    final railApps = _apps.take(5).toList();
    return OscAppRail(
      bottom: OscRailButton(
        icon: Icons.grid_view_rounded,
        label: 'All Apps',
        dashed: true,
        onTap: () {},
      ),
      children: [
        for (final app in railApps)
          OscRailButton(
            icon: _iconForRole(app.coreRole),
            label: _shortName(app.name),
            active: app.coreRole == _activeHub,
            accentColor: OscColors.violetLight,
            onTap: () => _launchApp(app),
            tooltip: app.name,
          ),
      ],
    );
  }

  Widget _buildHubDock() {
    final canvas = _core(CoreAppRole.canvas);
    final files = _core(CoreAppRole.files);
    final web = _core(CoreAppRole.web);

    return OscHubDock(
      segments: [
        OscHubSegment(
          label: 'Canvas Hub',
          active: _activeHub == CoreAppRole.canvas,
          enabled: canvas != null,
          onTap: () {
            _setActiveHub(CoreAppRole.canvas);
            if (canvas != null) _launchApp(canvas);
          },
        ),
        OscHubSegment(
          label: 'Files',
          active: _activeHub == CoreAppRole.files,
          enabled: files != null,
          onTap: () {
            _setActiveHub(CoreAppRole.files);
            if (files != null) _launchApp(files);
          },
        ),
        OscHubSegment(
          label: 'Web Link',
          active: _activeHub == CoreAppRole.web,
          enabled: web != null,
          onTap: () {
            _setActiveHub(CoreAppRole.web);
            if (web != null) _launchApp(web);
          },
        ),
      ],
    );
  }

  static IconData _iconForRole(CoreAppRole? role) {
    return switch (role) {
      CoreAppRole.canvas => Icons.dashboard_customize_rounded,
      CoreAppRole.files => Icons.folder_rounded,
      CoreAppRole.web => Icons.link_rounded,
      null => Icons.apps_rounded,
    };
  }

  static String _shortName(String name) {
    if (name.length <= 8) return name;
    return '${name.substring(0, 7)}…';
  }
}

// ─── Settings Button ───────────────────────────────────────────────────
class _SettingsButton extends StatelessWidget {
  @override
  Widget build(BuildContext context) {
    return Container(
      height: 28,
      padding: const EdgeInsets.symmetric(horizontal: 10),
      decoration: BoxDecoration(
        color: Colors.white.withValues(alpha: 0.04),
        borderRadius: OscRadii.iconBorder,
        border: Border.all(color: OscColors.border),
      ),
      child: Row(
        mainAxisSize: MainAxisSize.min,
        children: [
          Icon(
            Icons.settings_outlined,
            size: 13,
            color: Colors.white.withValues(alpha: 0.45),
          ),
          const SizedBox(width: 5),
          Text(
            'Settings',
            style: OscTypography.settingsButton.copyWith(
              color: Colors.white.withValues(alpha: 0.45),
            ),
          ),
        ],
      ),
    );
  }
}

// ─── Workspace (main content area with spec cards) ─────────────────────
class _Workspace extends StatelessWidget {
  const _Workspace({
    required this.apps,
    required this.error,
    required this.loading,
    required this.onLaunch,
  });

  final List<AppTile> apps;
  final String? error;
  final bool loading;
  final ValueChanged<AppTile> onLaunch;

  @override
  Widget build(BuildContext context) {
    if (loading) {
      return const Center(
        child: CircularProgressIndicator(color: OscColors.violetLight),
      );
    }
    if (error != null) {
      return Center(
        child: Text(error!, style: const TextStyle(color: OscColors.textMuted)),
      );
    }

    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        // Intent Bar
        const Padding(
          padding: EdgeInsets.fromLTRB(16, 0, 16, 0),
          child: OscIntentBar(),
        ),
        // Scrollable workspace cards
        Expanded(
          child: ListView(
            padding: const EdgeInsets.fromLTRB(14, 8, 14, 14),
            children: [
              // Document Canvas Card
              const _DocumentCanvasCard(),
              const SizedBox(height: 12),
              // Two-column row: Media Player + Knowledge Source
              const Row(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  Expanded(child: _MediaPlayerCard()),
                  SizedBox(width: 12),
                  Expanded(child: _KnowledgeSourceCard()),
                ],
              ),
              const SizedBox(height: 12),
              // Generated Engine View
              const _GeneratedEngineCard(),
              // Third-party apps grid (if any)
              if (apps.isNotEmpty) ...[
                const SizedBox(height: 16),
                Padding(
                  padding: const EdgeInsets.symmetric(horizontal: 4),
                  child: OscSectionLabel(
                    'INSTALLED APPS',
                    small: true,
                  ),
                ),
                const SizedBox(height: 8),
                GridView.builder(
                  shrinkWrap: true,
                  physics: const NeverScrollableScrollPhysics(),
                  gridDelegate: const SliverGridDelegateWithMaxCrossAxisExtent(
                    maxCrossAxisExtent: 200,
                    mainAxisSpacing: 10,
                    crossAxisSpacing: 10,
                    childAspectRatio: 1.1,
                  ),
                  itemCount: apps.length,
                  itemBuilder: (context, index) {
                    final app = apps[index];
                    return AppCard(
                      app: app,
                      onLaunch: () => onLaunch(app),
                      accentColor: OscColors.violet,
                    );
                  },
                ),
              ],
            ],
          ),
        ),
      ],
    );
  }
}

// ─── Document Canvas Card ──────────────────────────────────────────────
class _DocumentCanvasCard extends StatelessWidget {
  const _DocumentCanvasCard();

  @override
  Widget build(BuildContext context) {
    return OscCard(
      gradient: OscCard.violetGradient,
      padding: const EdgeInsets.all(18),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Row(
            mainAxisAlignment: MainAxisAlignment.spaceBetween,
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              Column(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  OscSectionLabel(
                    'DOCUMENT CANVAS',
                    color: OscColors.violetLight.withValues(alpha: 0.85),
                  ),
                  const SizedBox(height: 6),
                  Text(
                    '\u201CProject Proposal.md\u201D',
                    style: OscTypography.heading3,
                  ),
                ],
              ),
              const OscStatusBadge(
                label: 'Auto-saving',
                color: OscColors.green,
              ),
            ],
          ),
          const SizedBox(height: 14),
          Text(
            'The transition away from static software models requires a radical '
            'inversion of hardware abstraction maps. By structuring intent parsing '
            'frameworks at the system core layer, users no longer interact with '
            'isolated platforms, but rather text environments that shift dynamically '
            'based on temporary workflow requirements\u2026',
            style: OscTypography.bodySmall,
          ),
        ],
      ),
    );
  }
}

// ─── Media Player Card ─────────────────────────────────────────────────
class _MediaPlayerCard extends StatelessWidget {
  const _MediaPlayerCard();

  @override
  Widget build(BuildContext context) {
    return OscCard(
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          const OscSectionLabel('NOW PLAYING', small: true),
          const SizedBox(height: 10),
          Row(
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              Expanded(
                child: Column(
                  crossAxisAlignment: CrossAxisAlignment.start,
                  children: [
                    Text(
                      'Ambient Waves',
                      style: OscTypography.body.copyWith(
                        color: OscColors.textBright,
                      ),
                    ),
                    const SizedBox(height: 3),
                    Text(
                      'Cortex Generative Radio',
                      style: OscTypography.caption.copyWith(
                        color: Colors.white.withValues(alpha: 0.35),
                      ),
                    ),
                  ],
                ),
              ),
              const OscWaveform(),
            ],
          ),
          const SizedBox(height: 12),
          Wrap(
            spacing: 6,
            runSpacing: 4,
            children: [
              const OscTag(label: 'Local Sync'),
              OscTag.violet(label: 'Spatial Audio Active'),
            ],
          ),
        ],
      ),
    );
  }
}

// ─── Knowledge Source Card ──────────────────────────────────────────────
class _KnowledgeSourceCard extends StatelessWidget {
  const _KnowledgeSourceCard();

  @override
  Widget build(BuildContext context) {
    return OscCard(
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          const OscSectionLabel('KNOWLEDGE SOURCE ENGINE', small: true),
          const SizedBox(height: 8),
          Text(
            'https://en.wikipedia.org/wiki/Operating_system',
            style: OscTypography.monoSmall.copyWith(color: OscColors.skyBlue),
            overflow: TextOverflow.ellipsis,
          ),
          const SizedBox(height: 6),
          Text(
            'Operating Systems Architecture Evolution',
            style: OscTypography.bodySmall.copyWith(
              fontWeight: FontWeight.w500,
              color: OscColors.textBright,
            ),
          ),
          const SizedBox(height: 8),
          Text(
            'An operating system (OS) is system software that manages '
            'computer hardware, software resources, and provides common '
            'services for computer programs\u2026',
            style: TextStyle(
              fontSize: 10.5,
              height: 1.5,
              color: Colors.white.withValues(alpha: 0.35),
            ),
          ),
        ],
      ),
    );
  }
}

// ─── Generated Engine View Card ────────────────────────────────────────
class _GeneratedEngineCard extends StatelessWidget {
  const _GeneratedEngineCard();

  @override
  Widget build(BuildContext context) {
    return OscCard(
      activeBorder: true,
      gradient: OscCard.liveGradient,
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Row(
            mainAxisAlignment: MainAxisAlignment.spaceBetween,
            children: [
              const OscSectionLabel('GENERATED ENGINE VIEW', small: true),
              OscStatusBadge(
                label: 'Live',
                color: OscColors.violetLight,
              ),
            ],
          ),
          const SizedBox(height: 10),
          Text(
            'Injected general-purpose widget context directly into active '
            'space. Render layout optimized natively.',
            style: OscTypography.bodySmall,
          ),
        ],
      ),
    );
  }
}

// ─── Previews ──────────────────────────────────────────────────────────

@OscPreview(name: 'Document Canvas Card', group: 'Shell Workspace')
Widget documentCanvasPreview() {
  return const SizedBox(width: 500, child: _DocumentCanvasCard());
}

@OscPreview(name: 'Media Player Card', group: 'Shell Workspace')
Widget mediaPlayerPreview() {
  return const SizedBox(width: 300, child: _MediaPlayerCard());
}

@OscPreview(name: 'Knowledge Source Card', group: 'Shell Workspace')
Widget knowledgeSourcePreview() {
  return const SizedBox(width: 300, child: _KnowledgeSourceCard());
}

@OscPreview(name: 'Generated Engine Card', group: 'Shell Workspace')
Widget generatedEnginePreview() {
  return const SizedBox(width: 500, child: _GeneratedEngineCard());
}

@OscPreview(name: 'Settings Button', group: 'Shell Chrome')
Widget settingsButtonPreview() {
  return Padding(
    padding: const EdgeInsets.all(16),
    child: _SettingsButton(),
  );
}
