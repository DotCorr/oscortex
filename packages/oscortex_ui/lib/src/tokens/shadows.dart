import 'package:flutter/material.dart';

/// OSCortex canonical box shadows.
abstract final class OscShadows {
  /// Deep atmospheric shadow for canvas frame.
  /// `0 48px 120px -24px rgba(0,0,0,0.7)`
  static const BoxShadow frame = BoxShadow(
    color: Color(0xB3000000),
    blurRadius: 120,
    offset: Offset(0, 48),
    spreadRadius: -24,
  );

  /// Card elevation shadow.
  static const BoxShadow card = BoxShadow(
    color: Color(0x80000000),
    blurRadius: 24,
    offset: Offset(0, 8),
    spreadRadius: -12,
  );

  /// Hub dock shadow.
  static const BoxShadow hubDock = BoxShadow(
    color: Color(0x99000000),
    blurRadius: 40,
    offset: Offset(0, 12),
    spreadRadius: -8,
  );

  /// Hover-state elevated shadow.
  static const BoxShadow hover = BoxShadow(
    color: Color(0x80000000),
    blurRadius: 20,
    offset: Offset(0, 6),
    spreadRadius: -8,
  );

  /// Subtle shadow for small elements.
  static const BoxShadow subtle = BoxShadow(
    color: Color(0x40000000),
    blurRadius: 12,
    offset: Offset(0, 4),
    spreadRadius: -6,
  );
}
