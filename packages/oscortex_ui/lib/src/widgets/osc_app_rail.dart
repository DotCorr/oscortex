import 'package:flutter/material.dart';


import '../preview/osc_preview.dart';
import '../tokens/colors.dart';
import '../tokens/radii.dart';
import '../tokens/spacing.dart';

/// Left-side app rail (68px sidebar).
///
/// ```dart
/// OscAppRail(
///   children: [
///     OscRailButton(icon: Icons.folder, label: 'Files', active: true),
///     OscRailButton(icon: Icons.link, label: 'Web'),
///   ],
///   bottom: OscRailButton(icon: Icons.grid_view, label: 'All Apps', dashed: true),
/// )
/// ```
class OscAppRail extends StatelessWidget {
  const OscAppRail({
    super.key,
    required this.children,
    this.bottom,
  });

  final List<Widget> children;
  final Widget? bottom;

  @override
  Widget build(BuildContext context) {
    return Container(
      width: OscSpacing.railWidth,
      decoration: BoxDecoration(
        color: Colors.black.withValues(alpha: 0.20),
        border: Border(
          right: BorderSide(
            color: Colors.white.withValues(alpha: 0.07),
          ),
        ),
      ),
      child: Column(
        children: [
          const SizedBox(height: 10),
          for (final child in children)
            Padding(
              padding: const EdgeInsets.symmetric(horizontal: 6, vertical: 2),
              child: child,
            ),
          const Spacer(),
          if (bottom != null)
            Padding(
              padding: const EdgeInsets.fromLTRB(6, 0, 6, 12),
              child: bottom,
            ),
        ],
      ),
    );
  }
}

/// A button inside [OscAppRail].
class OscRailButton extends StatefulWidget {
  const OscRailButton({
    super.key,
    required this.icon,
    required this.label,
    this.active = false,
    this.accentColor = OscColors.violetLight,
    this.onTap,
    this.dashed = false,
    this.tooltip,
  });

  final IconData icon;
  final String label;
  final bool active;
  final Color accentColor;
  final VoidCallback? onTap;

  /// Dashed border style (used for "All Apps" button).
  final bool dashed;
  final String? tooltip;

  @override
  State<OscRailButton> createState() => _OscRailButtonState();
}

class _OscRailButtonState extends State<OscRailButton> {
  bool _hovered = false;

  @override
  Widget build(BuildContext context) {
    final bgAlpha = widget.active
        ? 0.06
        : _hovered
            ? 0.04
            : 0.0;

    final child = GestureDetector(
      onTap: widget.onTap,
      child: MouseRegion(
        onEnter: (_) => setState(() => _hovered = true),
        onExit: (_) => setState(() => _hovered = false),
        child: AnimatedContainer(
          duration: const Duration(milliseconds: 160),
          curve: Curves.easeOut,
          decoration: BoxDecoration(
            color: Colors.white.withValues(alpha: bgAlpha),
            borderRadius: OscRadii.buttonBorder,
            border: widget.dashed
                ? Border.all(
                    color: Colors.white.withValues(alpha: 0.10),
                  )
                : null,
          ),
          child: Column(
            children: [
              const SizedBox(height: 8),
              Container(
                width: 28,
                height: 28,
                decoration: BoxDecoration(
                  borderRadius: OscRadii.iconBorder,
                  border: Border.all(
                    color: widget.active
                        ? widget.accentColor.withValues(alpha: 0.30)
                        : Colors.white.withValues(alpha: 0.06),
                  ),
                  color: widget.active
                      ? widget.accentColor.withValues(alpha: 0.15)
                      : Colors.white.withValues(alpha: 0.03),
                ),
                child: Icon(
                  widget.icon,
                  size: 14,
                  color: widget.active
                      ? widget.accentColor
                      : Colors.white.withValues(
                          alpha: _hovered ? 0.60 : 0.40,
                        ),
                ),
              ),
              const SizedBox(height: 4),
              Text(
                widget.label,
                style: TextStyle(
                  fontSize: 7.5,
                  fontWeight: widget.active ? FontWeight.w600 : FontWeight.w400,
                  color: widget.active
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

    if (widget.tooltip != null) {
      return Tooltip(message: widget.tooltip!, child: child);
    }
    return child;
  }
}

// ---------------------------------------------------------------------------
// Previews
// ---------------------------------------------------------------------------

@OscPreview(name: 'App Rail with Buttons', group: 'OscAppRail')
Widget oscAppRailPreview() {
  return SizedBox(
    height: 400,
    child: OscAppRail(
      bottom: OscRailButton(
        icon: Icons.grid_view_rounded,
        label: 'All Apps',
        dashed: true,
      ),
      children: [
        OscRailButton(icon: Icons.dashboard_customize_rounded, label: 'Canvas', active: true),
        OscRailButton(icon: Icons.folder_rounded, label: 'Files'),
        OscRailButton(icon: Icons.link_rounded, label: 'Web'),
        OscRailButton(icon: Icons.apps_rounded, label: 'MyApp'),
      ],
    ),
  );
}
