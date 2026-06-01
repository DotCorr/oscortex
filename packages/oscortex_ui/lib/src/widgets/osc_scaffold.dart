import 'package:flutter/material.dart';


import '../preview/osc_preview.dart';
import '../tokens/colors.dart';
import '../tokens/radii.dart';
import '../tokens/shadows.dart';
import '../tokens/spacing.dart';

/// The full-bleed app scaffold with ambient gradient layer.
///
/// Wraps your app's body in the canonical OSCortex frame:
/// body bg → ambient gradients → rounded canvas frame → content.
///
/// ```dart
/// OscScaffold(
///   accent: OscColors.violet,
///   body: Column(children: [toolbar, content, hubDock]),
/// )
/// ```
class OscScaffold extends StatelessWidget {
  const OscScaffold({
    super.key,
    required this.body,
    this.accent = OscColors.violet,
    this.showFrame = true,
    this.padding = const EdgeInsets.all(OscSpacing.xl3),
  });

  final Widget body;

  /// Primary accent for the ambient gradient.
  final Color accent;

  /// Whether to wrap content in the rounded canvas frame.
  final bool showFrame;

  /// Outer padding around the frame.
  final EdgeInsets padding;

  @override
  Widget build(BuildContext context) {
    Widget content = body;

    if (showFrame) {
      content = Padding(
        padding: padding,
        child: DecoratedBox(
          decoration: BoxDecoration(
            color: OscColors.canvasBg,
            borderRadius: OscRadii.frameBorder,
            border: Border.all(color: OscColors.borderLight),
            boxShadow: const [OscShadows.frame],
          ),
          child: ClipRRect(
            borderRadius: OscRadii.frameBorder,
            child: Stack(
              children: [
                // Ambient layer
                Positioned.fill(
                  child: _AmbientLayer(accent: accent),
                ),
                content,
              ],
            ),
          ),
        ),
      );
    }

    return Scaffold(
      body: Container(
        decoration: BoxDecoration(
          color: OscColors.bodyBg,
          gradient: RadialGradient(
            center: const Alignment(0, -1.2),
            radius: 1.1,
            colors: [accent.withValues(alpha: 0.20), const Color(0x00070808)],
          ),
        ),
        child: SafeArea(child: content),
      ),
    );
  }
}

class _AmbientLayer extends StatelessWidget {
  const _AmbientLayer({required this.accent});

  final Color accent;

  @override
  Widget build(BuildContext context) {
    return DecoratedBox(
      decoration: BoxDecoration(
        gradient: RadialGradient(
          center: Alignment.topLeft,
          radius: 1.4,
          colors: [
            accent.withValues(alpha: 0.14),
            Colors.transparent,
          ],
        ),
      ),
      child: DecoratedBox(
        decoration: BoxDecoration(
          gradient: RadialGradient(
            center: const Alignment(0.85, -0.3),
            radius: 1.1,
            colors: [
              OscColors.skyBlue.withValues(alpha: 0.05),
              Colors.transparent,
            ],
          ),
        ),
      ),
    );
  }
}

// ---------------------------------------------------------------------------
// Previews
// ---------------------------------------------------------------------------

@OscPreview(name: 'Violet Scaffold', group: 'OscScaffold')
Widget oscScaffoldVioletPreview() {
  return OscScaffold(
    accent: OscColors.violet,
    body: Center(
      child: Text('Violet Accent Scaffold',
          style: TextStyle(color: Color(0xFFD4D4D8), fontSize: 14)),
    ),
  );
}

@OscPreview(name: 'Sky Blue Scaffold', group: 'OscScaffold')
Widget oscScaffoldSkyPreview() {
  return OscScaffold(
    accent: OscColors.skyBlue,
    body: Center(
      child: Text('Sky Blue Accent Scaffold',
          style: TextStyle(color: Color(0xFFD4D4D8), fontSize: 14)),
    ),
  );
}

@OscPreview(name: 'Frameless Scaffold', group: 'OscScaffold')
Widget oscScaffoldFramelessPreview() {
  return OscScaffold(
    accent: OscColors.green,
    showFrame: false,
    body: Center(
      child: Text('No Frame — Emerald',
          style: TextStyle(color: Color(0xFFD4D4D8), fontSize: 14)),
    ),
  );
}
