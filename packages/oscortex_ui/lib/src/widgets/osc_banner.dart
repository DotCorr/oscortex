import 'package:flutter/material.dart';

import '../preview/osc_preview.dart';
import '../tokens/colors.dart';
import '../tokens/radii.dart';

/// An animated status banner with color-coded states.
///
/// Used for install progress, sync status, etc.
///
/// ```dart
/// OscBanner(
///   message: 'Installing myapp.osx…',
///   state: OscBannerState.loading,
/// );
/// ```
enum OscBannerState {
  idle,
  loading,
  success,
  error,
}

class OscBanner extends StatelessWidget {
  const OscBanner({
    super.key,
    required this.message,
    this.state = OscBannerState.idle,
    this.icon,
  });

  final String message;
  final OscBannerState state;
  final IconData? icon;

  Color get _borderColor => switch (state) {
        OscBannerState.idle => OscColors.border,
        OscBannerState.loading => OscColors.skyBlue,
        OscBannerState.success => OscColors.green,
        OscBannerState.error => OscColors.red,
      };

  Color get _bgColor => switch (state) {
        OscBannerState.idle => OscColors.surface,
        OscBannerState.loading => OscColors.skyBlue.withValues(alpha: 0.08),
        OscBannerState.success => OscColors.green.withValues(alpha: 0.08),
        OscBannerState.error => OscColors.red.withValues(alpha: 0.08),
      };

  IconData get _stateIcon => icon ?? switch (state) {
        OscBannerState.idle => Icons.info_outline_rounded,
        OscBannerState.loading => Icons.downloading_rounded,
        OscBannerState.success => Icons.check_circle_rounded,
        OscBannerState.error => Icons.error_rounded,
      };

  @override
  Widget build(BuildContext context) {
    return AnimatedContainer(
      duration: const Duration(milliseconds: 300),
      curve: Curves.easeOut,
      padding: const EdgeInsets.symmetric(horizontal: 14, vertical: 10),
      decoration: BoxDecoration(
        color: _bgColor,
        borderRadius: OscRadii.innerBorder,
        border: Border.all(color: _borderColor.withValues(alpha: 0.35)),
      ),
      child: Row(
        children: [
          if (state == OscBannerState.loading)
            SizedBox.square(
              dimension: 16,
              child: CircularProgressIndicator(
                strokeWidth: 2,
                color: _borderColor,
              ),
            )
          else
            Icon(_stateIcon, size: 16, color: _borderColor),
          const SizedBox(width: 10),
          Expanded(
            child: Text(
              message,
              style: TextStyle(
                fontSize: 12,
                fontWeight: FontWeight.w500,
                color: _borderColor,
              ),
            ),
          ),
        ],
      ),
    );
  }
}

// ─── Previews ──────────────────────────────────────────────────────────

@OscPreview(name: 'All Banner States', group: 'OscBanner')
Widget oscBannerAllPreview() {
  return SizedBox(
    width: 400,
    child: Padding(
      padding: const EdgeInsets.all(16),
      child: Column(
        mainAxisSize: MainAxisSize.min,
        children: const [
          OscBanner(message: 'Ready to install apps.'),
          SizedBox(height: 8),
          OscBanner(
              message: 'Installing myapp.osx…',
              state: OscBannerState.loading),
          SizedBox(height: 8),
          OscBanner(
              message: 'Successfully installed!',
              state: OscBannerState.success),
          SizedBox(height: 8),
          OscBanner(
              message: 'Install failed: permission denied.',
              state: OscBannerState.error),
        ],
      ),
    ),
  );
}
