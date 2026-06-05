import 'package:flutter/material.dart';

import '../tokens/colors.dart';
import '../tokens/radii.dart';
import '../tokens/typography.dart';

/// Builds a canonical OSCortex [ThemeData].
///
/// ```dart
/// MaterialApp(
///   theme: OscTheme.dark(),            // default violet accent
///   theme: OscTheme.dark(accent: OscColors.skyBlue), // Files app
/// )
/// ```
abstract final class OscTheme {
  /// Build the standard OSCortex dark theme.
  ///
  /// [accent] overrides the primary accent color (default: violet).
  /// [fontFamily] overrides the base font (default: NotoSans).
  static ThemeData dark({
    Color accent = OscColors.violet,
    String fontFamily = OscTypography.fontFamily,
  }) {
    final colorScheme = ColorScheme.dark(
      primary: accent,
      surface: OscColors.canvasBg,
      onSurface: OscColors.textPrimary,
      error: OscColors.red,
    );

    return ThemeData(
      brightness: Brightness.dark,
      colorScheme: colorScheme,
      fontFamily: fontFamily,
      useMaterial3: true,
      scaffoldBackgroundColor: OscColors.bodyBg,

      // ── Card ─────────────────────────────────────────────────────
      cardTheme: CardThemeData(
        color: OscColors.surface,
        shape: RoundedRectangleBorder(
          borderRadius: OscRadii.cardBorder,
          side: const BorderSide(color: OscColors.border),
        ),
        elevation: 0,
        margin: EdgeInsets.zero,
      ),

      // ── Filled Button ────────────────────────────────────────────
      filledButtonTheme: FilledButtonThemeData(
        style: FilledButton.styleFrom(
          backgroundColor: accent,
          foregroundColor: Colors.white,
          shape: RoundedRectangleBorder(
            borderRadius: OscRadii.buttonBorder,
          ),
          minimumSize: const Size(0, 36),
          padding: const EdgeInsets.symmetric(horizontal: 16, vertical: 8),
          textStyle: const TextStyle(
            fontSize: 12,
            fontWeight: FontWeight.w600,
          ),
        ),
      ),

      // ── Outlined Button ──────────────────────────────────────────
      outlinedButtonTheme: OutlinedButtonThemeData(
        style: OutlinedButton.styleFrom(
          foregroundColor: OscColors.textPrimary,
          side: const BorderSide(color: OscColors.border),
          shape: RoundedRectangleBorder(
            borderRadius: OscRadii.buttonBorder,
          ),
          minimumSize: const Size(0, 36),
          padding: const EdgeInsets.symmetric(horizontal: 16, vertical: 8),
          textStyle: const TextStyle(
            fontSize: 12,
            fontWeight: FontWeight.w500,
          ),
        ),
      ),

      // ── Icon Button ──────────────────────────────────────────────
      iconButtonTheme: IconButtonThemeData(
        style: IconButton.styleFrom(
          foregroundColor: OscColors.textMuted,
        ),
      ),

      // ── Input Decoration ─────────────────────────────────────────
      inputDecorationTheme: InputDecorationTheme(
        filled: true,
        fillColor: OscColors.surface,
        contentPadding: const EdgeInsets.symmetric(
          horizontal: 14,
          vertical: 12,
        ),
        border: OutlineInputBorder(
          borderRadius: OscRadii.innerBorder,
          borderSide: const BorderSide(color: OscColors.border),
        ),
        enabledBorder: OutlineInputBorder(
          borderRadius: OscRadii.innerBorder,
          borderSide: const BorderSide(color: OscColors.border),
        ),
        focusedBorder: OutlineInputBorder(
          borderRadius: OscRadii.innerBorder,
          borderSide: BorderSide(color: accent),
        ),
        labelStyle: const TextStyle(
          fontSize: 12,
          color: OscColors.textMuted,
        ),
        hintStyle: const TextStyle(
          fontSize: 12,
          color: OscColors.textDim,
        ),
      ),

      // ── Chip ─────────────────────────────────────────────────────
      chipTheme: ChipThemeData(
        backgroundColor: OscColors.surface,
        side: const BorderSide(color: OscColors.border),
        shape: RoundedRectangleBorder(
          borderRadius: OscRadii.tagBorder,
        ),
        labelStyle: const TextStyle(
          fontSize: 11,
          color: OscColors.textPrimary,
        ),
        padding: const EdgeInsets.symmetric(horizontal: 8, vertical: 2),
      ),

      // ── Popup Menu ───────────────────────────────────────────────
      popupMenuTheme: PopupMenuThemeData(
        color: OscColors.canvasBg,
        shape: RoundedRectangleBorder(
          borderRadius: OscRadii.innerBorder,
          side: const BorderSide(color: OscColors.border),
        ),
        textStyle: const TextStyle(
          fontSize: 12,
          color: OscColors.textPrimary,
        ),
      ),

      // ── Snackbar ─────────────────────────────────────────────────
      snackBarTheme: SnackBarThemeData(
        backgroundColor: OscColors.canvasBg,
        contentTextStyle: const TextStyle(
          fontSize: 12,
          color: OscColors.textPrimary,
        ),
        shape: RoundedRectangleBorder(
          borderRadius: OscRadii.innerBorder,
          side: const BorderSide(color: OscColors.border),
        ),
        behavior: SnackBarBehavior.floating,
      ),

      // ── Divider ──────────────────────────────────────────────────
      dividerTheme: const DividerThemeData(
        color: OscColors.border,
        thickness: 1,
        space: 0,
      ),

      // ── Tooltip ──────────────────────────────────────────────────
      tooltipTheme: TooltipThemeData(
        decoration: BoxDecoration(
          color: OscColors.canvasBg,
          borderRadius: OscRadii.tagBorder,
          border: Border.all(color: OscColors.border),
        ),
        textStyle: const TextStyle(
          fontSize: 11,
          color: OscColors.textPrimary,
        ),
      ),

      // ── Scrollbar ────────────────────────────────────────────────
      scrollbarTheme: ScrollbarThemeData(
        thumbColor: WidgetStateProperty.all(
          OscColors.border,
        ),
        radius: const Radius.circular(4),
        thickness: WidgetStateProperty.all(4.0),
      ),

      // ── List Tile ────────────────────────────────────────────────
      listTileTheme: ListTileThemeData(
        tileColor: OscColors.surface,
        shape: RoundedRectangleBorder(
          borderRadius: OscRadii.cardBorder,
        ),
        contentPadding: const EdgeInsets.symmetric(
          horizontal: 16,
          vertical: 4,
        ),
      ),
    );
  }
}
