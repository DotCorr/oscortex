import 'package:flutter/material.dart';
import 'package:oscortex_ui/oscortex_ui.dart';

void main() {
  runApp(const CanvasApp());
}

// ─── App Root ────────────────────────────────────────────────────────────────

class CanvasApp extends StatelessWidget {
  const CanvasApp({super.key});

  @override
  Widget build(BuildContext context) {
    return MaterialApp(
      debugShowCheckedModeBanner: false,
      theme: OscTheme.dark(),
      home: const CanvasHome(),
    );
  }
}

// ─── Home Screen ─────────────────────────────────────────────────────────────

class CanvasHome extends StatelessWidget {
  const CanvasHome({super.key});

  @override
  Widget build(BuildContext context) {
    return Scaffold(
      backgroundColor: OscColors.bodyBg,
      body: Container(
        decoration: const BoxDecoration(
          gradient: RadialGradient(
            center: Alignment.topLeft,
            radius: 1.4,
            colors: [Color(0x332E1065), OscColors.bodyBg],
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
        Text(
          'Canvas',
          style: OscTypography.heading1.copyWith(
            color: OscColors.textPrimary,
            letterSpacing: -0.5,
            height: 1.1,
          ),
        ),
        const SizedBox(height: 6),
        Text(
          'Personal document surface — compose, ideate, render.',
          style: OscTypography.caption.copyWith(
            fontSize: 14,
            color: OscColors.textMuted.withValues(alpha: 0.7),
            letterSpacing: 0.1,
          ),
        ),
      ],
    );
  }

  // ── Document Canvas Card ──

  Widget _buildDocumentCanvasCard() {
    return OscCard(
      gradient: OscCard.violetGradient,
      padding: const EdgeInsets.all(24),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          // Label row
          Row(
            children: [
              const OscSectionLabel(
                'DOCUMENT CANVAS',
                color: OscColors.violetLight,
              ),
              const Spacer(),
              const OscStatusBadge(
                label: 'Auto-saving',
                color: OscColors.green,
              ),
            ],
          ),
          const SizedBox(height: 16),

          // Title
          Text(
            'Project Proposal.md',
            style: OscTypography.heading3.copyWith(
              fontSize: 20,
              fontWeight: FontWeight.w700,
              color: OscColors.textPrimary,
              letterSpacing: -0.3,
            ),
          ),
          const SizedBox(height: 20),

          // Divider
          Container(
            height: 1,
            color: OscColors.border,
          ),
          const SizedBox(height: 20),

          // Body text
          Text(
            'OSCortex is a bare-metal operating system designed from the ground up '
            'for a post-cloud era. The kernel provides first-class process isolation, '
            'a virtual filesystem layer, and a Flutter-based compositor that renders '
            'every userspace application as a native surface. Each app runs as its own '
            'PID with message-channel IPC, ensuring zero shared state between processes.',
            style: OscTypography.body.copyWith(
              fontSize: 14,
              fontWeight: FontWeight.w400,
              color: OscColors.textMuted,
              height: 1.7,
              letterSpacing: 0.15,
            ),
          ),
          const SizedBox(height: 16),
          Text(
            'This proposal outlines the milestone roadmap for v1.0 — covering kernel '
            'hardening, driver model stabilization, the app runtime sandbox, and the '
            'full design-system rollout across all bundled applications.',
            style: OscTypography.body.copyWith(
              fontSize: 14,
              fontWeight: FontWeight.w400,
              color: OscColors.textMuted,
              height: 1.7,
              letterSpacing: 0.15,
            ),
          ),
        ],
      ),
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

    return OscCard(
      padding: EdgeInsets.zero,
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          // Editor toolbar
          Container(
            padding: const EdgeInsets.symmetric(horizontal: 20, vertical: 12),
            decoration: const BoxDecoration(
              border: Border(
                bottom: BorderSide(color: OscColors.border),
              ),
            ),
            child: Row(
              children: [
                Icon(
                  Icons.edit_note_rounded,
                  size: 16,
                  color: OscColors.violetLight.withValues(alpha: 0.7),
                ),
                const SizedBox(width: 8),
                Text(
                  'Editor',
                  style: OscTypography.bodySmall.copyWith(
                    fontWeight: FontWeight.w600,
                    color: OscColors.textMuted,
                    letterSpacing: 0.5,
                  ),
                ),
                const Spacer(),
                Text(
                  'Ln 6, Col 48',
                  style: OscTypography.monoSmall.copyWith(
                    fontSize: 11,
                    fontWeight: FontWeight.w500,
                    color: OscColors.textMuted.withValues(alpha: 0.55),
                    letterSpacing: 0.3,
                  ),
                ),
                const SizedBox(width: 16),
                Text(
                  'UTF-8',
                  style: OscTypography.monoSmall.copyWith(
                    fontSize: 11,
                    fontWeight: FontWeight.w500,
                    color: OscColors.textMuted.withValues(alpha: 0.55),
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
              style: OscTypography.monoBody.copyWith(
                fontSize: 12,
                color: OscColors.textMuted.withValues(alpha: 0.4),
              ),
            ),
          ),
          const SizedBox(width: 16),
          // Gutter line
          Container(
            width: 1,
            height: 28,
            color: OscColors.border,
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
                    style: OscTypography.monoBody.copyWith(
                      fontSize: 13,
                      fontWeight: line.isHeading ? FontWeight.w700 : FontWeight.w400,
                      color: line.isHeading ? OscColors.violetLight : OscColors.textMuted,
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
    return OscCard(
      activeBorder: true,
      gradient: OscCard.liveGradient,
      padding: const EdgeInsets.all(24),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          // Label row
          Row(
            children: [
              const OscSectionLabel(
                'GENERATED ENGINE VIEW',
                color: OscColors.violetLight,
              ),
              const Spacer(),
              const OscStatusBadge(
                label: 'Live',
                color: OscColors.violetLight,
              ),
            ],
          ),
          const SizedBox(height: 16),

          // Divider
          Container(height: 1, color: OscColors.border),
          const SizedBox(height: 16),

          // Engine preview surface
          Container(
            width: double.infinity,
            padding: const EdgeInsets.all(20),
            decoration: BoxDecoration(
              color: OscColors.canvasBg,
              borderRadius: OscRadii.cardBorder,
              border: Border.all(
                color: OscColors.violet.withValues(alpha: 0.12),
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
                      color: OscColors.violetLight.withValues(alpha: 0.7),
                    ),
                    const SizedBox(width: 8),
                    Text(
                      'Render Pipeline Active',
                      style: OscTypography.body.copyWith(
                        fontWeight: FontWeight.w600,
                        color: OscColors.textPrimary,
                        letterSpacing: 0.2,
                      ),
                    ),
                  ],
                ),
                const SizedBox(height: 12),
                Text(
                  'The engine view compiles document structure into a live-rendered '
                  'surface. Changes propagate through the compositor pipeline in real '
                  'time, producing frame-accurate previews of the final output.',
                  style: OscTypography.body.copyWith(
                    fontWeight: FontWeight.w400,
                    color: OscColors.textMuted,
                    height: 1.65,
                    letterSpacing: 0.1,
                  ),
                ),
                const SizedBox(height: 16),
                // Stats row
                Row(
                  children: [
                    OscTag.green(label: 'Latency 4ms'),
                    const SizedBox(width: 10),
                    OscTag.green(label: 'FPS 60'),
                    const SizedBox(width: 10),
                    OscTag.green(label: 'Nodes 142'),
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

// ─── Data Models ─────────────────────────────────────────────────────────────

class _EditorLine {
  final int number;
  final String text;
  final bool isHeading;

  const _EditorLine(this.number, this.text, this.isHeading);
}

// ─── Previews ──────────────────────────────────────────────────────────

@OscPreview(name: 'Canvas App', group: 'Apps')
Widget canvasAppPreview() {
  return const CanvasApp();
}

@OscPreview(name: 'Canvas Home', group: 'Apps')
Widget canvasHomePreview() {
  return const CanvasHome();
}
