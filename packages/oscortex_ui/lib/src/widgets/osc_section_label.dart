import 'package:flutter/material.dart';

import '../tokens/typography.dart';

/// A mono-uppercase section header label.
///
/// ```dart
/// OscSectionLabel('DOCUMENT CANVAS', color: OscColors.violetLight)
/// OscSectionLabel('NOW PLAYING')
/// ```
class OscSectionLabel extends StatelessWidget {
  const OscSectionLabel(
    this.text, {
    super.key,
    this.color,
    this.small = false,
  });

  final String text;

  /// Label color (defaults to muted white).
  final Color? color;

  /// Use the smaller 9px variant (for "NOW PLAYING" etc.)
  final bool small;

  @override
  Widget build(BuildContext context) {
    return Text(
      text,
      style: (small ? OscTypography.monoLabelSmall : OscTypography.monoLabel)
          .copyWith(
        color: color ?? Colors.white.withValues(alpha: 0.35),
      ),
    );
  }
}
