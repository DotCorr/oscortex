import 'dart:async';

import 'package:flutter/material.dart';
import 'package:oscortex_ui/oscortex_ui.dart';

import '../models/app_tile.dart';
import '../services/shell_service.dart';

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
      title: 'OSCortex Embedded Hub',
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
          label: 'Setup Hub',
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

// ─── Workspace (Setup Dashboard wizard) ──────────────────────────────────
class _Workspace extends StatefulWidget {
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
  State<_Workspace> createState() => _WorkspaceState();
}

class _WorkspaceState extends State<_Workspace> {
  final List<String> _hardwareLogs = [
    "[INFO] Compositor: Online (1280x800 bpp=32) [OK]",
    "[INFO] Storage: VirtIO-Blk online (vdisk.img: 8 MiB) [OK]",
    "[INFO] Input: XHCI USB-HID router online [OK]",
    "[INFO] Network: VirtIO-Net ready (MAC 52:54:00:12:34:56) [OK]",
  ];
  
  bool _i2cScanning = true;
  bool _canScanning = false;
  Timer? _simTimer;
  int _simStep = 0;

  @override
  void initState() {
    super.initState();
    _simTimer = Timer.periodic(const Duration(seconds: 3), (timer) {
      if (!mounted) return;
      setState(() {
        _simStep++;
        if (_simStep == 1) {
          _hardwareLogs.add("[WARN] I2C: Unknown device detected at address 0x48");
        } else if (_simStep == 2) {
          _hardwareLogs.add("[INFO] Cortex: Analyzing datasheet for I2C 0x48...");
        } else if (_simStep == 3) {
          _hardwareLogs.add("[INFO] Cortex: ADT7410 temperature sensor matched");
        } else if (_simStep == 4) {
          _hardwareLogs.add("[INFO] Cortex: Generating CDP-WASM driver code...");
        } else if (_simStep == 5) {
          _hardwareLogs.add("[INFO] CDP-WASM: Driver compiled: temp_sensor_0.wasm");
          _hardwareLogs.add("[INFO] Device: TempSensor_0 registered successfully [Active]");
          _i2cScanning = false;
          _canScanning = true;
        } else if (_simStep == 6) {
          _hardwareLogs.add("[INFO] CAN: Scanning CAN0 bus at 500 kbps...");
        } else if (_simStep == 7) {
          _hardwareLogs.add("[WARN] CAN: Unknown infotainment protocol packets detected");
        } else if (_simStep == 8) {
          _hardwareLogs.add("[INFO] Cortex: Mapping packets for Toyota CAN bus...");
        } else if (_simStep == 9) {
          _hardwareLogs.add("[INFO] CDP-WASM: Toyota Infotainment driver: can_bus_toyota.wasm");
          _hardwareLogs.add("[INFO] Device: Infotainment interface mapped [Active]");
          _canScanning = false;
          timer.cancel();
        }
      });
    });
  }

  @override
  void dispose() {
    _simTimer?.cancel();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    if (widget.loading) {
      return const Center(
        child: CircularProgressIndicator(color: OscColors.violetLight),
      );
    }
    if (widget.error != null) {
      return Center(
        child: Text(widget.error!, style: const TextStyle(color: OscColors.textMuted)),
      );
    }

    return Padding(
      padding: const EdgeInsets.all(16.0),
      child: Row(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          // Left column: QR code & pairing
          Expanded(
            flex: 4,
            child: _buildPairingPanel(context),
          ),
          const SizedBox(width: 16),
          // Right column: Auto-discovery logs & system info
          Expanded(
            flex: 5,
            child: _buildDiscoveryPanel(),
          ),
        ],
      ),
    );
  }

  Widget _buildPairingPanel(BuildContext context) {
    return OscCard(
      gradient: OscCard.violetGradient,
      padding: const EdgeInsets.all(24),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          OscSectionLabel(
            'OSCORTEX EMBEDDED EDITION',
            color: OscColors.violetLight.withValues(alpha: 0.85),
          ),
          const SizedBox(height: 8),
          const Text(
            'Setup & Connection',
            style: OscTypography.heading3,
          ),
          const SizedBox(height: 24),
          const Center(
            child: _QrCodeWidget(),
          ),
          const SizedBox(height: 24),
          const Center(
            child: Text(
              'Pairing Code: OSC-982-CTX',
              style: TextStyle(
                fontSize: 16,
                fontWeight: FontWeight.bold,
                letterSpacing: 1.5,
                color: OscColors.violetLight,
              ),
            ),
          ),
          const SizedBox(height: 16),
          const Divider(color: OscColors.border, height: 1),
          const SizedBox(height: 16),
          _buildPairingStatusRow(
            label: 'WiFi Status',
            value: 'Connected to AP (OSCortex-Setup)',
            active: true,
          ),
          const SizedBox(height: 12),
          _buildPairingStatusRow(
            label: 'Bridge Server',
            value: 'Listening on port 50051 (TCP)',
            active: true,
          ),
          const SizedBox(height: 12),
          _buildPairingStatusRow(
            label: 'Dashboard Tunnel',
            value: 'Waiting for developer pair...',
            active: false,
          ),
          const Spacer(),
          OscButton(
            label: 'Configure WiFi',
            onPressed: () {
              ScaffoldMessenger.of(context).showSnackBar(
                const SnackBar(content: Text('Scanning for local WiFi access points...')),
              );
            },
          ),
        ],
      ),
    );
  }

  Widget _buildPairingStatusRow({
    required String label,
    required String value,
    required bool active,
  }) {
    return Row(
      children: [
        Container(
          width: 8,
          height: 8,
          decoration: BoxDecoration(
            shape: BoxShape.circle,
            color: active ? OscColors.green : OscColors.textMuted,
          ),
        ),
        const SizedBox(width: 8),
        Text(
          '$label: ',
          style: OscTypography.bodySmall.copyWith(
            fontWeight: FontWeight.bold,
            color: Colors.white70,
          ),
        ),
        Expanded(
          child: Text(
            value,
            style: OscTypography.bodySmall.copyWith(
              color: active ? OscColors.textBright : OscColors.textMuted,
            ),
            overflow: TextOverflow.ellipsis,
          ),
        ),
      ],
    );
  }

  Widget _buildDiscoveryPanel() {
    return OscCard(
      padding: const EdgeInsets.all(20),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          Row(
            mainAxisAlignment: MainAxisAlignment.spaceBetween,
            children: [
              const OscSectionLabel('CORTEX HARDWARE DISCOVERY'),
              OscStatusBadge(
                label: 'AI-Healing Active',
                color: OscColors.green,
              ),
            ],
          ),
          const SizedBox(height: 12),
          Expanded(
            child: Container(
              padding: const EdgeInsets.all(12),
              decoration: BoxDecoration(
                color: Colors.black.withValues(alpha: 0.4),
                borderRadius: BorderRadius.circular(8),
                border: Border.all(color: OscColors.border),
              ),
              child: ListView.builder(
                itemCount: _hardwareLogs.length,
                itemBuilder: (context, index) {
                  final logStr = _hardwareLogs[_hardwareLogs.length - 1 - index];
                  Color color = Colors.white70;
                  if (logStr.contains('[WARN]')) {
                    color = OscColors.violetLight;
                  } else if (logStr.contains('[ERROR]')) {
                    color = Colors.redAccent;
                  } else if (logStr.contains('Active') || logStr.contains('[OK]') || logStr.contains('successfully') || logStr.contains('registered')) {
                    color = OscColors.green;
                  }
                  
                  return Padding(
                    padding: const EdgeInsets.only(bottom: 6.0),
                    child: Text(
                      logStr,
                      style: OscTypography.monoSmall.copyWith(color: color),
                    ),
                  );
                },
              ),
            ),
          ),
          const SizedBox(height: 16),
          Container(
            padding: const EdgeInsets.all(12),
            decoration: BoxDecoration(
              color: Colors.white.withValues(alpha: 0.02),
              borderRadius: BorderRadius.circular(8),
              border: Border.all(color: OscColors.border),
            ),
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                const Text(
                  'Cortex Scan Status',
                  style: TextStyle(
                    fontSize: 12,
                    fontWeight: FontWeight.bold,
                    color: OscColors.textBright,
                  ),
                ),
                const SizedBox(height: 8),
                Row(
                  children: [
                    if (_i2cScanning) ...[
                      const SizedBox(
                        width: 12,
                        height: 12,
                        child: CircularProgressIndicator(
                          strokeWidth: 1.5,
                          color: OscColors.violetLight,
                        ),
                      ),
                      const SizedBox(width: 8),
                      const Text(
                        'Scanning I2C addresses...',
                        style: TextStyle(fontSize: 11, color: OscColors.textMuted),
                      ),
                    ] else if (_canScanning) ...[
                      const SizedBox(
                        width: 12,
                        height: 12,
                        child: CircularProgressIndicator(
                          strokeWidth: 1.5,
                          color: OscColors.violetLight,
                        ),
                      ),
                      const SizedBox(width: 8),
                      const Text(
                        'Mapping CAN bus packet structures...',
                        style: TextStyle(fontSize: 11, color: OscColors.textMuted),
                      ),
                    ] else ...[
                      const Icon(Icons.check_circle_rounded, color: OscColors.green, size: 14),
                      const SizedBox(width: 8),
                      const Text(
                        'All hardware initialized. System ready for hot-push.',
                        style: TextStyle(fontSize: 11, color: OscColors.green),
                      ),
                    ]
                  ],
                ),
              ],
            ),
          ),
        ],
      ),
    );
  }
}

// ─── Stylized QR Code Widget ─────────────────────────────────────────────
class _QrCodeWidget extends StatelessWidget {
  const _QrCodeWidget();

  @override
  Widget build(BuildContext context) {
    final grid = [
      [1,1,1,1,1,1,1,0,1,0,1,1,0,1,1,1,1,1,1,1],
      [1,0,0,0,0,0,1,0,0,1,0,0,0,1,0,0,0,0,0,1],
      [1,0,1,1,1,0,1,0,1,0,1,1,0,1,0,1,1,1,0,1],
      [1,0,1,1,1,0,1,0,1,1,0,0,0,1,0,1,1,1,0,1],
      [1,0,1,1,1,0,1,0,0,0,1,0,1,1,0,1,1,1,0,1],
      [1,0,0,0,0,0,1,0,1,1,0,1,0,1,0,0,0,0,0,1],
      [1,1,1,1,1,1,1,0,1,0,1,0,0,1,1,1,1,1,1,1],
      [0,0,0,0,0,0,0,0,0,1,1,1,0,0,0,0,0,0,0,0],
      [1,1,0,1,0,1,0,1,1,0,0,0,1,1,0,1,0,1,1,0],
      [0,0,1,0,0,1,1,0,1,1,1,0,1,0,0,1,1,0,0,1],
      [1,0,1,1,0,0,0,1,0,0,1,1,0,1,1,0,1,0,1,0],
      [0,0,0,1,1,1,0,1,1,0,1,0,0,0,1,1,1,0,1,1],
      [0,0,0,0,0,0,0,0,1,1,0,1,1,1,0,0,0,0,1,1],
      [1,1,1,1,1,1,1,0,0,1,0,0,1,1,0,1,0,1,0,0],
      [1,0,0,0,0,0,1,0,1,0,1,1,0,0,1,1,0,0,1,1],
      [1,0,1,1,1,0,1,0,0,1,1,0,1,0,1,0,1,1,0,0],
      [1,0,1,1,1,0,1,0,1,0,0,1,0,1,0,1,0,0,1,1],
      [1,0,1,1,1,0,1,0,0,1,1,1,0,0,1,1,1,0,0,1],
      [1,0,0,0,0,0,1,0,1,1,0,0,1,0,1,0,1,0,1,1],
      [1,1,1,1,1,1,1,0,1,0,1,1,1,1,0,1,0,1,0,1]
    ];

    return Container(
      padding: const EdgeInsets.all(12),
      decoration: BoxDecoration(
        color: Colors.white,
        borderRadius: BorderRadius.circular(12),
        boxShadow: [
          BoxShadow(
            color: Colors.black.withValues(alpha: 0.2),
            blurRadius: 10,
            spreadRadius: 2,
          ),
        ],
      ),
      child: SizedBox(
        width: 160,
        height: 160,
        child: GridView.builder(
          physics: const NeverScrollableScrollPhysics(),
          gridDelegate: SliverGridDelegateWithFixedCrossAxisCount(
            crossAxisCount: grid.length,
          ),
          itemCount: grid.length * grid.length,
          itemBuilder: (context, index) {
            final r = index ~/ grid.length;
            final c = index % grid.length;
            final isActive = grid[r][c] == 1;
            return Container(
              color: isActive ? Colors.black : Colors.white,
            );
          },
        ),
      ),
    );
  }
}

// ─── Previews ──────────────────────────────────────────────────────────
@OscPreview(name: 'Stylized QR Code', group: 'Setup Dashboard')
Widget qrCodePreview() {
  return const Padding(
    padding: EdgeInsets.all(24),
    child: _QrCodeWidget(),
  );
}

@OscPreview(name: 'Settings Button', group: 'Setup Chrome')
Widget settingsButtonPreview() {
  return Padding(
    padding: const EdgeInsets.all(16),
    child: _SettingsButton(),
  );
}

