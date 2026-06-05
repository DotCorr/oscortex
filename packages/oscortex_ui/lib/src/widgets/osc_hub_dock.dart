import 'package:flutter/material.dart';


import '../preview/osc_preview.dart';
import '../tokens/radii.dart';
import '../tokens/shadows.dart';

/// Glassmorphic hub dock bar (bottom navigation pill).
///
/// ```dart
/// OscHubDock(
///   segments: [
///     OscHubSegment(label: 'Canvas Hub', active: true, onTap: () {}),
///     OscHubSegment(label: 'Files', onTap: () {}),
///     OscHubSegment(label: 'Web Link', onTap: () {}),
///   ],
/// )
/// ```
class OscHubDock extends StatelessWidget {
  const OscHubDock({
    super.key,
    required this.segments,
    this.padding = const EdgeInsets.fromLTRB(16, 6, 16, 16),
  });

  final List<Widget> segments;
  final EdgeInsets padding;

  @override
  Widget build(BuildContext context) {
    return Padding(
      padding: padding,
      child: Center(
        child: Container(
          decoration: BoxDecoration(
            color: const Color(0xC70E0E12),
            borderRadius: OscRadii.pillBorder,
            border: Border.all(
              color: Colors.white.withValues(alpha: 0.10),
            ),
            boxShadow: const [OscShadows.hubDock],
          ),
          child: Padding(
            padding: const EdgeInsets.all(5),
            child: Row(
              mainAxisSize: MainAxisSize.min,
              children: [
                for (int i = 0; i < segments.length; i++) ...[
                  if (i > 0) const SizedBox(width: 2),
                  segments[i],
                ],
              ],
            ),
          ),
        ),
      ),
    );
  }
}

/// A single segment inside [OscHubDock].
class OscHubSegment extends StatefulWidget {
  const OscHubSegment({
    super.key,
    required this.label,
    this.active = false,
    this.enabled = true,
    this.onTap,
  });

  final String label;
  final bool active;
  final bool enabled;
  final VoidCallback? onTap;

  @override
  State<OscHubSegment> createState() => _OscHubSegmentState();
}

class _OscHubSegmentState extends State<OscHubSegment> {
  bool _hovered = false;

  @override
  Widget build(BuildContext context) {
    final bgAlpha = widget.active
        ? 0.10
        : _hovered
            ? 0.04
            : widget.enabled
                ? 0.02
                : 0.01;

    final textAlpha = widget.active
        ? 0.98
        : _hovered
            ? 0.90
            : widget.enabled
                ? 0.74
                : 0.35;

    return MouseRegion(
      onEnter: (_) => setState(() => _hovered = true),
      onExit: (_) => setState(() => _hovered = false),
      child: GestureDetector(
        onTap: widget.onTap,
        child: AnimatedContainer(
          duration: const Duration(milliseconds: 180),
          curve: Curves.easeOut,
          padding: const EdgeInsets.symmetric(horizontal: 16, vertical: 8),
          decoration: BoxDecoration(
            color: Colors.white.withValues(alpha: bgAlpha),
            borderRadius: OscRadii.pillBorder,
          ),
          child: Text(
            widget.label,
            style: TextStyle(
              fontSize: 12,
              fontWeight: widget.active ? FontWeight.w700 : FontWeight.w500,
              color: Colors.white.withValues(alpha: textAlpha),
            ),
          ),
        ),
      ),
    );
  }
}

// ---------------------------------------------------------------------------
// Previews
// ---------------------------------------------------------------------------

@OscPreview(name: 'Hub Dock with Segments', group: 'OscHubDock')
Widget oscHubDockPreview() {
  return SizedBox(
    width: 500,
    height: 80,
    child: OscHubDock(
      segments: [
        OscHubSegment(label: 'Canvas Hub', active: true, onTap: () {}),
        OscHubSegment(label: 'Files', onTap: () {}),
        OscHubSegment(label: 'Web Link', onTap: () {}),
      ],
    ),
  );
}

@OscPreview(name: 'Hub Dock Disabled Segment', group: 'OscHubDock')
Widget oscHubDockDisabledPreview() {
  return SizedBox(
    width: 500,
    height: 80,
    child: OscHubDock(
      segments: [
        OscHubSegment(label: 'Canvas Hub', active: true, onTap: () {}),
        OscHubSegment(label: 'Files', enabled: false),
        OscHubSegment(label: 'Web Link', enabled: false),
      ],
    ),
  );
}
