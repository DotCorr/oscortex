import 'package:flutter/material.dart';

import '../tokens/typography.dart';
import '../preview/osc_preview.dart';

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

@OscPreview(name: 'Default Label', group: 'OscSectionLabel')
Widget oscSectionLabelPreview() {
  return const Padding(
    padding: EdgeInsets.all(16),
    child: OscSectionLabel('DOCUMENT CANVAS'),
  );
}

@OscPreview(name: 'Violet Colored', group: 'OscSectionLabel')
Widget oscSectionLabelVioletPreview() {
  return Padding(
    padding: const EdgeInsets.all(16),
    child: OscSectionLabel('KNOWLEDGE SOURCE ENGINE', color: Color(0xFFA78BFA)),
  );
}

@OscPreview(name: 'Small Label', group: 'OscSectionLabel')
Widget oscSectionLabelSmallPreview() {
  return const Padding(
    padding: EdgeInsets.all(16),
    child: OscSectionLabel('NOW PLAYING', small: true),
  );
}
