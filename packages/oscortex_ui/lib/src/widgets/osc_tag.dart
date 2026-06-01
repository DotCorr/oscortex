import 'package:flutter/material.dart';

import '../tokens/colors.dart';
import '../tokens/radii.dart';
import '../preview/osc_preview.dart';

/// A tag chip matching the spec's small tag/label style.
///
/// ```dart
/// OscTag(label: 'Local Sync')
/// OscTag.violet(label: 'Spatial Audio Active')
/// OscTag.green(label: 'Connected')
/// ```
class OscTag extends StatelessWidget {
  const OscTag({
    super.key,
    required this.label,
    this.backgroundColor,
    this.borderColor,
    this.textColor,
    this.icon,
  });

  final String label;
  final Color? backgroundColor;
  final Color? borderColor;
  final Color? textColor;
  final IconData? icon;

  /// Violet-accented tag.
  factory OscTag.violet({Key? key, required String label, IconData? icon}) {
    return OscTag(
      key: key,
      label: label,
      icon: icon,
      backgroundColor: OscColors.violet.withValues(alpha: 0.10),
      borderColor: OscColors.violet.withValues(alpha: 0.20),
      textColor: OscColors.violetLight,
    );
  }

  /// Green-accented tag.
  factory OscTag.green({Key? key, required String label, IconData? icon}) {
    return OscTag(
      key: key,
      label: label,
      icon: icon,
      backgroundColor: OscColors.green.withValues(alpha: 0.10),
      borderColor: OscColors.green.withValues(alpha: 0.20),
      textColor: OscColors.green,
    );
  }

  /// Sky-blue-accented tag.
  factory OscTag.sky({Key? key, required String label, IconData? icon}) {
    return OscTag(
      key: key,
      label: label,
      icon: icon,
      backgroundColor: OscColors.skyBlue.withValues(alpha: 0.10),
      borderColor: OscColors.skyBlue.withValues(alpha: 0.20),
      textColor: OscColors.skyBlue,
    );
  }

  /// Red-accented tag (error / destructive).
  factory OscTag.red({Key? key, required String label, IconData? icon}) {
    return OscTag(
      key: key,
      label: label,
      icon: icon,
      backgroundColor: OscColors.red.withValues(alpha: 0.10),
      borderColor: OscColors.red.withValues(alpha: 0.20),
      textColor: OscColors.red,
    );
  }

  @override
  Widget build(BuildContext context) {
    return Container(
      padding: const EdgeInsets.symmetric(horizontal: 8, vertical: 3),
      decoration: BoxDecoration(
        color: backgroundColor ?? Colors.white.withValues(alpha: 0.04),
        borderRadius: OscRadii.tagBorder,
        border: Border.all(
          color: borderColor ?? Colors.white.withValues(alpha: 0.05),
        ),
      ),
      child: Row(
        mainAxisSize: MainAxisSize.min,
        children: [
          if (icon != null) ...[
            Icon(icon, size: 10, color: textColor ?? OscColors.textMuted),
            const SizedBox(width: 4),
          ],
          Text(
            label,
            style: TextStyle(
              fontFamily: 'RobotoMono',
              fontSize: 9,
              color: textColor ?? Colors.white.withValues(alpha: 0.45),
            ),
          ),
        ],
      ),
    );
  }
}

@OscPreview(name: 'All Tag Variants', group: 'OscTag')
Widget oscTagAllPreview() {
  return Padding(
    padding: EdgeInsets.all(16),
    child: Wrap(
      spacing: 8,
      runSpacing: 8,
      children: [
        OscTag(label: 'Default'),
        OscTag.violet(label: 'Spatial Audio Active'),
        OscTag.green(label: 'Connected'),
        OscTag.sky(label: 'Downloading'),
        OscTag.red(label: 'Error'),
        OscTag.violet(label: 'With Icon', icon: Icons.music_note),
        OscTag.green(label: 'Saved', icon: Icons.check_circle),
      ],
    ),
  );
}
