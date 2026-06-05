import 'package:flutter/material.dart';

/// OSCortex canonical typography scale.
///
/// Font families, weights, and pre-built [TextStyle]s for consistent
/// text rendering across all OSCortex apps.
///
/// The bare-metal OS bundles NotoSans as the primary font.
/// RobotoMono is used for code / terminal contexts (falls back to
/// the platform's default monospace if unavailable).
abstract final class OscTypography {
  // ── Font Families ────────────────────────────────────────────────────
  /// Primary UI font (bundled in the OS initramfs).
  static const String fontFamily = 'NotoSans';

  /// Monospace font for code, terminal, labels.
  static const String monoFamily = 'RobotoMono';

  // ── Heading Styles ───────────────────────────────────────────────────
  /// App title — 32px, extra-bold.
  static const TextStyle heading1 = TextStyle(
    fontSize: 32,
    fontWeight: FontWeight.w800,
    color: Color(0xFFE5E5E5),
    height: 1.2,
  );

  /// Section title — 28px, extra-bold.
  static const TextStyle heading2 = TextStyle(
    fontSize: 28,
    fontWeight: FontWeight.w800,
    color: Color(0xFFE5E5E5),
    height: 1.25,
  );

  /// Card title — 15px, semi-bold.
  static const TextStyle heading3 = TextStyle(
    fontSize: 15,
    fontWeight: FontWeight.w600,
    color: Colors.white,
    height: 1.3,
  );

  // ── Body Styles ──────────────────────────────────────────────────────
  /// Standard body text — 13px, medium.
  static const TextStyle body = TextStyle(
    fontSize: 13,
    fontWeight: FontWeight.w500,
    color: Color(0xFFE5E5E5),
    height: 1.5,
  );

  /// Secondary body text — 12px, normal.
  static const TextStyle bodySmall = TextStyle(
    fontSize: 12,
    fontWeight: FontWeight.w400,
    height: 1.75,
    color: Color(0x73FFFFFF), // ~0.45 alpha
  );

  /// Description / subtitle — 11px, muted.
  static const TextStyle caption = TextStyle(
    fontSize: 11,
    fontWeight: FontWeight.w400,
    color: Color(0xFF71717A),
    height: 1.4,
  );

  // ── Label Styles ─────────────────────────────────────────────────────
  /// Top chrome / toolbar title — 13px, semi-bold.
  static const TextStyle toolbarTitle = TextStyle(
    fontSize: 13,
    fontWeight: FontWeight.w600,
    color: Color(0xFFE5E5E5),
  );

  /// Status bar text — 11px, muted.
  static const TextStyle statusText = TextStyle(
    fontSize: 11,
    color: Color(0x66FFFFFF), // ~0.40 alpha
  );

  /// Clock display — 12px, tabular figures.
  static const TextStyle clock = TextStyle(
    fontSize: 12,
    fontWeight: FontWeight.w500,
    color: Color(0xFF71717A),
    fontFeatures: [FontFeature.tabularFigures()],
  );

  // ── Mono Styles ──────────────────────────────────────────────────────
  /// Mono label (section headers like "DOCUMENT CANVAS") — 10px, bold.
  static const TextStyle monoLabel = TextStyle(
    fontFamily: monoFamily,
    fontSize: 10,
    fontWeight: FontWeight.w600,
    letterSpacing: 1.4,
  );

  /// Smaller mono label — 9px (used for "NOW PLAYING", etc.)
  static const TextStyle monoLabelSmall = TextStyle(
    fontFamily: monoFamily,
    fontSize: 9,
    fontWeight: FontWeight.w600,
    letterSpacing: 1.2,
  );

  /// Mono body — 12px, for code text / prompts.
  static const TextStyle monoBody = TextStyle(
    fontFamily: monoFamily,
    fontSize: 12,
    fontWeight: FontWeight.w400,
  );

  /// Mono URL / small code — 10px.
  static const TextStyle monoSmall = TextStyle(
    fontFamily: monoFamily,
    fontSize: 10,
    fontWeight: FontWeight.w400,
  );

  /// Intent bar prompt caret — 13px, bold.
  static const TextStyle monoPrompt = TextStyle(
    fontFamily: monoFamily,
    fontSize: 13,
    fontWeight: FontWeight.w700,
  );

  // ── Tag / Badge Styles ───────────────────────────────────────────────
  /// Tag chip text — 9px mono.
  static const TextStyle tag = TextStyle(
    fontFamily: monoFamily,
    fontSize: 9,
    fontWeight: FontWeight.w400,
  );

  /// Badge text (e.g. "Auto-saving", "Live") — 9px, medium.
  static const TextStyle badge = TextStyle(
    fontSize: 9,
    fontWeight: FontWeight.w500,
  );

  /// Rail button label — 7.5px.
  static const TextStyle railLabel = TextStyle(
    fontSize: 7.5,
    fontWeight: FontWeight.w500,
  );

  /// Settings button — 11px.
  static const TextStyle settingsButton = TextStyle(
    fontSize: 11,
    fontWeight: FontWeight.w400,
  );

  /// Hub dock segment — 12px.
  static const TextStyle hubSegment = TextStyle(
    fontSize: 12,
  );
}
