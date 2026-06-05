import 'dart:ui';

import 'package:flutter/material.dart';

import '../preview/osc_preview.dart';
import '../tokens/colors.dart';
import '../tokens/radii.dart';
import '../tokens/spacing.dart';
import '../tokens/typography.dart';
import 'osc_button.dart';

/// An OSCortex-native modal overlay — no Material Dialog.
///
/// Glassmorphic frosted panel with accent border glow.
///
/// ```dart
/// OscModal.show(
///   context: context,
///   title: 'Confirm Delete',
///   child: Text('Are you sure?'),
///   actions: [
///     OscButton(label: 'Cancel', onPressed: () => Navigator.pop(context)),
///     OscButton(label: 'Delete', accent: OscColors.red, onPressed: delete),
///   ],
/// );
/// ```
class OscModal extends StatelessWidget {
  const OscModal({
    super.key,
    this.title,
    required this.child,
    this.actions,
    this.accent,
    this.width = 420,
  });

  final String? title;
  final Widget child;
  final List<Widget>? actions;
  final Color? accent;
  final double width;

  /// Show the modal as a route overlay.
  static Future<T?> show<T>({
    required BuildContext context,
    String? title,
    required Widget child,
    List<Widget>? actions,
    Color? accent,
    double width = 420,
    bool dismissible = true,
  }) {
    return showGeneralDialog<T>(
      context: context,
      barrierDismissible: dismissible,
      barrierLabel: title ?? 'Dialog',
      barrierColor: Colors.black.withValues(alpha: 0.60),
      transitionDuration: const Duration(milliseconds: 260),
      transitionBuilder: (ctx, anim, secondAnim, dialogChild) {
        final curve = CurvedAnimation(parent: anim, curve: Curves.easeOutCubic);
        return ScaleTransition(
          scale: Tween<double>(begin: 0.92, end: 1.0).animate(curve),
          child: FadeTransition(opacity: curve, child: dialogChild),
        );
      },
      pageBuilder: (ctx, anim, secondAnim) {
        return Center(
          child: OscModal(
            title: title,
            accent: accent,
            width: width,
            child: child,
            actions: actions,
          ),
        );
      },
    );
  }

  @override
  Widget build(BuildContext context) {
    final color = accent ?? OscColors.violet;

    return ClipRRect(
      borderRadius: OscRadii.cardBorder,
      child: BackdropFilter(
        filter: ImageFilter.blur(sigmaX: 24, sigmaY: 24),
        child: Container(
          width: width,
          constraints: const BoxConstraints(maxHeight: 500),
          decoration: BoxDecoration(
            color: OscColors.canvasBg.withValues(alpha: 0.85),
            borderRadius: OscRadii.cardBorder,
            border: Border.all(
              color: color.withValues(alpha: 0.20),
            ),
            boxShadow: [
              BoxShadow(
                color: color.withValues(alpha: 0.08),
                blurRadius: 40,
                spreadRadius: 4,
              ),
              BoxShadow(
                color: Colors.black.withValues(alpha: 0.40),
                blurRadius: 20,
              ),
            ],
          ),
          child: Column(
            mainAxisSize: MainAxisSize.min,
            crossAxisAlignment: CrossAxisAlignment.stretch,
            children: [
              // Title bar
              if (title != null)
                Container(
                  padding: const EdgeInsets.fromLTRB(
                      OscSpacing.xl5, OscSpacing.lg, OscSpacing.lg, 0),
                  child: Row(
                    children: [
                      Expanded(
                        child: Text(
                          title!,
                          style: OscTypography.heading3.copyWith(
                            color: OscColors.textPrimary,
                          ),
                        ),
                      ),
                      GestureDetector(
                        onTap: () => Navigator.of(context).pop(),
                        child: Container(
                          width: 28,
                          height: 28,
                          decoration: BoxDecoration(
                            color: Colors.white.withValues(alpha: 0.05),
                            borderRadius: BorderRadius.circular(6),
                          ),
                          child: const Icon(Icons.close_rounded,
                              size: 14, color: OscColors.textMuted),
                        ),
                      ),
                    ],
                  ),
                ),

              // Content
              Flexible(
                child: SingleChildScrollView(
                  padding: EdgeInsets.fromLTRB(
                    OscSpacing.xl5,
                    title != null ? OscSpacing.lg : OscSpacing.xl5,
                    OscSpacing.xl5,
                    actions != null ? OscSpacing.lg : OscSpacing.xl5,
                  ),
                  child: child,
                ),
              ),

              // Actions bar
              if (actions != null && actions!.isNotEmpty)
                Container(
                  padding: const EdgeInsets.fromLTRB(
                      OscSpacing.xl5, 0, OscSpacing.xl5, OscSpacing.lg),
                  child: Row(
                    mainAxisAlignment: MainAxisAlignment.end,
                    children: [
                      for (var i = 0; i < actions!.length; i++) ...[
                        if (i > 0) const SizedBox(width: OscSpacing.md),
                        actions![i],
                      ],
                    ],
                  ),
                ),
            ],
          ),
        ),
      ),
    );
  }
}

// ─── Previews ──────────────────────────────────────────────────────────

@OscPreview(name: 'Confirm Modal', group: 'OscModal')
Widget oscModalConfirmPreview() {
  return OscModal(
    title: 'Confirm Delete',
    accent: OscColors.red,
    child: Text(
      'Are you sure you want to delete this file? This action cannot be undone.',
      style: TextStyle(color: OscColors.textMuted, fontSize: 13),
    ),
    actions: [
      OscOutlineButton(label: 'Cancel', onPressed: () {}),
      OscButton(label: 'Delete', accent: OscColors.red, onPressed: () {}),
    ],
  );
}

@OscPreview(name: 'Info Modal', group: 'OscModal')
Widget oscModalInfoPreview() {
  return OscModal(
    title: 'About OSCortex',
    child: Text(
      'OSCortex v0.1.0\nA modern operating system built with Flutter.\n\n© 2026 DotCorr',
      style: TextStyle(color: OscColors.textMuted, fontSize: 13),
    ),
    actions: [
      OscButton(label: 'OK', onPressed: () {}),
    ],
  );
}
