import 'package:flutter/material.dart';
import 'dart:math' as math;

void main() {
  runApp(const CanvasApp());
}

// ─── Design Tokens ───────────────────────────────────────────────────────────

const Color _bodyBg = Color(0xFF07080B);
const Color _canvasBg = Color(0xFF0B0D12);
const Color _surfaceCard = Color(0x06FFFFFF);
const Color _accentViolet = Color(0xFF7C3AED);
const Color _accentVioletLight = Color(0xFFA78BFA);
const Color _signalGreen = Color(0xFF10B981);
const Color _border = Color(0x14FFFFFF);
const Color _textMain = Color(0xFFD4D4D8);
const Color _textMuted = Color(0xFF71717A);

// ─── App Root ────────────────────────────────────────────────────────────────

class CanvasApp extends StatelessWidget {
  const CanvasApp({super.key});

  @override
  Widget build(BuildContext context) {
    return MaterialApp(
      debugShowCheckedModeBanner: false,
      theme: ThemeData(
        brightness: Brightness.dark,
        fontFamily: 'NotoSans',
        scaffoldBackgroundColor: _bodyBg,
        colorScheme: const ColorScheme.dark(
          primary: _accentViolet,
          secondary: _accentVioletLight,
          surface: _canvasBg,
        ),
        useMaterial3: true,
      ),
      home: const CanvasHome(),
    );
  }
}

// ─── Home Screen ─────────────────────────────────────────────────────────────

class CanvasHome extends StatefulWidget {
  const CanvasHome({super.key});

  @override
  State<CanvasHome> createState() => _CanvasHomeState();
}

class _CanvasHomeState extends State<CanvasHome> with TickerProviderStateMixin {
  late final AnimationController _autoSavePulse;
  late final AnimationController _livePulse;

  @override
  void initState() {
    super.initState();
    _autoSavePulse = AnimationController(
      vsync: this,
      duration: const Duration(milliseconds: 1800),
    )..repeat(reverse: true);
    _livePulse = AnimationController(
      vsync: this,
      duration: const Duration(milliseconds: 1800),
    )..repeat(reverse: true);
  }

  @override
  void dispose() {
    _autoSavePulse.dispose();
    _livePulse.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    return Scaffold(
      backgroundColor: _bodyBg,
      body: Container(
        decoration: const BoxDecoration(
          gradient: RadialGradient(
            center: Alignment.topLeft,
            radius: 1.4,
            colors: [Color(0x332E1065), _bodyBg],
          ),
        ),
        child: SafeArea(
          child: LayoutBuilder(
            builder: (context, constraints) {
              return SingleChildScrollView(
                padding: const EdgeInsets.symmetric(
                  horizontal: 32,
                  vertical: 28,
                ),
                child: ConstrainedBox(
                  constraints: BoxConstraints(
                    minHeight: constraints.maxHeight - 56,
                  ),
                  child: Column(
                    crossAxisAlignment: CrossAxisAlignment.start,
                    children: [
                      _buildHeader(),
                      const SizedBox(height: 28),
                      _buildDocumentCanvasCard(),
                      const SizedBox(height: 20),
                      _buildRuledEditorArea(),
                      const SizedBox(height: 20),
                      _buildEngineViewCard(),
                      const SizedBox(height: 28),
                    ],
                  ),
                ),
              );
            },
          ),
        ),
      ),
    );
  }

  // ── Header ──

  Widget _buildHeader() {
    return Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        const Text(
          'Canvas',
          style: TextStyle(
            fontSize: 32,
            fontWeight: FontWeight.w800,
            color: _textMain,
            letterSpacing: -0.5,
            height: 1.1,
          ),
        ),
        const SizedBox(height: 6),
        Text(
          'Personal document surface — compose, ideate, render.',
          style: TextStyle(
            fontSize: 14,
            fontWeight: FontWeight.w400,
            color: _textMuted.withValues(alpha: 0.7),
            letterSpacing: 0.1,
          ),
        ),
      ],
    );
  }

  // ── Document Canvas Card ──

  Widget _buildDocumentCanvasCard() {
    return Container(
      decoration: BoxDecoration(
        color: _surfaceCard,
        borderRadius: BorderRadius.circular(20),
        border: Border.all(color: _border),
      ),
      child: Padding(
        padding: const EdgeInsets.all(24),
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            // Label row
            Row(
              children: [
                Container(
                  width: 3,
                  height: 14,
                  decoration: BoxDecoration(
                    color: _accentViolet,
                    borderRadius: BorderRadius.circular(2),
                  ),
                ),
                const SizedBox(width: 10),
                const Text(
                  'DOCUMENT CANVAS',
                  style: TextStyle(
                    fontSize: 11,
                    fontWeight: FontWeight.w700,
                    color: _accentVioletLight,
                    letterSpacing: 1.6,
                    fontFamily: 'RobotoMono',
                  ),
                ),
              ],
            ),
            const SizedBox(height: 16),

            // Title row with auto-save badge
            Row(
              crossAxisAlignment: CrossAxisAlignment.center,
              children: [
                const Expanded(
                  child: Text(
                    'Project Proposal.md',
                    style: TextStyle(
                      fontSize: 20,
                      fontWeight: FontWeight.w700,
                      color: _textMain,
                      letterSpacing: -0.3,
                    ),
                  ),
                ),
                _buildAutoSaveBadge(),
              ],
            ),
            const SizedBox(height: 20),

            // Divider
            Container(
              height: 1,
              color: _border,
            ),
            const SizedBox(height: 20),

            // Body text
            const Text(
              'OSCortex is a bare-metal operating system designed from the ground up '
              'for a post-cloud era. The kernel provides first-class process isolation, '
              'a virtual filesystem layer, and a Flutter-based compositor that renders '
              'every userspace application as a native surface. Each app runs as its own '
              'PID with message-channel IPC, ensuring zero shared state between processes.',
              style: TextStyle(
                fontSize: 14,
                fontWeight: FontWeight.w400,
                color: _textMuted,
                height: 1.7,
                letterSpacing: 0.15,
              ),
            ),
            const SizedBox(height: 16),
            const Text(
              'This proposal outlines the milestone roadmap for v1.0 — covering kernel '
              'hardening, driver model stabilization, the app runtime sandbox, and the '
              'full design-system rollout across all bundled applications.',
              style: TextStyle(
                fontSize: 14,
                fontWeight: FontWeight.w400,
                color: _textMuted,
                height: 1.7,
                letterSpacing: 0.15,
              ),
            ),
          ],
        ),
      ),
    );
  }

  // ── Auto-save Badge ──

  Widget _buildAutoSaveBadge() {
    return AnimatedBuilder(
      animation: _autoSavePulse,
      builder: (context, child) {
        final double eased = _easeInOut(_autoSavePulse.value);
        final double opacity = 0.5 + 0.5 * eased;
        final double scale = 1.0 + 0.35 * eased;
        return Container(
          padding: const EdgeInsets.symmetric(horizontal: 10, vertical: 5),
          decoration: BoxDecoration(
            color: _signalGreen.withValues(alpha: 0.1),
            borderRadius: BorderRadius.circular(20),
            border: Border.all(
              color: _signalGreen.withValues(alpha: 0.2),
            ),
          ),
          child: Row(
            mainAxisSize: MainAxisSize.min,
            children: [
              SizedBox(
                width: 14,
                height: 14,
                child: Stack(
                  alignment: Alignment.center,
                  children: [
                    // Pulse ring
                    Transform.scale(
                      scale: scale,
                      child: Container(
                        width: 8,
                        height: 8,
                        decoration: BoxDecoration(
                          shape: BoxShape.circle,
                          color: _signalGreen.withValues(
                            alpha: 0.24 * (1.0 - eased),
                          ),
                        ),
                      ),
                    ),
                    // Core dot
                    Opacity(
                      opacity: opacity,
                      child: Container(
                        width: 6,
                        height: 6,
                        decoration: const BoxDecoration(
                          shape: BoxShape.circle,
                          color: _signalGreen,
                        ),
                      ),
                    ),
                  ],
                ),
              ),
              const SizedBox(width: 6),
              const Text(
                'Auto-saving',
                style: TextStyle(
                  fontSize: 11,
                  fontWeight: FontWeight.w600,
                  color: _signalGreen,
                  letterSpacing: 0.3,
                ),
              ),
            ],
          ),
        );
      },
    );
  }

  // ── Ruled-Line Editor Area ──

  Widget _buildRuledEditorArea() {
    final List<_EditorLine> lines = [
      const _EditorLine(1, '## Architecture Overview', true),
      const _EditorLine(2, '', false),
      const _EditorLine(3, 'The kernel boots into a minimal init sequence that', false),
      const _EditorLine(4, 'hands off to the Flutter compositor within 1.2s on', false),
      const _EditorLine(5, 'target hardware. Process scheduling uses a priority', false),
      const _EditorLine(6, 'queue with preemptive multitasking for UI threads.', false),
      const _EditorLine(7, '', false),
      const _EditorLine(8, '## Driver Model', true),
      const _EditorLine(9, '', false),
      const _EditorLine(10, 'Drivers are loaded as WASM modules via the CDP', false),
      const _EditorLine(11, 'interface, sandboxed from kernel memory space. Each', false),
      const _EditorLine(12, 'driver registers capabilities through typed syscalls.', false),
    ];

    return Container(
      decoration: BoxDecoration(
        color: _surfaceCard,
        borderRadius: BorderRadius.circular(20),
        border: Border.all(color: _border),
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          // Editor toolbar
          Container(
            padding: const EdgeInsets.symmetric(horizontal: 20, vertical: 12),
            decoration: const BoxDecoration(
              border: Border(
                bottom: BorderSide(color: _border),
              ),
            ),
            child: Row(
              children: [
                Icon(
                  Icons.edit_note_rounded,
                  size: 16,
                  color: _accentVioletLight.withValues(alpha: 0.7),
                ),
                const SizedBox(width: 8),
                const Text(
                  'Editor',
                  style: TextStyle(
                    fontSize: 12,
                    fontWeight: FontWeight.w600,
                    color: _textMuted,
                    letterSpacing: 0.5,
                  ),
                ),
                const Spacer(),
                Text(
                  'Ln 6, Col 48',
                  style: TextStyle(
                    fontSize: 11,
                    fontWeight: FontWeight.w500,
                    color: _textMuted.withValues(alpha: 0.55),
                    fontFamily: 'RobotoMono',
                    letterSpacing: 0.3,
                  ),
                ),
                const SizedBox(width: 16),
                Text(
                  'UTF-8',
                  style: TextStyle(
                    fontSize: 11,
                    fontWeight: FontWeight.w500,
                    color: _textMuted.withValues(alpha: 0.55),
                    fontFamily: 'RobotoMono',
                    letterSpacing: 0.3,
                  ),
                ),
              ],
            ),
          ),
          // Lines
          Padding(
            padding: const EdgeInsets.symmetric(vertical: 8),
            child: Column(
              children: lines.map((line) => _buildEditorLine(line)).toList(),
            ),
          ),
        ],
      ),
    );
  }

  Widget _buildEditorLine(_EditorLine line) {
    return Container(
      padding: const EdgeInsets.symmetric(horizontal: 20, vertical: 0),
      height: 28,
      child: Row(
        crossAxisAlignment: CrossAxisAlignment.center,
        children: [
          // Line number
          SizedBox(
            width: 28,
            child: Text(
              '${line.number}',
              textAlign: TextAlign.right,
              style: TextStyle(
                fontSize: 12,
                fontWeight: FontWeight.w400,
                color: _textMuted.withValues(alpha: 0.4),
                fontFamily: 'RobotoMono',
              ),
            ),
          ),
          const SizedBox(width: 16),
          // Gutter line
          Container(
            width: 1,
            height: 28,
            color: _border,
          ),
          const SizedBox(width: 16),
          // Content
          Expanded(
            child: line.text.isEmpty
                ? const SizedBox.shrink()
                : Text(
                    line.text,
                    maxLines: 1,
                    overflow: TextOverflow.ellipsis,
                    style: TextStyle(
                      fontSize: 13,
                      fontWeight: line.isHeading ? FontWeight.w700 : FontWeight.w400,
                      color: line.isHeading ? _accentVioletLight : _textMuted,
                      fontFamily: 'RobotoMono',
                      height: 1.0,
                      letterSpacing: 0.2,
                    ),
                  ),
          ),
        ],
      ),
    );
  }

  // ── Generated Engine View Card ──

  Widget _buildEngineViewCard() {
    return Container(
      decoration: BoxDecoration(
        color: _surfaceCard,
        borderRadius: BorderRadius.circular(20),
        border: Border.all(color: _border),
      ),
      child: Padding(
        padding: const EdgeInsets.all(24),
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            // Label row
            Row(
              children: [
                Container(
                  width: 3,
                  height: 14,
                  decoration: BoxDecoration(
                    color: _accentViolet,
                    borderRadius: BorderRadius.circular(2),
                  ),
                ),
                const SizedBox(width: 10),
                const Text(
                  'GENERATED ENGINE VIEW',
                  style: TextStyle(
                    fontSize: 11,
                    fontWeight: FontWeight.w700,
                    color: _accentVioletLight,
                    letterSpacing: 1.6,
                    fontFamily: 'RobotoMono',
                  ),
                ),
                const Spacer(),
                _buildLiveBadge(),
              ],
            ),
            const SizedBox(height: 16),

            // Divider
            Container(height: 1, color: _border),
            const SizedBox(height: 16),

            // Engine preview surface
            Container(
              width: double.infinity,
              padding: const EdgeInsets.all(20),
              decoration: BoxDecoration(
                color: _canvasBg,
                borderRadius: BorderRadius.circular(14),
                border: Border.all(
                  color: _accentViolet.withValues(alpha: 0.12),
                ),
              ),
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  Row(
                    children: [
                      Icon(
                        Icons.auto_awesome_rounded,
                        size: 16,
                        color: _accentVioletLight.withValues(alpha: 0.7),
                      ),
                      const SizedBox(width: 8),
                      const Text(
                        'Render Pipeline Active',
                        style: TextStyle(
                          fontSize: 13,
                          fontWeight: FontWeight.w600,
                          color: _textMain,
                          letterSpacing: 0.2,
                        ),
                      ),
                    ],
                  ),
                  const SizedBox(height: 12),
                  const Text(
                    'The engine view compiles document structure into a live-rendered '
                    'surface. Changes propagate through the compositor pipeline in real '
                    'time, producing frame-accurate previews of the final output.',
                    style: TextStyle(
                      fontSize: 13,
                      fontWeight: FontWeight.w400,
                      color: _textMuted,
                      height: 1.65,
                      letterSpacing: 0.1,
                    ),
                  ),
                  const SizedBox(height: 16),
                  // Stats row
                  Row(
                    children: [
                      _buildStatChip('Latency', '4ms'),
                      const SizedBox(width: 10),
                      _buildStatChip('FPS', '60'),
                      const SizedBox(width: 10),
                      _buildStatChip('Nodes', '142'),
                    ],
                  ),
                ],
              ),
            ),
          ],
        ),
      ),
    );
  }

  Widget _buildStatChip(String label, String value) {
    return Container(
      padding: const EdgeInsets.symmetric(horizontal: 10, vertical: 5),
      decoration: BoxDecoration(
        color: _surfaceCard,
        borderRadius: BorderRadius.circular(8),
        border: Border.all(color: _border),
      ),
      child: Row(
        mainAxisSize: MainAxisSize.min,
        children: [
          Text(
            label,
            style: TextStyle(
              fontSize: 10,
              fontWeight: FontWeight.w500,
              color: _textMuted.withValues(alpha: 0.63),
              letterSpacing: 0.5,
            ),
          ),
          const SizedBox(width: 6),
          Text(
            value,
            style: const TextStyle(
              fontSize: 11,
              fontWeight: FontWeight.w700,
              color: _signalGreen,
              fontFamily: 'RobotoMono',
              letterSpacing: 0.3,
            ),
          ),
        ],
      ),
    );
  }

  // ── Live Badge ──

  Widget _buildLiveBadge() {
    return AnimatedBuilder(
      animation: _livePulse,
      builder: (context, child) {
        final double eased = _easeInOut(_livePulse.value);
        final double opacity = 0.5 + 0.5 * eased;
        final double scale = 1.0 + 0.4 * eased;
        return Container(
          padding: const EdgeInsets.symmetric(horizontal: 10, vertical: 4),
          decoration: BoxDecoration(
            color: _accentViolet.withValues(alpha: 0.1),
            borderRadius: BorderRadius.circular(20),
            border: Border.all(
              color: _accentViolet.withValues(alpha: 0.2),
            ),
          ),
          child: Row(
            mainAxisSize: MainAxisSize.min,
            children: [
              SizedBox(
                width: 14,
                height: 14,
                child: Stack(
                  alignment: Alignment.center,
                  children: [
                    Transform.scale(
                      scale: scale,
                      child: Container(
                        width: 8,
                        height: 8,
                        decoration: BoxDecoration(
                          shape: BoxShape.circle,
                          color: _accentViolet.withValues(
                            alpha: 0.24 * (1.0 - eased),
                          ),
                        ),
                      ),
                    ),
                    Opacity(
                      opacity: opacity,
                      child: Container(
                        width: 6,
                        height: 6,
                        decoration: const BoxDecoration(
                          shape: BoxShape.circle,
                          color: _accentViolet,
                        ),
                      ),
                    ),
                  ],
                ),
              ),
              const SizedBox(width: 6),
              const Text(
                'Live',
                style: TextStyle(
                  fontSize: 11,
                  fontWeight: FontWeight.w600,
                  color: _accentVioletLight,
                  letterSpacing: 0.3,
                ),
              ),
            ],
          ),
        );
      },
    );
  }

  // ── Easing helper ──

  double _easeInOut(double t) {
    return 0.5 - 0.5 * math.cos(t * math.pi);
  }
}

// ─── Data Models ─────────────────────────────────────────────────────────────

class _EditorLine {
  final int number;
  final String text;
  final bool isHeading;

  const _EditorLine(this.number, this.text, this.isHeading);
}
