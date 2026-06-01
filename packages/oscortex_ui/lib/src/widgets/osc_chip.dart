import 'package:flutter/material.dart';

import '../preview/osc_preview.dart';
import '../tokens/colors.dart';
import '../tokens/radii.dart';

/// An OSCortex-themed quick-action chip.
///
/// Used for quick-access navigation (e.g. path shortcuts, filter tags).
///
/// ```dart
/// OscChip(
///   label: '/Applications',
///   icon: Icons.apps_rounded,
///   onTap: () => navigate('/Applications'),
/// );
/// ```
class OscChip extends StatelessWidget {
  const OscChip({
    super.key,
    required this.label,
    this.icon,
    this.onTap,
    this.accent,
    this.selected = false,
  });

  final String label;
  final IconData? icon;
  final VoidCallback? onTap;
  final Color? accent;
  final bool selected;

  @override
  Widget build(BuildContext context) {
    final color = accent ?? OscColors.skyBlue;

    return ActionChip(
      avatar: icon != null
          ? Icon(icon, size: 14, color: selected ? color : color.withValues(alpha: 0.7))
          : null,
      label: Text(
        label,
        style: TextStyle(
          fontSize: 12,
          fontWeight: selected ? FontWeight.w600 : FontWeight.w500,
          color: selected ? OscColors.textPrimary : OscColors.textPrimary,
        ),
      ),
      onPressed: onTap,
      backgroundColor: selected
          ? color.withValues(alpha: 0.12)
          : Colors.white.withValues(alpha: 0.04),
      side: BorderSide(
        color: selected
            ? color.withValues(alpha: 0.30)
            : Colors.white.withValues(alpha: 0.08),
      ),
      shape: RoundedRectangleBorder(borderRadius: OscRadii.buttonBorder),
    );
  }
}

// ─── Previews ──────────────────────────────────────────────────────────

@OscPreview(name: 'Quick Chips', group: 'OscChip')
Widget oscChipPreview() {
  return Padding(
    padding: const EdgeInsets.all(16),
    child: Wrap(
      spacing: 8,
      runSpacing: 8,
      children: [
        OscChip(label: '/Applications', icon: Icons.apps_rounded, onTap: () {}),
        OscChip(label: '/tmp', icon: Icons.folder_open_rounded, onTap: () {}),
        OscChip(label: '/sys/app', icon: Icons.memory_rounded, onTap: () {}),
        OscChip(label: 'Selected', icon: Icons.check_circle, selected: true, onTap: () {}),
        OscChip(label: 'Green', icon: Icons.eco_rounded, accent: OscColors.green, onTap: () {}),
      ],
    ),
  );
}
