import 'dart:math' as math;

import 'package:flutter/material.dart';

import '../preview/osc_preview.dart';
import '../tokens/colors.dart';

import '../tokens/spacing.dart';

/// An OSCortex-native slider — no Material Slider.
///
/// Custom track + thumb with glow trail and accent color.
///
/// ```dart
/// OscSlider(
///   value: 0.5,
///   min: 0, max: 100,
///   onChanged: (v) => setState(() => vol = v),
/// );
/// ```
class OscSlider extends StatefulWidget {
  const OscSlider({
    super.key,
    required this.value,
    this.min = 0,
    this.max = 1,
    this.onChanged,
    this.accent,
    this.showLabel = false,
  });

  final double value;
  final double min;
  final double max;
  final ValueChanged<double>? onChanged;
  final Color? accent;
  final bool showLabel;

  @override
  State<OscSlider> createState() => _OscSliderState();
}

class _OscSliderState extends State<OscSlider> {
  bool _dragging = false;
  bool _hovered = false;

  double get _fraction {
    if (widget.max <= widget.min) return 0;
    return ((widget.value - widget.min) / (widget.max - widget.min))
        .clamp(0.0, 1.0);
  }

  void _onDrag(double localX, double trackWidth) {
    if (widget.onChanged == null) return;
    final fraction = (localX / trackWidth).clamp(0.0, 1.0);
    final val = widget.min + fraction * (widget.max - widget.min);
    widget.onChanged!(val);
  }

  @override
  Widget build(BuildContext context) {
    final color = widget.accent ?? OscColors.violet;

    return MouseRegion(
      onEnter: (_) => setState(() => _hovered = true),
      onExit: (_) => setState(() => _hovered = false),
      child: LayoutBuilder(
        builder: (context, constraints) {
          final trackWidth = constraints.maxWidth;
          const trackHeight = 6.0;
          const thumbRadius = 8.0;
          const totalHeight = thumbRadius * 2 + 4;

          return GestureDetector(
            onPanStart: (d) {
              setState(() => _dragging = true);
              _onDrag(d.localPosition.dx, trackWidth);
            },
            onPanUpdate: (d) => _onDrag(d.localPosition.dx, trackWidth),
            onPanEnd: (_) => setState(() => _dragging = false),
            onTapDown: (d) => _onDrag(d.localPosition.dx, trackWidth),
            child: SizedBox(
              height: totalHeight + (widget.showLabel ? 20 : 0),
              child: Column(
                mainAxisSize: MainAxisSize.min,
                children: [
                  SizedBox(
                    height: totalHeight,
                    child: Stack(
                      clipBehavior: Clip.none,
                      children: [
                        // Inactive track
                        Positioned(
                          top: (totalHeight - trackHeight) / 2,
                          left: 0,
                          right: 0,
                          height: trackHeight,
                          child: Container(
                            decoration: BoxDecoration(
                              color: Colors.white.withValues(alpha: 0.06),
                              borderRadius:
                                  BorderRadius.circular(trackHeight / 2),
                            ),
                          ),
                        ),
                        // Active track
                        Positioned(
                          top: (totalHeight - trackHeight) / 2,
                          left: 0,
                          width: _fraction * trackWidth,
                          height: trackHeight,
                          child: Container(
                            decoration: BoxDecoration(
                              gradient: LinearGradient(
                                colors: [
                                  color.withValues(alpha: 0.5),
                                  color,
                                ],
                              ),
                              borderRadius:
                                  BorderRadius.circular(trackHeight / 2),
                              boxShadow: _dragging || _hovered
                                  ? [
                                      BoxShadow(
                                        color: color.withValues(alpha: 0.25),
                                        blurRadius: 8,
                                      ),
                                    ]
                                  : null,
                            ),
                          ),
                        ),
                        // Thumb
                        Positioned(
                          top: (totalHeight - thumbRadius * 2) / 2,
                          left: math.max(
                              0,
                              _fraction * trackWidth - thumbRadius),
                          child: AnimatedContainer(
                            duration: const Duration(milliseconds: 120),
                            width: thumbRadius * 2,
                            height: thumbRadius * 2,
                            decoration: BoxDecoration(
                              shape: BoxShape.circle,
                              color: color,
                              border: Border.all(
                                color: Colors.white.withValues(alpha: 0.25),
                                width: 2,
                              ),
                              boxShadow: [
                                BoxShadow(
                                  color: color.withValues(
                                      alpha: _dragging ? 0.5 : 0.2),
                                  blurRadius: _dragging ? 12 : 4,
                                ),
                              ],
                            ),
                          ),
                        ),
                      ],
                    ),
                  ),
                  if (widget.showLabel) ...[
                    const SizedBox(height: OscSpacing.xs),
                    Row(
                      mainAxisAlignment: MainAxisAlignment.spaceBetween,
                      children: [
                        Text(
                          widget.min.toStringAsFixed(
                              widget.min == widget.min.roundToDouble() ? 0 : 1),
                          style: const TextStyle(
                              fontSize: 10, color: OscColors.textDim),
                        ),
                        Text(
                          widget.value.toStringAsFixed(
                              widget.value == widget.value.roundToDouble()
                                  ? 0
                                  : 1),
                          style: TextStyle(
                            fontSize: 10,
                            fontWeight: FontWeight.w600,
                            color: color,
                          ),
                        ),
                        Text(
                          widget.max.toStringAsFixed(
                              widget.max == widget.max.roundToDouble() ? 0 : 1),
                          style: const TextStyle(
                              fontSize: 10, color: OscColors.textDim),
                        ),
                      ],
                    ),
                  ],
                ],
              ),
            ),
          );
        },
      ),
    );
  }
}

// ─── Previews ──────────────────────────────────────────────────────────

@OscPreview(name: 'Default Slider', group: 'OscSlider')
Widget oscSliderPreview() {
  return const SizedBox(
    width: 300,
    child: Padding(
      padding: EdgeInsets.all(24),
      child: OscSlider(value: 0.5),
    ),
  );
}

@OscPreview(name: 'Volume Slider', group: 'OscSlider')
Widget oscSliderVolumePreview() {
  return const SizedBox(
    width: 300,
    child: Padding(
      padding: EdgeInsets.all(24),
      child: OscSlider(
        value: 75,
        min: 0,
        max: 100,
        showLabel: true,
        accent: OscColors.skyBlue,
      ),
    ),
  );
}

@OscPreview(name: 'Green Range', group: 'OscSlider')
Widget oscSliderGreenPreview() {
  return const SizedBox(
    width: 300,
    child: Padding(
      padding: EdgeInsets.all(24),
      child: OscSlider(
        value: 30,
        min: 0,
        max: 100,
        showLabel: true,
        accent: OscColors.green,
      ),
    ),
  );
}
