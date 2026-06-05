import 'package:flutter/material.dart';
import 'package:flutter/widget_previews.dart';

import '../theme/osc_theme.dart';
import '../tokens/colors.dart';

/// Custom preview annotation that pre-configures the OSCortex dark theme.
///
/// Use instead of `@Preview(...)` to get consistent theming in previews:
///
/// ```dart
/// @OscPreview(name: 'My Card')
/// Widget myCardPreview() => OscCard(child: Text('Hello'));
/// ```
final class OscPreview extends Preview {
  const OscPreview({
    super.name,
    super.group,
    super.size,
    super.textScaleFactor,
    super.wrapper,
    super.brightness = Brightness.dark,
    super.localizations,
  }) : super(theme: OscPreview._theme);

  static PreviewThemeData _theme() {
    return PreviewThemeData(
      materialDark: OscTheme.dark(),
      materialLight: OscTheme.dark(), // OSCortex is always dark
    );
  }
}

/// Multi-preview that shows both violet and sky-blue accent variants.
final class OscAccentPreview extends MultiPreview {
  const OscAccentPreview({this.previewName = 'Widget'});

  final String previewName;

  @override
  List<Preview> get previews => [
        Preview(
          group: 'Accents',
          name: '$previewName — Violet',
          brightness: Brightness.dark,
          theme: () => PreviewThemeData(
            materialDark: OscTheme.dark(accent: OscColors.violet),
          ),
        ),
        Preview(
          group: 'Accents',
          name: '$previewName — Sky Blue',
          brightness: Brightness.dark,
          theme: () => PreviewThemeData(
            materialDark: OscTheme.dark(accent: OscColors.skyBlue),
          ),
        ),
        Preview(
          group: 'Accents',
          name: '$previewName — Emerald',
          brightness: Brightness.dark,
          theme: () => PreviewThemeData(
            materialDark: OscTheme.dark(accent: OscColors.green),
          ),
        ),
      ];
}

