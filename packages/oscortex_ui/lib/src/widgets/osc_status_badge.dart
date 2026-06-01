import 'package:flutter/material.dart';

import '../tokens/colors.dart';
import '../tokens/radii.dart';

/// A status badge with an animated pulsing dot.
///
/// ```dart
/// OscStatusBadge(label: 'Auto-saving', color: OscColors.green)
/// OscStatusBadge(label: 'Live', color: OscColors.violetLight)
/// ```
class OscStatusBadge extends StatefulWidget {
  const OscStatusBadge({
    super.key,
    required this.label,
    this.color = OscColors.green,
    this.pulseDuration = const Duration(milliseconds: 1800),
  });

  final String label;

  /// Dot and text color.
  final Color color;

  /// Duration of one full pulse cycle.
  final Duration pulseDuration;

  @override
  State<OscStatusBadge> createState() => _OscStatusBadgeState();
}

class _OscStatusBadgeState extends State<OscStatusBadge>
    with SingleTickerProviderStateMixin {
  late final AnimationController _ctrl;
  late final Animation<double> _pulse;

  @override
  void initState() {
    super.initState();
    _ctrl = AnimationController(vsync: this, duration: widget.pulseDuration)
      ..repeat(reverse: true);
    _pulse = Tween(begin: 1.0, end: 0.4).animate(
      CurvedAnimation(parent: _ctrl, curve: Curves.easeInOut),
    );
  }

  @override
  void dispose() {
    _ctrl.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    return Container(
      padding: const EdgeInsets.symmetric(horizontal: 10, vertical: 4),
      decoration: BoxDecoration(
        borderRadius: OscRadii.pillBorder,
        border: Border.all(color: widget.color.withValues(alpha: 0.20)),
        color: widget.color.withValues(alpha: 0.10),
      ),
      child: Row(
        mainAxisSize: MainAxisSize.min,
        children: [
          AnimatedBuilder(
            animation: _pulse,
            builder: (_, child) => Opacity(
              opacity: _pulse.value,
              child: Transform.scale(
                scale: 0.85 + (_pulse.value * 0.15),
                child: child,
              ),
            ),
            child: Container(
              width: 6,
              height: 6,
              decoration: BoxDecoration(
                shape: BoxShape.circle,
                color: widget.color,
              ),
            ),
          ),
          const SizedBox(width: 6),
          Text(
            widget.label,
            style: TextStyle(
              fontSize: 9,
              fontWeight: FontWeight.w500,
              color: widget.color,
            ),
          ),
        ],
      ),
    );
  }
}
