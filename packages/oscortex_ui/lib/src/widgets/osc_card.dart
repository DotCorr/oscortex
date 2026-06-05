import 'package:flutter/material.dart';

import '../tokens/colors.dart';
import '../tokens/radii.dart';
import '../tokens/shadows.dart';
import '../preview/osc_preview.dart';

/// A surface card matching the OSCortex `canvas-surface` spec.
///
/// ```dart
/// OscCard(
///   child: Text('Content'),
///   gradient: OscCard.violetGradient,  // optional tinted gradient
///   activeBorder: true,                // violet active border
/// )
/// ```
class OscCard extends StatefulWidget {
  const OscCard({
    super.key,
    required this.child,
    this.padding = const EdgeInsets.all(16),
    this.gradient,
    this.activeBorder = false,
    this.onTap,
    this.hoverEnabled = true,
  });

  final Widget child;
  final EdgeInsets padding;

  /// Optional gradient overlay (e.g. violet tint for Document Canvas).
  final Gradient? gradient;

  /// Whether to use the violet active border instead of the default.
  final bool activeBorder;

  /// Tap callback — makes the card interactive.
  final VoidCallback? onTap;

  /// Whether hover effects are enabled.
  final bool hoverEnabled;

  // ── Pre-built gradients ────────────────────────────────────────────
  /// Violet tint for "Document Canvas" style cards.
  static const Gradient violetGradient = LinearGradient(
    begin: Alignment(-0.6, -1),
    end: Alignment(0.8, 1),
    colors: [Color(0x0D7C3AED), Color(0x06FFFFFF)],
  );

  /// Live / active card gradient.
  static const Gradient liveGradient = LinearGradient(
    begin: Alignment.centerLeft,
    end: Alignment.centerRight,
    colors: [Color(0x127C3AED), Color(0x06FFFFFF)],
  );

  @override
  State<OscCard> createState() => _OscCardState();
}

class _OscCardState extends State<OscCard> {
  bool _hovered = false;

  @override
  Widget build(BuildContext context) {
    final isHovered = widget.hoverEnabled && _hovered;

    return MouseRegion(
      onEnter: widget.hoverEnabled ? (_) => setState(() => _hovered = true) : null,
      onExit: widget.hoverEnabled ? (_) => setState(() => _hovered = false) : null,
      child: GestureDetector(
        onTap: widget.onTap,
        child: AnimatedContainer(
          duration: const Duration(milliseconds: 200),
          curve: Curves.easeOut,
          padding: widget.padding,
          decoration: BoxDecoration(
            borderRadius: OscRadii.cardBorder,
            border: Border.all(
              color: widget.activeBorder
                  ? OscColors.borderActive
                  : isHovered
                      ? OscColors.borderHover
                      : OscColors.border,
            ),
            color: widget.gradient == null
                ? (isHovered ? OscColors.surfaceHover : OscColors.surface)
                : null,
            gradient: widget.gradient,
            boxShadow: [
              isHovered ? OscShadows.hover : OscShadows.card,
            ],
          ),
          child: widget.child,
        ),
      ),
    );
  }
}

@OscPreview(name: 'Default Card', group: 'OscCard')
Widget oscCardDefaultPreview() {
  return const SizedBox(
    width: 320,
    child: OscCard(
      child: Text('Default surface card with standard border and shadow.',
          style: TextStyle(color: Color(0xFFD4D4D8), fontSize: 12)),
    ),
  );
}

@OscPreview(name: 'Violet Gradient Card', group: 'OscCard')
Widget oscCardVioletPreview() {
  return SizedBox(
    width: 320,
    child: OscCard(
      gradient: OscCard.violetGradient,
      padding: const EdgeInsets.all(18),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        mainAxisSize: MainAxisSize.min,
        children: [
          Text('DOCUMENT CANVAS',
              style: TextStyle(
                  fontFamily: 'RobotoMono', fontSize: 10, fontWeight: FontWeight.w600,
                  letterSpacing: 1.4, color: Color(0xFFA78BFA))),
          SizedBox(height: 6),
          Text('\u201CProject Proposal.md\u201D',
              style: TextStyle(fontSize: 15, fontWeight: FontWeight.w600, color: Colors.white)),
        ],
      ),
    ),
  );
}

@OscPreview(name: 'Active Border Card', group: 'OscCard')
Widget oscCardActivePreview() {
  return SizedBox(
    width: 320,
    child: OscCard(
      activeBorder: true,
      gradient: OscCard.liveGradient,
      child: Text('Active border card with live gradient.',
          style: TextStyle(color: Color(0xFFD4D4D8), fontSize: 12)),
    ),
  );
}
