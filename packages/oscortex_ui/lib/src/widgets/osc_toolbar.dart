import 'package:flutter/material.dart';

import '../tokens/colors.dart';
import '../tokens/spacing.dart';

/// Top chrome toolbar matching the spec's 44px header.
///
/// ```dart
/// OscToolbar(
///   title: 'Personal Canvas',
///   status: '3 apps available',
///   trailing: [clockWidget, refreshButton],
/// )
/// ```
class OscToolbar extends StatelessWidget {
  const OscToolbar({
    super.key,
    required this.title,
    this.status,
    this.leading,
    this.trailing = const [],
  });

  final String title;
  final String? status;
  final Widget? leading;
  final List<Widget> trailing;

  @override
  Widget build(BuildContext context) {
    return Container(
      height: OscSpacing.chromeHeight,
      padding: const EdgeInsets.symmetric(horizontal: OscSpacing.xl3),
      decoration: BoxDecoration(
        border: Border(
          bottom: BorderSide(
            color: Colors.white.withValues(alpha: 0.07),
          ),
        ),
      ),
      child: Row(
        children: [
          if (leading != null) ...[
            leading!,
            const SizedBox(width: OscSpacing.md),
          ],
          Text(
            title,
            style: const TextStyle(
              fontSize: 13,
              fontWeight: FontWeight.w600,
              color: OscColors.textBright,
            ),
          ),
          if (status != null) ...[
            const SizedBox(width: OscSpacing.xl2),
            Expanded(
              child: Text(
                status!,
                overflow: TextOverflow.ellipsis,
                style: TextStyle(
                  fontSize: 11,
                  color: Colors.white.withValues(alpha: 0.40),
                ),
              ),
            ),
          ] else
            const Spacer(),
          for (final widget in trailing) ...[
            const SizedBox(width: OscSpacing.md),
            widget,
          ],
        ],
      ),
    );
  }
}
