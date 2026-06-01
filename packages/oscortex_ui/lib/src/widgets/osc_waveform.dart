import 'dart:math' as math;

import 'package:flutter/material.dart';

import '../tokens/colors.dart';

/// Animated audio waveform bars matching the spec's media player.
///
/// ```dart
/// OscWaveform(barCount: 8, height: 32)
/// OscWaveform(barCount: 6, height: 24, color: OscColors.green)
/// ```
class OscWaveform extends StatefulWidget {
  const OscWaveform({
    super.key,
    this.barCount = 8,
    this.height = 32,
    this.barWidth = 3,
    this.barGap = 3,
    this.colorStart = OscColors.violet,
    this.colorEnd = OscColors.violetLight,
    this.duration = const Duration(milliseconds: 1200),
  });

  final int barCount;
  final double height;
  final double barWidth;
  final double barGap;

  /// Bottom gradient color.
  final Color colorStart;

  /// Top gradient color.
  final Color colorEnd;

  /// Full animation cycle duration.
  final Duration duration;

  @override
  State<OscWaveform> createState() => _OscWaveformState();
}

class _OscWaveformState extends State<OscWaveform>
    with SingleTickerProviderStateMixin {
  late final AnimationController _ctrl;

  @override
  void initState() {
    super.initState();
    _ctrl = AnimationController(vsync: this, duration: widget.duration)
      ..repeat(reverse: true);
  }

  @override
  void dispose() {
    _ctrl.dispose();
    super.dispose();
  }

  // Pre-computed relative heights for each bar position.
  static const _relativeHeights = [
    0.36, 0.72, 0.43, 0.86, 0.57, 1.0, 0.64, 0.36,
    0.50, 0.78, 0.42, 0.90, 0.55, 0.70, 0.48, 0.82,
  ];

  @override
  Widget build(BuildContext context) {
    return SizedBox(
      height: widget.height,
      child: Row(
        mainAxisSize: MainAxisSize.min,
        crossAxisAlignment: CrossAxisAlignment.end,
        children: List.generate(widget.barCount, (i) {
          final delay = i / widget.barCount;
          final relH = _relativeHeights[i % _relativeHeights.length];
          final maxH = widget.height * relH;

          return AnimatedBuilder(
            animation: _ctrl,
            builder: (_, _) {
              final phase = (_ctrl.value + delay) % 1.0;
              final scale = 0.3 + 0.7 * math.sin(phase * math.pi);
              return Container(
                width: widget.barWidth,
                height: maxH * scale,
                margin: EdgeInsets.only(
                  left: i == 0 ? 0 : widget.barGap,
                ),
                decoration: BoxDecoration(
                  borderRadius: BorderRadius.circular(999),
                  gradient: LinearGradient(
                    begin: Alignment.bottomCenter,
                    end: Alignment.topCenter,
                    colors: [widget.colorStart, widget.colorEnd],
                  ),
                ),
              );
            },
          );
        }),
      ),
    );
  }
}
