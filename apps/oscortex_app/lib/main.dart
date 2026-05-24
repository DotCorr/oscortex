import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'dart:math' as math;

void main() {
  runApp(const OSCortexApp());
}

class OSCortexApp extends StatelessWidget {
  const OSCortexApp({super.key});

  @override
  Widget build(BuildContext context) {
    return MaterialApp(
      title: 'OSCortex v1',
      debugShowCheckedModeBanner: false,
      theme: ThemeData.dark().copyWith(
        scaffoldBackgroundColor: const Color(0xFF060A0F),
        colorScheme: const ColorScheme.dark(
          primary: Color(0xFF3B82F6),
          secondary: Color(0xFF8B5CF6),
          surface: Color(0xFF0D1117),
        ),
      ),
      home: const LauncherScreen(),
    );
  }
}

// ── App registry ──────────────────────────────────────────────────────────────

class AppEntry {
  final String id;
  final String name;
  final IconData icon;
  final Color color;
  final Color bgColor;
  final String description;
  const AppEntry(this.id, this.name, this.icon, this.color, this.bgColor,
      this.description);
}

const _apps = [
  AppEntry('terminal', 'Terminal', Icons.terminal,
      Color(0xFF3FB950), Color(0xFF0D2614), 'Shell · Bash · System'),
  AppEntry('files', 'Files', Icons.folder_open,
      Color(0xFF79C0FF), Color(0xFF0C1E2E), 'Browse · Manage · Copy'),
  AppEntry('settings', 'Settings', Icons.settings,
      Color(0xFF8B949E), Color(0xFF161B22), 'Configure · Display · Net'),
  AppEntry('editor', 'Editor', Icons.code,
      Color(0xFFF78166), Color(0xFF2B1012), 'Code · Edit · Syntax'),
  AppEntry('flutter', 'Flutter', Icons.flutter_dash,
      Color(0xFF3B82F6), Color(0xFF0A1628), 'UI · Widgets · Hot Reload'),
  AppEntry('about', 'About', Icons.info_outline,
      Color(0xFFC084FC), Color(0xFF1A0D2E), 'v1.0 · OSCortex · AI-First'),
];

// ── Main launcher screen ──────────────────────────────────────────────────────

class LauncherScreen extends StatefulWidget {
  const LauncherScreen({super.key});
  @override
  State<LauncherScreen> createState() => _LauncherScreenState();
}

class _LauncherScreenState extends State<LauncherScreen>
    with TickerProviderStateMixin {
  int _hovered = -1;
  late AnimationController _bgController;
  late AnimationController _gridController;
  final _launchChannel = const MethodChannel('oscortex/launch');

  @override
  void initState() {
    super.initState();
    _bgController = AnimationController(
      vsync: this,
      duration: const Duration(seconds: 20),
    )..repeat();
    _gridController = AnimationController(
      vsync: this,
      duration: const Duration(milliseconds: 600),
    )..forward();
  }

  @override
  void dispose() {
    _bgController.dispose();
    _gridController.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    return Scaffold(
      body: Stack(
        children: [
          // Animated gradient background
          AnimatedBuilder(
            animation: _bgController,
            builder: (_, __) => _buildBackground(_bgController.value),
          ),
          // Main layout
          Column(
            children: [
              _buildTaskbar(),
              Expanded(child: _buildDesktop()),
              _buildDock(),
            ],
          ),
        ],
      ),
    );
  }

  // ── Background ──────────────────────────────────────────────────────────────

  Widget _buildBackground(double t) {
    final angle = t * 2 * math.pi;
    return Container(
      decoration: BoxDecoration(
        gradient: RadialGradient(
          center: Alignment(
            math.sin(angle) * 0.3,
            math.cos(angle * 0.7) * 0.2,
          ),
          radius: 1.4,
          colors: const [
            Color(0xFF0D1525),
            Color(0xFF060A0F),
            Color(0xFF030508),
          ],
          stops: const [0.0, 0.6, 1.0],
        ),
      ),
    );
  }

  // ── Taskbar ─────────────────────────────────────────────────────────────────

  Widget _buildTaskbar() {
    return Container(
      height: 44,
      decoration: BoxDecoration(
        color: const Color(0x99111827),
        border: const Border(
          bottom: BorderSide(color: Color(0x332D3748), width: 1),
        ),
        boxShadow: [
          BoxShadow(
            color: const Color(0xFF000000).withOpacity(0.4),
            blurRadius: 16,
          ),
        ],
      ),
      child: Row(
        children: [
          // Logo + name
          const SizedBox(width: 16),
          Container(
            width: 26,
            height: 26,
            decoration: BoxDecoration(
              gradient: const LinearGradient(
                begin: Alignment.topLeft,
                end: Alignment.bottomRight,
                colors: [Color(0xFF3B82F6), Color(0xFF8B5CF6)],
              ),
              borderRadius: BorderRadius.circular(6),
              boxShadow: const [
                BoxShadow(
                    color: Color(0x803B82F6), blurRadius: 8, spreadRadius: 1),
              ],
            ),
            child: const Icon(Icons.memory, size: 15, color: Colors.white),
          ),
          const SizedBox(width: 10),
          const Text(
            'OSCortex',
            style: TextStyle(
              color: Color(0xFFE6EDF3),
              fontWeight: FontWeight.w700,
              fontSize: 14,
              letterSpacing: 0.3,
            ),
          ),
          const SizedBox(width: 8),
          Container(
            padding: const EdgeInsets.symmetric(horizontal: 6, vertical: 2),
            decoration: BoxDecoration(
              color: const Color(0x333B82F6),
              borderRadius: BorderRadius.circular(4),
              border: Border.all(color: const Color(0x553B82F6), width: 1),
            ),
            child: const Text(
              'v1.0',
              style: TextStyle(
                color: Color(0xFF3B82F6),
                fontSize: 10,
                fontWeight: FontWeight.w600,
                letterSpacing: 0.5,
              ),
            ),
          ),
          const Spacer(),
          // Status indicators
          _buildStatusChip(Icons.circle, 'Flutter', const Color(0xFF3FB950)),
          const SizedBox(width: 8),
          _buildStatusChip(Icons.circle, 'Dart VM', const Color(0xFF3B82F6)),
          const SizedBox(width: 8),
          _buildStatusChip(Icons.circle, 'AI Cortex', const Color(0xFFC084FC)),
          const SizedBox(width: 16),
          // Window controls
          _buildWinControl(const Color(0xFFFF5F57), Icons.close),
          const SizedBox(width: 6),
          _buildWinControl(const Color(0xFFFFBD2E), Icons.remove),
          const SizedBox(width: 6),
          _buildWinControl(const Color(0xFF28CA41), Icons.fullscreen),
          const SizedBox(width: 16),
        ],
      ),
    );
  }

  Widget _buildStatusChip(IconData icon, String label, Color color) {
    return Row(
      mainAxisSize: MainAxisSize.min,
      children: [
        Icon(icon, size: 7, color: color),
        const SizedBox(width: 4),
        Text(
          label,
          style: TextStyle(color: color.withOpacity(0.9), fontSize: 11),
        ),
      ],
    );
  }

  Widget _buildWinControl(Color color, IconData icon) {
    return GestureDetector(
      onTap: () {},
      child: Container(
        width: 13,
        height: 13,
        decoration: BoxDecoration(
          color: color,
          shape: BoxShape.circle,
        ),
      ),
    );
  }

  // ── Desktop grid ─────────────────────────────────────────────────────────────

  Widget _buildDesktop() {
    return Center(
      child: Column(
        mainAxisAlignment: MainAxisAlignment.center,
        children: [
          // Hero text
          AnimatedBuilder(
            animation: _gridController,
            builder: (_, child) => Opacity(
              opacity: _gridController.value,
              child: Transform.translate(
                offset: Offset(0, 20 * (1 - _gridController.value)),
                child: child,
              ),
            ),
            child: Column(
              children: [
                ShaderMask(
                  shaderCallback: (rect) => const LinearGradient(
                    colors: [Color(0xFF3B82F6), Color(0xFF8B5CF6), Color(0xFF3B82F6)],
                  ).createShader(rect),
                  child: const Text(
                    'OSCortex',
                    style: TextStyle(
                      color: Colors.white,
                      fontSize: 32,
                      fontWeight: FontWeight.w800,
                      letterSpacing: -0.5,
                    ),
                  ),
                ),
                const SizedBox(height: 4),
                const Text(
                  'AI-First Operating System  ·  Flutter UI Runtime  ·  v1.0',
                  style: TextStyle(
                    color: Color(0xFF8B949E),
                    fontSize: 12,
                    letterSpacing: 0.3,
                  ),
                ),
              ],
            ),
          ),
          const SizedBox(height: 48),
          // App grid
          AnimatedBuilder(
            animation: _gridController,
            builder: (_, child) => Opacity(
              opacity: _gridController.value,
              child: child,
            ),
            child: Wrap(
              spacing: 16,
              runSpacing: 16,
              alignment: WrapAlignment.center,
              children: List.generate(_apps.length, (i) {
                return TweenAnimationBuilder<double>(
                  tween: Tween(begin: 0.0, end: 1.0),
                  duration: Duration(milliseconds: 400 + i * 80),
                  curve: Curves.easeOutBack,
                  builder: (_, v, child) => Opacity(
                    opacity: v.clamp(0, 1),
                    child: Transform.scale(scale: v.clamp(0, 1), child: child),
                  ),
                  child: _buildAppCard(i),
                );
              }),
            ),
          ),
        ],
      ),
    );
  }

  Widget _buildAppCard(int i) {
    final app = _apps[i];
    final isHov = _hovered == i;

    return MouseRegion(
      onEnter: (_) => setState(() => _hovered = i),
      onExit: (_) => setState(() => _hovered = -1),
      cursor: SystemMouseCursors.click,
      child: GestureDetector(
        onTap: () => _launch(i),
        child: AnimatedContainer(
          duration: const Duration(milliseconds: 180),
          curve: Curves.easeOut,
          width: 128,
          height: 128,
          decoration: BoxDecoration(
            color: isHov ? app.bgColor.withOpacity(0.9) : app.bgColor,
            borderRadius: BorderRadius.circular(16),
            border: Border.all(
              color: isHov ? app.color.withOpacity(0.6) : const Color(0x22FFFFFF),
              width: isHov ? 1.5 : 1.0,
            ),
            boxShadow: isHov
                ? [
                    BoxShadow(
                      color: app.color.withOpacity(0.25),
                      blurRadius: 20,
                      spreadRadius: 2,
                    ),
                  ]
                : [],
          ),
          child: AnimatedScale(
            scale: isHov ? 1.04 : 1.0,
            duration: const Duration(milliseconds: 180),
            child: Column(
              mainAxisAlignment: MainAxisAlignment.center,
              children: [
                Container(
                  width: 48,
                  height: 48,
                  decoration: BoxDecoration(
                    color: app.color.withOpacity(0.12),
                    borderRadius: BorderRadius.circular(12),
                    border: Border.all(
                        color: app.color.withOpacity(0.2), width: 1),
                  ),
                  child: Icon(app.icon, size: 26, color: app.color),
                ),
                const SizedBox(height: 10),
                Text(
                  app.name,
                  style: const TextStyle(
                    color: Color(0xFFE6EDF3),
                    fontSize: 12,
                    fontWeight: FontWeight.w600,
                    letterSpacing: 0.1,
                  ),
                ),
                const SizedBox(height: 3),
                Text(
                  app.description,
                  style: TextStyle(
                    color: const Color(0xFF8B949E).withOpacity(0.8),
                    fontSize: 9,
                    letterSpacing: 0.2,
                  ),
                  textAlign: TextAlign.center,
                ),
              ],
            ),
          ),
        ),
      ),
    );
  }

  // ── Dock ─────────────────────────────────────────────────────────────────────

  Widget _buildDock() {
    return Container(
      height: 60,
      decoration: BoxDecoration(
        color: const Color(0x99111827),
        border: const Border(
          top: BorderSide(color: Color(0x222D3748), width: 1),
        ),
      ),
      child: Row(
        mainAxisAlignment: MainAxisAlignment.center,
        children: [
          for (int i = 0; i < _apps.length; i++) ...[
            _buildDockIcon(i),
            if (i == 2) const SizedBox(width: 12),
          ],
          const SizedBox(width: 24),
          Container(
            width: 1,
            height: 32,
            color: const Color(0x332D3748),
          ),
          const SizedBox(width: 24),
          // Clock / system info
          Column(
            mainAxisAlignment: MainAxisAlignment.center,
            crossAxisAlignment: CrossAxisAlignment.end,
            children: [
              const Text(
                'Flutter · v1.0',
                style: TextStyle(
                  color: Color(0xFF3B82F6),
                  fontSize: 11,
                  fontWeight: FontWeight.w600,
                ),
              ),
              const Text(
                'AI-First OS',
                style: TextStyle(color: Color(0xFF8B949E), fontSize: 10),
              ),
            ],
          ),
          const SizedBox(width: 16),
        ],
      ),
    );
  }

  Widget _buildDockIcon(int i) {
    final app = _apps[i];
    final isHov = _hovered == (1000 + i);
    return MouseRegion(
      onEnter: (_) => setState(() => _hovered = 1000 + i),
      onExit: (_) => setState(() => _hovered = -1),
      cursor: SystemMouseCursors.click,
      child: GestureDetector(
        onTap: () => _launch(i),
        child: Tooltip(
          message: app.name,
          preferBelow: false,
          child: AnimatedContainer(
            duration: const Duration(milliseconds: 150),
            width: 40,
            height: 40,
            margin: const EdgeInsets.symmetric(horizontal: 3),
            decoration: BoxDecoration(
              color: isHov
                  ? app.bgColor
                  : const Color(0xFF1C2128),
              borderRadius: BorderRadius.circular(10),
              border: Border.all(
                color: isHov
                    ? app.color.withOpacity(0.5)
                    : const Color(0x222D3748),
                width: 1,
              ),
              boxShadow: isHov
                  ? [
                      BoxShadow(
                          color: app.color.withOpacity(0.3),
                          blurRadius: 10)
                    ]
                  : [],
            ),
            child: Icon(app.icon, size: 20, color: app.color),
          ),
        ),
      ),
    );
  }

  // ── App launch ───────────────────────────────────────────────────────────────

  void _launch(int index) {
    final app = _apps[index];

    // Show launch notification
    ScaffoldMessenger.of(context).showSnackBar(
      SnackBar(
        content: Row(
          children: [
            Icon(app.icon, color: app.color, size: 16),
            const SizedBox(width: 10),
            Text(
              'Launching ${app.name}…',
              style: const TextStyle(
                  color: Color(0xFFE6EDF3), fontWeight: FontWeight.w500),
            ),
          ],
        ),
        backgroundColor: const Color(0xFF161B22),
        shape: RoundedRectangleBorder(
          borderRadius: BorderRadius.circular(8),
          side: BorderSide(color: app.color.withOpacity(0.3), width: 1),
        ),
        behavior: SnackBarBehavior.floating,
        margin: const EdgeInsets.all(16),
        duration: const Duration(milliseconds: 1500),
      ),
    );

    // Notify kernel via platform channel
    _launchChannel
        .invokeMethod('launch', {'app': app.id, 'name': app.name})
        .catchError((_) {});
  }
}
