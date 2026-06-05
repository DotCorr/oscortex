import 'package:flutter/material.dart';

/// OSCortex canonical color palette.
///
/// Derived from [oscortex-ui-spec.html] design tokens.
/// Use these instead of hardcoding hex values in app code.
abstract final class OscColors {
  // ── Backgrounds ──────────────────────────────────────────────────────
  /// Deepest body background.  `#07080B`
  static const Color bodyBg = Color(0xFF07080B);

  /// Canvas shell / frame background.  `#0B0D12`
  static const Color canvasBg = Color(0xFF0B0D12);

  /// Elevated surface (cards, tiles).  `rgba(255,255,255,0.025)`
  static const Color surface = Color(0x06FFFFFF);

  /// Slightly brighter surface for hover states.
  static const Color surfaceHover = Color(0x09FFFFFF);

  /// Dark overlay used in rails, sidebars.
  static const Color overlay = Color(0x33000000);

  // ── Accents ──────────────────────────────────────────────────────────
  /// Primary accent — violet.  `#7C3AED`
  static const Color violet = Color(0xFF7C3AED);

  /// Lighter violet for text / icons on dark.  `#A78BFA`
  static const Color violetLight = Color(0xFFA78BFA);

  /// Sky blue — used by Files app and secondary highlights.  `#38BDF8`
  static const Color skyBlue = Color(0xFF38BDF8);

  /// Signal green — success, auto-save, healthy.  `#10B981`
  static const Color green = Color(0xFF10B981);

  /// Signal red — errors, destructive actions.  `#EF4444`
  static const Color red = Color(0xFFEF4444);

  /// Amber — warnings.  `#F59E0B`
  static const Color amber = Color(0xFFF59E0B);

  // ── Borders ──────────────────────────────────────────────────────────
  /// Standard subtle border.  `rgba(255,255,255,0.08)`
  static const Color border = Color(0x14FFFFFF);

  /// Lighter border for outer frames.  `rgba(255,255,255,0.05)`
  static const Color borderLight = Color(0x0DFFFFFF);

  /// Active / focused border (violet tint).  `rgba(167,139,250,0.25)`
  static const Color borderActive = Color(0x40A78BFA);

  /// Hover-state border.  `rgba(255,255,255,0.12)`
  static const Color borderHover = Color(0x1FFFFFFF);

  // ── Text ─────────────────────────────────────────────────────────────
  /// Primary text.  `#D4D4D8`
  static const Color textPrimary = Color(0xFFD4D4D8);

  /// Bright white text for headings.  `#E5E5E5`
  static const Color textBright = Color(0xFFE5E5E5);

  /// Muted text for secondary labels.  `#71717A`
  static const Color textMuted = Color(0xFF71717A);

  /// Dim text for tertiary / disabled labels.  `#52525B`
  static const Color textDim = Color(0xFF52525B);

  // ── Helpers ──────────────────────────────────────────────────────────
  /// Return the canonical accent for a named app role.
  static Color accentFor(String role) {
    return switch (role) {
      'canvas' => violet,
      'files' => skyBlue,
      'web' => green,
      _ => violet,
    };
  }
}
