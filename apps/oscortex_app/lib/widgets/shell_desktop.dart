import 'dart:async';
import 'dart:math' as math;

import 'package:flutter/material.dart';

import '../models/app_tile.dart';
import '../services/shell_service.dart';
import 'app_card.dart';

// ─── OSCortex Design Tokens (from oscortex-ui-spec.html) ───────────────
const _kBodyBg = Color(0xFF07080B);
const _kCanvasBg = Color(0xFF0B0D12);
const _kSurfaceCard = Color(0x06FFFFFF);
const _kAccentViolet = Color(0xFF7C3AED);
const _kAccentVioletLight = Color(0xFFA78BFA);
const _kActiveBorder = Color(0x40A78BFA);
const _kSignalGreen = Color(0xFF10B981);
const _kSkyBlue = Color(0xFF38BDF8);
const _kBorder = Color(0x14FFFFFF);
const _kBorderLight = Color(0x0DFFFFFF);
const _kTextMuted = Color(0xFF71717A);

// ─── Shell Desktop ─────────────────────────────────────────────────────
class ShellDesktop extends StatefulWidget {
  const ShellDesktop({super.key, required this.accentColor});

  final Color accentColor;

  @override
  State<ShellDesktop> createState() => _ShellDesktopState();
}

class _ShellDesktopState extends State<ShellDesktop> {
  List<AppTile> _apps = const [];
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
      final items = await ShellService.refreshApps();
      if (!mounted) return;
      setState(() {
        _apps = items;
        _loading = false;
      });
      _setStatus('${items.length} app(s) available from the OS registry.');
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
    _setStatus('Launching ${app.name} as a separate app PID...', snack: true);
    final success = await ShellService.launchApp(app.id);
    if (!success) {
      _setStatus('Launch failed for ${app.name}.', snack: true);
      return;
    }
    _setStatus('${app.name} launch requested.');
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

  List<AppTile> get _nonCoreApps => [
        for (final app in _apps)
          if (!app.isCore) app,
      ];

  @override
  Widget build(BuildContext context) {
    return Listener(
      onPointerDown: (event) {
        ShellService.trace(
          'pointer_down x=${event.position.dx} y=${event.position.dy} btns=${event.buttons}',
        );
      },
      child: Scaffold(
        body: Container(
          decoration: const BoxDecoration(
            color: _kBodyBg,
            gradient: RadialGradient(
              center: Alignment(0, -1.2),
              radius: 1.1,
              colors: [Color(0x337C3AED), Color(0x00070808)],
            ),
          ),
          child: SafeArea(
            child: Padding(
              padding: const EdgeInsets.all(16),
              child: DecoratedBox(
                decoration: BoxDecoration(
                  color: _kCanvasBg,
                  borderRadius: BorderRadius.circular(20),
                  border: Border.all(color: _kBorderLight),
                  boxShadow: const [
                    BoxShadow(
                      color: Color(0xB3000000),
                      blurRadius: 120,
                      offset: Offset(0, 48),
                      spreadRadius: -24,
                    ),
                  ],
                ),
                child: ClipRRect(
                  borderRadius: BorderRadius.circular(20),
                  child: Stack(
                    children: [
                      const Positioned.fill(child: _AmbientLayer()),
                      Column(
                        children: [
                          _TopChrome(
                            status: _status,
                            loading: _loading,
                            clockLabel: _clockLabel,
                            onRefresh: _loading
                                ? null
                                : () => _refreshApps(showSpinner: true),
                          ),
                          Expanded(
                            child: Row(
                              children: [
                                _AppRail(
                                  apps: _apps.take(5).toList(),
                                  activeCoreRole: _activeHub,
                                  onLaunch: _launchApp,
                                ),
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
                          _HubDock(
                            canvas: _core(CoreAppRole.canvas),
                            files: _core(CoreAppRole.files),
                            web: _core(CoreAppRole.web),
                            active: _activeHub,
                            onActivate: _setActiveHub,
                            onLaunch: _launchApp,
                          ),
                        ],
                      ),
                    ],
                  ),
                ),
              ),
            ),
          ),
        ),
      ),
    );
  }
}

// ─── Ambient Glow Layer (dual radial gradients from spec) ──────────────
class _AmbientLayer extends StatelessWidget {
  const _AmbientLayer();

  @override
  Widget build(BuildContext context) {
    return DecoratedBox(
      decoration: BoxDecoration(
        gradient: RadialGradient(
          center: Alignment.topLeft,
          radius: 1.4,
          colors: [
            _kAccentViolet.withValues(alpha: 0.14),
            Colors.transparent,
          ],
        ),
      ),
      child: DecoratedBox(
        decoration: BoxDecoration(
          gradient: RadialGradient(
            center: const Alignment(0.85, -0.3),
            radius: 1.1,
            colors: [
              _kSkyBlue.withValues(alpha: 0.05),
              Colors.transparent,
            ],
          ),
        ),
      ),
    );
  }
}

// ─── Top Chrome (44px header) ──────────────────────────────────────────
class _TopChrome extends StatelessWidget {
  const _TopChrome({
    required this.status,
    required this.loading,
    required this.clockLabel,
    required this.onRefresh,
  });

  final String status;
  final bool loading;
  final String clockLabel;
  final VoidCallback? onRefresh;

  @override
  Widget build(BuildContext context) {
    return Container(
      height: 44,
      padding: const EdgeInsets.symmetric(horizontal: 16),
      decoration: BoxDecoration(
        border: Border(
          bottom: BorderSide(color: Colors.white.withValues(alpha: 0.07)),
        ),
      ),
      child: Row(
        children: [
          const Text(
            'Personal Canvas',
            style: TextStyle(
              fontSize: 13,
              fontWeight: FontWeight.w600,
              color: Color(0xFFE5E5E5),
            ),
          ),
          const SizedBox(width: 14),
          Expanded(
            child: Text(
              status,
              overflow: TextOverflow.ellipsis,
              style: TextStyle(
                fontSize: 11,
                color: Colors.white.withValues(alpha: 0.40),
              ),
            ),
          ),
          const SizedBox(width: 8),
          _SettingsButton(),
          const SizedBox(width: 10),
          Text(
            clockLabel,
            style: const TextStyle(
              fontSize: 12,
              fontWeight: FontWeight.w500,
              color: _kTextMuted,
              fontFeatures: [FontFeature.tabularFigures()],
            ),
          ),
          const SizedBox(width: 4),
          SizedBox(
            width: 32,
            height: 32,
            child: IconButton(
              onPressed: onRefresh,
              tooltip: 'Refresh installed apps',
              padding: EdgeInsets.zero,
              iconSize: 16,
              icon: loading
                  ? const SizedBox.square(
                      dimension: 14,
                      child: CircularProgressIndicator(
                        strokeWidth: 1.5,
                        color: _kAccentVioletLight,
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
      ),
    );
  }
}

class _SettingsButton extends StatelessWidget {
  @override
  Widget build(BuildContext context) {
    return Container(
      height: 28,
      padding: const EdgeInsets.symmetric(horizontal: 10),
      decoration: BoxDecoration(
        color: Colors.white.withValues(alpha: 0.04),
        borderRadius: BorderRadius.circular(8),
        border: Border.all(color: Colors.white.withValues(alpha: 0.08)),
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
            style: TextStyle(
              fontSize: 11,
              color: Colors.white.withValues(alpha: 0.45),
            ),
          ),
        ],
      ),
    );
  }
}

// ─── App Rail (68px, spec-compliant) ───────────────────────────────────
class _AppRail extends StatelessWidget {
  const _AppRail({
    required this.apps,
    required this.activeCoreRole,
    required this.onLaunch,
  });

  final List<AppTile> apps;
  final CoreAppRole activeCoreRole;
  final ValueChanged<AppTile> onLaunch;

  @override
  Widget build(BuildContext context) {
    return Container(
      width: 68,
      decoration: BoxDecoration(
        color: Colors.black.withValues(alpha: 0.20),
        border: Border(
          right: BorderSide(color: Colors.white.withValues(alpha: 0.07)),
        ),
      ),
      child: Column(
        children: [
          const SizedBox(height: 10),
          for (final app in apps)
            Padding(
              padding: const EdgeInsets.symmetric(horizontal: 6, vertical: 2),
              child: Tooltip(
                message: app.name,
                child: _RailAppButton(
                  app: app,
                  active: app.coreRole == activeCoreRole,
                  onTap: () => onLaunch(app),
                ),
              ),
            ),
          const Spacer(),
          // "All Apps" dashed button at bottom
          Padding(
            padding: const EdgeInsets.fromLTRB(6, 0, 6, 12),
            child: Container(
              decoration: BoxDecoration(
                borderRadius: BorderRadius.circular(10),
                border: Border.all(
                  color: Colors.white.withValues(alpha: 0.10),
                  style: BorderStyle.solid,
                ),
              ),
              child: Column(
                children: [
                  const SizedBox(height: 8),
                  Icon(
                    Icons.grid_view_rounded,
                    size: 16,
                    color: Colors.white.withValues(alpha: 0.40),
                  ),
                  const SizedBox(height: 4),
                  Text(
                    'All Apps',
                    style: TextStyle(
                      fontSize: 7,
                      fontWeight: FontWeight.w500,
                      color: Colors.white.withValues(alpha: 0.40),
                    ),
                  ),
                  const SizedBox(height: 6),
                ],
              ),
            ),
          ),
        ],
      ),
    );
  }
}

// ─── Rail App Button (icon + label, violet active state) ───────────────
class _RailAppButton extends StatefulWidget {
  const _RailAppButton({
    required this.app,
    required this.active,
    required this.onTap,
  });

  final AppTile app;
  final bool active;
  final VoidCallback onTap;

  @override
  State<_RailAppButton> createState() => _RailAppButtonState();
}

class _RailAppButtonState extends State<_RailAppButton> {
  bool _hovered = false;

  @override
  Widget build(BuildContext context) {
    final icon = switch (widget.app.coreRole) {
      CoreAppRole.canvas => Icons.dashboard_customize_rounded,
      CoreAppRole.files => Icons.folder_rounded,
      CoreAppRole.web => Icons.link_rounded,
      null => Icons.apps_rounded,
    };

    final isActive = widget.active;
    final bgAlpha = isActive
        ? 0.06
        : _hovered
            ? 0.04
            : 0.0;

    return MouseRegion(
      onEnter: (_) => setState(() => _hovered = true),
      onExit: (_) => setState(() => _hovered = false),
      child: GestureDetector(
        onTap: widget.onTap,
        child: AnimatedContainer(
          duration: const Duration(milliseconds: 160),
          curve: Curves.easeOut,
          decoration: BoxDecoration(
            color: Colors.white.withValues(alpha: bgAlpha),
            borderRadius: BorderRadius.circular(10),
          ),
          child: Column(
            children: [
              const SizedBox(height: 8),
              Container(
                width: 28,
                height: 28,
                decoration: BoxDecoration(
                  borderRadius: BorderRadius.circular(8),
                  border: Border.all(
                    color: isActive
                        ? _kAccentViolet.withValues(alpha: 0.30)
                        : Colors.white.withValues(alpha: 0.06),
                  ),
                  color: isActive
                      ? _kAccentViolet.withValues(alpha: 0.15)
                      : Colors.white.withValues(alpha: 0.03),
                ),
                child: Icon(
                  icon,
                  size: 14,
                  color: widget.app.isCore
                      ? (isActive
                          ? _kAccentVioletLight
                          : Colors.white.withValues(alpha: 0.50))
                      : Colors.white.withValues(
                          alpha: _hovered ? 0.60 : 0.40),
                ),
              ),
              const SizedBox(height: 4),
              Text(
                _shortName(widget.app.name),
                style: TextStyle(
                  fontSize: 7.5,
                  fontWeight: isActive ? FontWeight.w600 : FontWeight.w400,
                  color: isActive
                      ? Colors.white.withValues(alpha: 0.85)
                      : Colors.white.withValues(alpha: 0.45),
                ),
                overflow: TextOverflow.ellipsis,
              ),
              const SizedBox(height: 6),
            ],
          ),
        ),
      ),
    );
  }

  String _shortName(String name) {
    if (name.length <= 8) return name;
    return '${name.substring(0, 7)}…';
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
        child: CircularProgressIndicator(color: _kAccentVioletLight),
      );
    }
    if (error != null) {
      return Center(
        child: Text(error!, style: const TextStyle(color: _kTextMuted)),
      );
    }

    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        // Intent Bar
        const Padding(
          padding: EdgeInsets.fromLTRB(16, 0, 16, 0),
          child: _IntentInputBar(),
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
                  child: Text(
                    'INSTALLED APPS',
                    style: TextStyle(
                      fontFamily: 'RobotoMono',
                      fontSize: 9,
                      fontWeight: FontWeight.w600,
                      letterSpacing: 1.4,
                      color: Colors.white.withValues(alpha: 0.35),
                    ),
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
                      accentColor: _kAccentViolet,
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

// ─── Intent Input Bar ──────────────────────────────────────────────────
class _IntentInputBar extends StatefulWidget {
  const _IntentInputBar();

  @override
  State<_IntentInputBar> createState() => _IntentInputBarState();
}

class _IntentInputBarState extends State<_IntentInputBar> {
  bool _caretOn = true;
  Timer? _caretTimer;

  @override
  void initState() {
    super.initState();
    _caretTimer = Timer.periodic(const Duration(milliseconds: 500), (_) {
      if (!mounted) return;
      setState(() => _caretOn = !_caretOn);
    });
  }

  @override
  void dispose() {
    _caretTimer?.cancel();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    return Container(
      padding: const EdgeInsets.symmetric(horizontal: 16, vertical: 10),
      decoration: BoxDecoration(
        border: Border(
          bottom: BorderSide(color: Colors.white.withValues(alpha: 0.06)),
        ),
      ),
      child: Container(
        padding: const EdgeInsets.symmetric(horizontal: 12, vertical: 9),
        decoration: BoxDecoration(
          color: Colors.white.withValues(alpha: 0.035),
          borderRadius: BorderRadius.circular(12),
          border: Border.all(color: Colors.white.withValues(alpha: 0.06)),
        ),
        child: Row(
          children: [
            const Text(
              '>',
              style: TextStyle(
                fontFamily: 'RobotoMono',
                fontSize: 13,
                fontWeight: FontWeight.w700,
                color: _kAccentVioletLight,
              ),
            ),
            const SizedBox(width: 8),
            Text(
              'What are we building today?',
              style: TextStyle(
                fontFamily: 'RobotoMono',
                fontSize: 12,
                color: Colors.white.withValues(alpha: 0.70),
              ),
            ),
            const SizedBox(width: 2),
            AnimatedOpacity(
              duration: const Duration(milliseconds: 80),
              opacity: _caretOn ? 1 : 0,
              child: Container(
                width: 7,
                height: 16,
                decoration: BoxDecoration(
                  color: _kAccentVioletLight,
                  borderRadius: BorderRadius.circular(1),
                ),
              ),
            ),
          ],
        ),
      ),
    );
  }
}

// ─── Document Canvas Card ──────────────────────────────────────────────
class _DocumentCanvasCard extends StatelessWidget {
  const _DocumentCanvasCard();

  @override
  Widget build(BuildContext context) {
    return Container(
      padding: const EdgeInsets.all(18),
      decoration: BoxDecoration(
        borderRadius: BorderRadius.circular(14),
        border: Border.all(color: _kBorder),
        gradient: const LinearGradient(
          begin: Alignment(-0.6, -1),
          end: Alignment(0.8, 1),
          colors: [Color(0x0D7C3AED), Color(0x06FFFFFF)],
        ),
        boxShadow: const [
          BoxShadow(
            color: Color(0x80000000),
            blurRadius: 24,
            offset: Offset(0, 8),
            spreadRadius: -12,
          ),
        ],
      ),
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
                  Text(
                    'DOCUMENT CANVAS',
                    style: TextStyle(
                      fontFamily: 'RobotoMono',
                      fontSize: 10,
                      fontWeight: FontWeight.w600,
                      letterSpacing: 1.4,
                      color: _kAccentVioletLight.withValues(alpha: 0.85),
                    ),
                  ),
                  const SizedBox(height: 6),
                  const Text(
                    '\u201CProject Proposal.md\u201D',
                    style: TextStyle(
                      fontSize: 15,
                      fontWeight: FontWeight.w600,
                      color: Colors.white,
                    ),
                  ),
                ],
              ),
              const _AutoSaveBadge(),
            ],
          ),
          const SizedBox(height: 14),
          Text(
            'The transition away from static software models requires a radical '
            'inversion of hardware abstraction maps. By structuring intent parsing '
            'frameworks at the system core layer, users no longer interact with '
            'isolated platforms, but rather text environments that shift dynamically '
            'based on temporary workflow requirements\u2026',
            style: TextStyle(
              fontSize: 12,
              height: 1.75,
              color: Colors.white.withValues(alpha: 0.45),
            ),
          ),
        ],
      ),
    );
  }
}

class _AutoSaveBadge extends StatefulWidget {
  const _AutoSaveBadge();

  @override
  State<_AutoSaveBadge> createState() => _AutoSaveBadgeState();
}

class _AutoSaveBadgeState extends State<_AutoSaveBadge>
    with SingleTickerProviderStateMixin {
  late final AnimationController _ctrl;
  late final Animation<double> _pulse;

  @override
  void initState() {
    super.initState();
    _ctrl = AnimationController(
      vsync: this,
      duration: const Duration(milliseconds: 1800),
    )..repeat(reverse: true);
    _pulse = Tween(begin: 1.0, end: 0.4).animate(
      CurvedAnimation(parent: _ctrl, curve: Curves.easeInOut),
    );
  }

  @override
  void dispose() {
    _ctrl.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    return Container(
      padding: const EdgeInsets.symmetric(horizontal: 10, vertical: 4),
      decoration: BoxDecoration(
        borderRadius: BorderRadius.circular(999),
        border: Border.all(color: _kSignalGreen.withValues(alpha: 0.20)),
        color: _kSignalGreen.withValues(alpha: 0.10),
      ),
      child: Row(
        mainAxisSize: MainAxisSize.min,
        children: [
          AnimatedBuilder(
            animation: _pulse,
            builder: (_, child) => Opacity(
              opacity: _pulse.value,
              child: Transform.scale(
                scale: 0.85 + (_pulse.value * 0.15),
                child: child,
              ),
            ),
            child: Container(
              width: 6,
              height: 6,
              decoration: const BoxDecoration(
                shape: BoxShape.circle,
                color: _kSignalGreen,
              ),
            ),
          ),
          const SizedBox(width: 6),
          const Text(
            'Auto-saving',
            style: TextStyle(
              fontSize: 9,
              fontWeight: FontWeight.w500,
              color: _kSignalGreen,
            ),
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
    return Container(
      padding: const EdgeInsets.all(16),
      decoration: BoxDecoration(
        borderRadius: BorderRadius.circular(14),
        border: Border.all(color: _kBorder),
        color: _kSurfaceCard,
        boxShadow: const [
          BoxShadow(
            color: Color(0x80000000),
            blurRadius: 24,
            offset: Offset(0, 8),
            spreadRadius: -12,
          ),
        ],
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Text(
            'NOW PLAYING',
            style: TextStyle(
              fontFamily: 'RobotoMono',
              fontSize: 9,
              fontWeight: FontWeight.w600,
              letterSpacing: 1.2,
              color: Colors.white.withValues(alpha: 0.35),
            ),
          ),
          const SizedBox(height: 10),
          Row(
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              Expanded(
                child: Column(
                  crossAxisAlignment: CrossAxisAlignment.start,
                  children: [
                    const Text(
                      'Ambient Waves',
                      style: TextStyle(
                        fontSize: 13,
                        fontWeight: FontWeight.w500,
                        color: Color(0xFFE5E5E5),
                      ),
                    ),
                    const SizedBox(height: 3),
                    Text(
                      'Cortex Generative Radio',
                      style: TextStyle(
                        fontSize: 11,
                        color: Colors.white.withValues(alpha: 0.35),
                      ),
                    ),
                  ],
                ),
              ),
              const _WaveformBars(),
            ],
          ),
          const SizedBox(height: 12),
          Wrap(
            spacing: 6,
            runSpacing: 4,
            children: [
              _TagChip(
                label: 'Local Sync',
                bg: Colors.white.withValues(alpha: 0.04),
                border: Colors.white.withValues(alpha: 0.05),
                textColor: Colors.white.withValues(alpha: 0.45),
              ),
              _TagChip(
                label: 'Spatial Audio Active',
                bg: _kAccentViolet.withValues(alpha: 0.10),
                border: _kAccentViolet.withValues(alpha: 0.20),
                textColor: _kAccentVioletLight,
              ),
            ],
          ),
        ],
      ),
    );
  }
}

class _TagChip extends StatelessWidget {
  const _TagChip({
    required this.label,
    required this.bg,
    required this.border,
    required this.textColor,
  });

  final String label;
  final Color bg;
  final Color border;
  final Color textColor;

  @override
  Widget build(BuildContext context) {
    return Container(
      padding: const EdgeInsets.symmetric(horizontal: 8, vertical: 3),
      decoration: BoxDecoration(
        color: bg,
        borderRadius: BorderRadius.circular(6),
        border: Border.all(color: border),
      ),
      child: Text(
        label,
        style: TextStyle(
          fontFamily: 'RobotoMono',
          fontSize: 9,
          color: textColor,
        ),
      ),
    );
  }
}

// ─── Waveform Bars (animated) ──────────────────────────────────────────
class _WaveformBars extends StatefulWidget {
  const _WaveformBars();

  @override
  State<_WaveformBars> createState() => _WaveformBarsState();
}

class _WaveformBarsState extends State<_WaveformBars>
    with SingleTickerProviderStateMixin {
  late final AnimationController _ctrl;

  static const _barHeights = [10.0, 20.0, 12.0, 24.0, 16.0, 28.0, 18.0, 10.0];
  static const _barDelays = [0.0, 0.08, 0.16, 0.24, 0.32, 0.40, 0.48, 0.56];

  @override
  void initState() {
    super.initState();
    _ctrl = AnimationController(
      vsync: this,
      duration: const Duration(milliseconds: 1200),
    )..repeat(reverse: true);
  }

  @override
  void dispose() {
    _ctrl.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    return SizedBox(
      height: 32,
      child: Row(
        mainAxisSize: MainAxisSize.min,
        crossAxisAlignment: CrossAxisAlignment.end,
        children: List.generate(8, (i) {
          return AnimatedBuilder(
            animation: _ctrl,
            builder: (_, _) {
              final phase = (_ctrl.value + _barDelays[i]) % 1.0;
              final scale = 0.3 + 0.7 * math.sin(phase * math.pi);
              return Container(
                width: 3,
                height: _barHeights[i] * scale,
                margin: const EdgeInsets.only(left: 3),
                decoration: BoxDecoration(
                  borderRadius: BorderRadius.circular(999),
                  gradient: const LinearGradient(
                    begin: Alignment.bottomCenter,
                    end: Alignment.topCenter,
                    colors: [Color(0xFF7C3AED), Color(0xFFA78BFA)],
                  ),
                ),
              );
            },
          );
        }),
      ),
    );
  }
}

// ─── Knowledge Source Card ──────────────────────────────────────────────
class _KnowledgeSourceCard extends StatelessWidget {
  const _KnowledgeSourceCard();

  @override
  Widget build(BuildContext context) {
    return Container(
      padding: const EdgeInsets.all(16),
      decoration: BoxDecoration(
        borderRadius: BorderRadius.circular(14),
        border: Border.all(color: _kBorder),
        color: _kSurfaceCard,
        boxShadow: const [
          BoxShadow(
            color: Color(0x80000000),
            blurRadius: 24,
            offset: Offset(0, 8),
            spreadRadius: -12,
          ),
        ],
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Text(
            'KNOWLEDGE SOURCE ENGINE',
            style: TextStyle(
              fontFamily: 'RobotoMono',
              fontSize: 9,
              fontWeight: FontWeight.w600,
              letterSpacing: 1.2,
              color: Colors.white.withValues(alpha: 0.35),
            ),
          ),
          const SizedBox(height: 8),
          const Text(
            'https://en.wikipedia.org/wiki/Operating_system',
            style: TextStyle(
              fontFamily: 'RobotoMono',
              fontSize: 10,
              color: _kSkyBlue,
            ),
            overflow: TextOverflow.ellipsis,
          ),
          const SizedBox(height: 6),
          const Text(
            'Operating Systems Architecture Evolution',
            style: TextStyle(
              fontSize: 12,
              fontWeight: FontWeight.w500,
              color: Color(0xFFE5E5E5),
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
class _GeneratedEngineCard extends StatefulWidget {
  const _GeneratedEngineCard();

  @override
  State<_GeneratedEngineCard> createState() => _GeneratedEngineCardState();
}

class _GeneratedEngineCardState extends State<_GeneratedEngineCard>
    with SingleTickerProviderStateMixin {
  late final AnimationController _ctrl;
  late final Animation<double> _pulse;

  @override
  void initState() {
    super.initState();
    _ctrl = AnimationController(
      vsync: this,
      duration: const Duration(milliseconds: 1800),
    )..repeat(reverse: true);
    _pulse = Tween(begin: 1.0, end: 0.4).animate(
      CurvedAnimation(parent: _ctrl, curve: Curves.easeInOut),
    );
  }

  @override
  void dispose() {
    _ctrl.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    return Container(
      padding: const EdgeInsets.all(16),
      decoration: BoxDecoration(
        borderRadius: BorderRadius.circular(14),
        border: Border.all(color: _kActiveBorder),
        gradient: const LinearGradient(
          begin: Alignment.centerLeft,
          end: Alignment.centerRight,
          colors: [Color(0x127C3AED), _kSurfaceCard],
        ),
        boxShadow: const [
          BoxShadow(
            color: Color(0x80000000),
            blurRadius: 24,
            offset: Offset(0, 8),
            spreadRadius: -12,
          ),
        ],
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Row(
            mainAxisAlignment: MainAxisAlignment.spaceBetween,
            children: [
              Text(
                'GENERATED ENGINE VIEW',
                style: TextStyle(
                  fontFamily: 'RobotoMono',
                  fontSize: 9,
                  fontWeight: FontWeight.w600,
                  letterSpacing: 1.2,
                  color: Colors.white.withValues(alpha: 0.35),
                ),
              ),
              Container(
                padding:
                    const EdgeInsets.symmetric(horizontal: 8, vertical: 3),
                decoration: BoxDecoration(
                  borderRadius: BorderRadius.circular(6),
                  color: _kAccentViolet.withValues(alpha: 0.10),
                  border: Border.all(
                      color: _kAccentViolet.withValues(alpha: 0.25)),
                ),
                child: Row(
                  mainAxisSize: MainAxisSize.min,
                  children: [
                    AnimatedBuilder(
                      animation: _pulse,
                      builder: (_, child) => Opacity(
                        opacity: _pulse.value,
                        child: child,
                      ),
                      child: Container(
                        width: 6,
                        height: 6,
                        decoration: const BoxDecoration(
                          shape: BoxShape.circle,
                          color: _kAccentVioletLight,
                        ),
                      ),
                    ),
                    const SizedBox(width: 5),
                    const Text(
                      'Live',
                      style: TextStyle(
                        fontSize: 9,
                        fontWeight: FontWeight.w500,
                        color: _kAccentVioletLight,
                      ),
                    ),
                  ],
                ),
              ),
            ],
          ),
          const SizedBox(height: 10),
          Text(
            'Injected general-purpose widget context directly into active '
            'space. Render layout optimized natively.',
            style: TextStyle(
              fontSize: 12,
              height: 1.6,
              color: Colors.white.withValues(alpha: 0.45),
            ),
          ),
        ],
      ),
    );
  }
}

// ─── Hub Dock (glassmorphic bottom bar) ────────────────────────────────
class _HubDock extends StatelessWidget {
  const _HubDock({
    required this.canvas,
    required this.files,
    required this.web,
    required this.active,
    required this.onActivate,
    required this.onLaunch,
  });

  final AppTile? canvas;
  final AppTile? files;
  final AppTile? web;
  final CoreAppRole active;
  final ValueChanged<CoreAppRole> onActivate;
  final ValueChanged<AppTile> onLaunch;

  @override
  Widget build(BuildContext context) {
    return Padding(
      padding: const EdgeInsets.fromLTRB(16, 6, 16, 16),
      child: Center(
        child: Container(
          decoration: BoxDecoration(
            color: const Color(0xC70E0E12),
            borderRadius: BorderRadius.circular(999),
            border: Border.all(color: Colors.white.withValues(alpha: 0.10)),
            boxShadow: const [
              BoxShadow(
                color: Color(0x99000000),
                blurRadius: 40,
                offset: Offset(0, 12),
                spreadRadius: -8,
              ),
            ],
          ),
          child: Padding(
            padding: const EdgeInsets.all(5),
            child: Row(
              mainAxisSize: MainAxisSize.min,
              children: [
                _HubSegment(
                  label: 'Canvas Hub',
                  app: canvas,
                  role: CoreAppRole.canvas,
                  active: active,
                  onActivate: onActivate,
                  onLaunch: onLaunch,
                ),
                const SizedBox(width: 2),
                _HubSegment(
                  label: 'Files',
                  app: files,
                  role: CoreAppRole.files,
                  active: active,
                  onActivate: onActivate,
                  onLaunch: onLaunch,
                ),
                const SizedBox(width: 2),
                _HubSegment(
                  label: 'Web Link',
                  app: web,
                  role: CoreAppRole.web,
                  active: active,
                  onActivate: onActivate,
                  onLaunch: onLaunch,
                ),
              ],
            ),
          ),
        ),
      ),
    );
  }
}

class _HubSegment extends StatefulWidget {
  const _HubSegment({
    required this.label,
    required this.app,
    required this.role,
    required this.active,
    required this.onActivate,
    required this.onLaunch,
  });

  final String label;
  final AppTile? app;
  final CoreAppRole role;
  final CoreAppRole active;
  final ValueChanged<CoreAppRole> onActivate;
  final ValueChanged<AppTile> onLaunch;

  @override
  State<_HubSegment> createState() => _HubSegmentState();
}

class _HubSegmentState extends State<_HubSegment> {
  bool _hovered = false;

  @override
  Widget build(BuildContext context) {
    final enabled = widget.app != null;
    final selected = widget.active == widget.role;

    final bgAlpha = selected
        ? 0.10
        : _hovered
            ? 0.04
            : enabled
                ? 0.02
                : 0.01;

    final textAlpha = selected
        ? 0.98
        : _hovered
            ? 0.90
            : enabled
                ? 0.74
                : 0.35;

    return MouseRegion(
      onEnter: (_) => setState(() => _hovered = true),
      onExit: (_) => setState(() => _hovered = false),
      child: GestureDetector(
        onTap: () {
          widget.onActivate(widget.role);
          if (enabled) widget.onLaunch(widget.app!);
        },
        child: AnimatedContainer(
          duration: const Duration(milliseconds: 180),
          curve: Curves.easeOut,
          padding: const EdgeInsets.symmetric(horizontal: 16, vertical: 8),
          decoration: BoxDecoration(
            color: Colors.white.withValues(alpha: bgAlpha),
            borderRadius: BorderRadius.circular(999),
          ),
          child: Text(
            widget.label,
            style: TextStyle(
              fontSize: 12,
              fontWeight: selected ? FontWeight.w700 : FontWeight.w500,
              color: Colors.white.withValues(alpha: textAlpha),
            ),
          ),
        ),
      ),
    );
  }
}
