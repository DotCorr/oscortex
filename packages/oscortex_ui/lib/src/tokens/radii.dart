import 'package:flutter/material.dart';

/// OSCortex canonical border radii.
abstract final class OscRadii {
  /// Pill / fully-rounded (hub dock, badges). `999px`
  static const Radius pill = Radius.circular(999);
  static const BorderRadius pillBorder = BorderRadius.all(pill);

  /// Canvas frame radius. `20px`
  static const double frame = 20;
  static const BorderRadius frameBorder = BorderRadius.all(Radius.circular(20));

  /// Surface card radius. `14px`
  static const double card = 14;
  static const BorderRadius cardBorder = BorderRadius.all(Radius.circular(14));

  /// Inner element radius. `12px`
  static const double inner = 12;
  static const BorderRadius innerBorder = BorderRadius.all(Radius.circular(12));

  /// Button / chip radius. `10px`
  static const double button = 10;
  static const BorderRadius buttonBorder = BorderRadius.all(Radius.circular(10));

  /// Icon container radius. `8px`
  static const double icon = 8;
  static const BorderRadius iconBorder = BorderRadius.all(Radius.circular(8));

  /// Tag / small chip radius. `6px`
  static const double tag = 6;
  static const BorderRadius tagBorder = BorderRadius.all(Radius.circular(6));

  /// Tiny radius. `4px`
  static const double tiny = 4;
  static const BorderRadius tinyBorder = BorderRadius.all(Radius.circular(4));
}
