import 'package:flutter/material.dart';

import '../preview/osc_preview.dart';
import '../tokens/colors.dart';
import '../tokens/radii.dart';

/// A small icon-action button matching the spec's toolbar controls.
///
/// 38×38 square with surface background, border, and centered child.
///
/// ```dart
/// OscIconButton(
///   icon: Icons.refresh_rounded,
///   tooltip: 'Refresh',
///   onPressed: () {},
/// );
/// ```
class OscIconButton extends StatefulWidget {
  const OscIconButton({
    super.key,
    required this.icon,
    this.tooltip,
    this.onPressed,
    this.size = 38,
    this.iconSize = 16,
    this.accent,
    this.loading = false,
  });

  final IconData icon;
  final String? tooltip;
  final VoidCallback? onPressed;
  final double size;
  final double iconSize;
  final Color? accent;
  final bool loading;

  @override
  State<OscIconButton> createState() => _OscIconButtonState();
}

class _OscIconButtonState extends State<OscIconButton> {
  bool _hovered = false;

  @override
  Widget build(BuildContext context) {
    final color = widget.accent ?? OscColors.textMuted;

    final child = widget.loading
        ? SizedBox.square(
            dimension: widget.iconSize,
            child: CircularProgressIndicator(
              strokeWidth: 1.5,
              color: widget.accent ?? OscColors.violetLight,
            ),
          )
        : Icon(
            widget.icon,
            size: widget.iconSize,
            color: _hovered
                ? color.withValues(alpha: 0.9)
                : color.withValues(alpha: 0.65),
          );

    Widget button = MouseRegion(
      onEnter: (_) => setState(() => _hovered = true),
      onExit: (_) => setState(() => _hovered = false),
      child: GestureDetector(
        onTap: widget.loading ? null : widget.onPressed,
        child: AnimatedContainer(
          duration: const Duration(milliseconds: 160),
          width: widget.size,
          height: widget.size,
          decoration: BoxDecoration(
            color: _hovered
                ? OscColors.surfaceHover
                : OscColors.surface,
            borderRadius: OscRadii.buttonBorder,
            border: Border.all(
              color: _hovered ? OscColors.borderHover : OscColors.border,
            ),
          ),
          child: Center(child: child),
        ),
      ),
    );

    if (widget.tooltip != null) {
      button = Tooltip(message: widget.tooltip!, child: button);
    }

    return button;
  }
}

// ─── Previews ──────────────────────────────────────────────────────────

@OscPreview(name: 'Default Icon Button', group: 'OscIconButton')
Widget oscIconButtonPreview() {
  return Padding(
    padding: const EdgeInsets.all(16),
    child: Wrap(
      spacing: 8,
      children: [
        OscIconButton(icon: Icons.refresh_rounded, tooltip: 'Refresh', onPressed: () {}),
        OscIconButton(icon: Icons.source_rounded, tooltip: 'Source', onPressed: () {}),
        OscIconButton(icon: Icons.settings_rounded, tooltip: 'Settings', onPressed: () {}),
        OscIconButton(icon: Icons.download_rounded, tooltip: 'Download', accent: OscColors.skyBlue, onPressed: () {}),
        const OscIconButton(icon: Icons.refresh_rounded, loading: true),
      ],
    ),
  );
}
