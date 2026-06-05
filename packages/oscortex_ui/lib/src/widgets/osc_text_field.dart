import 'package:flutter/material.dart';

import '../preview/osc_preview.dart';
import '../tokens/colors.dart';
import '../tokens/radii.dart';
import '../tokens/typography.dart';

/// An OSCortex-themed text input field.
///
/// Matches the spec's dark-surface input with accent-colored focus borders.
///
/// ```dart
/// OscTextField(
///   hint: 'Enter filesystem path…',
///   prefixIcon: Icons.terminal_rounded,
///   controller: myController,
///   onSubmitted: (val) => print(val),
/// );
/// ```
class OscTextField extends StatelessWidget {
  const OscTextField({
    super.key,
    this.hint,
    this.prefixIcon,
    this.suffix,
    this.controller,
    this.onSubmitted,
    this.onChanged,
    this.accent,
    this.readOnly = false,
    this.autofocus = false,
  });

  final String? hint;
  final IconData? prefixIcon;
  final Widget? suffix;
  final TextEditingController? controller;
  final ValueChanged<String>? onSubmitted;
  final ValueChanged<String>? onChanged;
  final Color? accent;
  final bool readOnly;
  final bool autofocus;

  @override
  Widget build(BuildContext context) {
    final accentColor = accent ?? OscColors.violet;

    return TextField(
      controller: controller,
      onSubmitted: onSubmitted,
      onChanged: onChanged,
      readOnly: readOnly,
      autofocus: autofocus,
      style: OscTypography.body.copyWith(
        color: OscColors.textPrimary,
        fontWeight: FontWeight.w500,
      ),
      cursorColor: accentColor,
      decoration: InputDecoration(
        hintText: hint,
        hintStyle: OscTypography.body.copyWith(color: OscColors.textDim),
        prefixIcon: prefixIcon != null
            ? Icon(prefixIcon, size: 18, color: OscColors.textMuted)
            : null,
        suffixIcon: suffix,
        filled: true,
        fillColor: Colors.white.withValues(alpha: 0.035),
        contentPadding:
            const EdgeInsets.symmetric(horizontal: 14, vertical: 12),
        border: OutlineInputBorder(
          borderRadius: OscRadii.innerBorder,
          borderSide: BorderSide(color: OscColors.border),
        ),
        enabledBorder: OutlineInputBorder(
          borderRadius: OscRadii.innerBorder,
          borderSide: BorderSide(color: OscColors.border),
        ),
        focusedBorder: OutlineInputBorder(
          borderRadius: OscRadii.innerBorder,
          borderSide: BorderSide(color: accentColor, width: 1.5),
        ),
      ),
    );
  }
}

// ─── Previews ──────────────────────────────────────────────────────────

@OscPreview(name: 'Default TextField', group: 'OscTextField')
Widget oscTextFieldPreview() {
  return const SizedBox(
    width: 400,
    child: Padding(
      padding: EdgeInsets.all(16),
      child: OscTextField(
        hint: 'Enter filesystem path…',
        prefixIcon: Icons.terminal_rounded,
      ),
    ),
  );
}

@OscPreview(name: 'Sky Blue Accent', group: 'OscTextField')
Widget oscTextFieldSkyPreview() {
  return const SizedBox(
    width: 400,
    child: Padding(
      padding: EdgeInsets.all(16),
      child: OscTextField(
        hint: 'Search files...',
        prefixIcon: Icons.search_rounded,
        accent: OscColors.skyBlue,
      ),
    ),
  );
}

@OscPreview(name: 'With URL', group: 'OscTextField')
Widget oscTextFieldUrlPreview() {
  return const SizedBox(
    width: 400,
    child: Padding(
      padding: EdgeInsets.all(16),
      child: OscTextField(
        hint: 'https://example.com',
        prefixIcon: Icons.link_rounded,
        accent: OscColors.green,
      ),
    ),
  );
}
