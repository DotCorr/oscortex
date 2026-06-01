import 'package:flutter/material.dart';

import '../preview/osc_preview.dart';
import '../tokens/colors.dart';
import '../tokens/radii.dart';

/// An OSCortex-themed filled button.
///
/// ```dart
/// OscButton(label: 'Install', onPressed: () {});
/// OscButton(label: 'Save', icon: Icons.save, onPressed: () {});
/// OscButton.loading(label: 'Installing');  // spinner state
/// ```
class OscButton extends StatelessWidget {
  const OscButton({
    super.key,
    required this.label,
    this.icon,
    this.onPressed,
    this.accent,
    this.loading = false,
    this.expand = false,
  });

  /// Convenience constructor for a loading/spinner state.
  const OscButton.loading({
    super.key,
    required this.label,
    this.accent,
    this.expand = false,
  })  : icon = null,
        onPressed = null,
        loading = true;

  final String label;
  final IconData? icon;
  final VoidCallback? onPressed;
  final Color? accent;

  /// Show a spinner instead of the label.
  final bool loading;

  /// If true, takes full available width.
  final bool expand;

  @override
  Widget build(BuildContext context) {
    final color = accent ?? OscColors.violet;
    final child = loading
        ? SizedBox.square(
            dimension: 14,
            child: CircularProgressIndicator(
              strokeWidth: 2,
              color: color.computeLuminance() > 0.5
                  ? OscColors.bodyBg
                  : Colors.white,
            ),
          )
        : icon != null
            ? Row(
                mainAxisSize: MainAxisSize.min,
                children: [
                  Icon(icon, size: 16),
                  const SizedBox(width: 6),
                  Text(label),
                ],
              )
            : Text(label);

    final button = FilledButton(
      onPressed: loading ? null : onPressed,
      style: FilledButton.styleFrom(
        backgroundColor: color,
        foregroundColor: Colors.white,
        disabledBackgroundColor: color.withValues(alpha: 0.4),
        disabledForegroundColor: Colors.white.withValues(alpha: 0.5),
        shape: RoundedRectangleBorder(borderRadius: OscRadii.buttonBorder),
        padding: const EdgeInsets.symmetric(horizontal: 16, vertical: 10),
        textStyle: const TextStyle(fontSize: 13, fontWeight: FontWeight.w600),
      ),
      child: child,
    );

    return expand
        ? SizedBox(width: double.infinity, height: 38, child: button)
        : button;
  }
}

/// An OSCortex-themed outlined / ghost button.
///
/// ```dart
/// OscOutlineButton(label: 'Open', onPressed: () {});
/// ```
class OscOutlineButton extends StatelessWidget {
  const OscOutlineButton({
    super.key,
    required this.label,
    this.icon,
    this.onPressed,
    this.accent,
    this.expand = false,
  });

  final String label;
  final IconData? icon;
  final VoidCallback? onPressed;
  final Color? accent;
  final bool expand;

  @override
  Widget build(BuildContext context) {
    final color = accent ?? Colors.white.withValues(alpha: 0.60);

    final child = icon != null
        ? Row(
            mainAxisSize: MainAxisSize.min,
            children: [
              Icon(icon, size: 16),
              const SizedBox(width: 6),
              Text(label),
            ],
          )
        : Text(label);

    final button = OutlinedButton(
      onPressed: onPressed,
      style: OutlinedButton.styleFrom(
        foregroundColor: color,
        side: BorderSide(color: color.withValues(alpha: 0.25)),
        shape: RoundedRectangleBorder(borderRadius: OscRadii.buttonBorder),
        padding: const EdgeInsets.symmetric(horizontal: 16, vertical: 10),
        textStyle: const TextStyle(fontSize: 13, fontWeight: FontWeight.w500),
      ),
      child: child,
    );

    return expand
        ? SizedBox(width: double.infinity, height: 38, child: button)
        : button;
  }
}

// ─── Previews ──────────────────────────────────────────────────────────

@OscPreview(name: 'Filled Button', group: 'OscButton')
Widget oscButtonFilledPreview() {
  return Padding(
    padding: const EdgeInsets.all(16),
    child: OscButton(label: 'Install', onPressed: () {}),
  );
}

@OscPreview(name: 'Filled with Icon', group: 'OscButton')
Widget oscButtonIconPreview() {
  return Padding(
    padding: const EdgeInsets.all(16),
    child: OscButton(
        label: 'Save', icon: Icons.save_rounded, onPressed: () {}),
  );
}

@OscPreview(name: 'Loading State', group: 'OscButton')
Widget oscButtonLoadingPreview() {
  return const Padding(
    padding: EdgeInsets.all(16),
    child: OscButton.loading(label: 'Installing'),
  );
}

@OscPreview(name: 'Sky Blue Accent', group: 'OscButton')
Widget oscButtonSkyPreview() {
  return Padding(
    padding: const EdgeInsets.all(16),
    child: OscButton(
        label: 'Download', accent: OscColors.skyBlue, onPressed: () {}),
  );
}

@OscPreview(name: 'Expand Full Width', group: 'OscButton')
Widget oscButtonExpandPreview() {
  return Padding(
    padding: const EdgeInsets.all(16),
    child: SizedBox(
      width: 300,
      child: OscButton(
          label: 'Install App', accent: OscColors.green, expand: true, onPressed: () {}),
    ),
  );
}

@OscPreview(name: 'Outline Button', group: 'OscButton')
Widget oscOutlineButtonPreview() {
  return Padding(
    padding: const EdgeInsets.all(16),
    child: OscOutlineButton(label: 'Open', onPressed: () {}),
  );
}

@OscPreview(name: 'All Button Variants', group: 'OscButton')
Widget oscButtonAllPreview() {
  return Padding(
    padding: const EdgeInsets.all(16),
    child: Wrap(
      spacing: 8,
      runSpacing: 8,
      children: [
        OscButton(label: 'Violet', onPressed: () {}),
        OscButton(label: 'Sky', accent: OscColors.skyBlue, onPressed: () {}),
        OscButton(label: 'Green', accent: OscColors.green, onPressed: () {}),
        const OscButton.loading(label: 'Loading'),
        OscOutlineButton(label: 'Open', onPressed: () {}),
        OscOutlineButton(label: 'Cancel', accent: OscColors.red, onPressed: () {}),
      ],
    ),
  );
}
