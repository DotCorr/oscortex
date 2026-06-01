import 'package:flutter/material.dart';

import '../preview/osc_preview.dart';
import '../tokens/colors.dart';
import '../tokens/radii.dart';
import '../tokens/typography.dart';

/// A horizontally scrollable breadcrumb trail.
///
/// Each segment is tappable except the last (active) one.
///
/// ```dart
/// OscBreadcrumbs(
///   segments: ['/', 'Applications', 'MyApp'],
///   onTap: (index) => navigateToDepth(index),
/// );
/// ```
class OscBreadcrumbs extends StatelessWidget {
  const OscBreadcrumbs({
    super.key,
    required this.segments,
    this.onTap,
    this.accent,
  });

  final List<String> segments;
  final ValueChanged<int>? onTap;
  final Color? accent;

  @override
  Widget build(BuildContext context) {
    final color = accent ?? OscColors.skyBlue;

    return SizedBox(
      height: 32,
      child: ListView.separated(
        scrollDirection: Axis.horizontal,
        itemCount: segments.length,
        separatorBuilder: (_, __) => Padding(
          padding: const EdgeInsets.symmetric(horizontal: 4),
          child: Icon(
            Icons.chevron_right_rounded,
            size: 14,
            color: OscColors.textDim,
          ),
        ),
        itemBuilder: (context, index) {
          final isLast = index == segments.length - 1;
          return GestureDetector(
            onTap: isLast ? null : () => onTap?.call(index),
            child: Container(
              padding: const EdgeInsets.symmetric(horizontal: 8, vertical: 4),
              decoration: isLast
                  ? BoxDecoration(
                      color: color.withValues(alpha: 0.10),
                      borderRadius: OscRadii.tagBorder,
                      border: Border.all(color: color.withValues(alpha: 0.20)),
                    )
                  : null,
              child: Center(
                child: Text(
                  segments[index],
                  style: OscTypography.monoSmall.copyWith(
                    color: isLast
                        ? color
                        : OscColors.textMuted,
                    fontWeight: isLast ? FontWeight.w600 : FontWeight.w400,
                  ),
                ),
              ),
            ),
          );
        },
      ),
    );
  }
}

// ─── Previews ──────────────────────────────────────────────────────────

@OscPreview(name: 'Path Breadcrumbs', group: 'OscBreadcrumbs')
Widget oscBreadcrumbsPreview() {
  return SizedBox(
    width: 500,
    child: Padding(
      padding: const EdgeInsets.all(16),
      child: OscBreadcrumbs(
        segments: const ['/', 'Applications', 'MyApp', 'assets'],
        onTap: (_) {},
      ),
    ),
  );
}

@OscPreview(name: 'Root Only', group: 'OscBreadcrumbs')
Widget oscBreadcrumbsRootPreview() {
  return SizedBox(
    width: 400,
    child: Padding(
      padding: const EdgeInsets.all(16),
      child: OscBreadcrumbs(
        segments: const ['/'],
        accent: OscColors.green,
      ),
    ),
  );
}
