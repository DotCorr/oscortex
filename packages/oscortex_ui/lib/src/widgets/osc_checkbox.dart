import 'package:flutter/material.dart';

import '../preview/osc_preview.dart';
import '../tokens/colors.dart';

import '../tokens/spacing.dart';

/// An OSCortex-native checkbox — no Material CheckBox.
///
/// Square toggle with animated fill, border glow, and accent color.
///
/// ```dart
/// OscCheckbox(
///   value: isChecked,
///   label: 'I agree to the terms',
///   onChanged: (val) => setState(() => isChecked = val),
/// );
/// ```
class OscCheckbox extends StatefulWidget {
  const OscCheckbox({
    super.key,
    required this.value,
    this.label,
    this.onChanged,
    this.accent,
  });

  final bool value;
  final String? label;
  final ValueChanged<bool>? onChanged;
  final Color? accent;

  @override
  State<OscCheckbox> createState() => _OscCheckboxState();
}

class _OscCheckboxState extends State<OscCheckbox>
    with SingleTickerProviderStateMixin {
  late AnimationController _anim;
  late Animation<double> _scale;
  bool _hovered = false;

  @override
  void initState() {
    super.initState();
    _anim = AnimationController(
      vsync: this,
      duration: const Duration(milliseconds: 180),
      value: widget.value ? 1.0 : 0.0,
    );
    _scale = CurvedAnimation(parent: _anim, curve: Curves.easeOutBack);
  }

  @override
  void didUpdateWidget(OscCheckbox old) {
    super.didUpdateWidget(old);
    if (old.value != widget.value) {
      widget.value ? _anim.forward() : _anim.reverse();
    }
  }

  @override
  void dispose() {
    _anim.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final color = widget.accent ?? OscColors.violet;

    final box = MouseRegion(
      onEnter: (_) => setState(() => _hovered = true),
      onExit: (_) => setState(() => _hovered = false),
      child: GestureDetector(
        onTap: widget.onChanged != null
            ? () => widget.onChanged!(!widget.value)
            : null,
        child: AnimatedBuilder(
          animation: _scale,
          builder: (context, _) {
            return Container(
              width: 20,
              height: 20,
              decoration: BoxDecoration(
                color: widget.value
                    ? color.withValues(alpha: 0.15 + _scale.value * 0.10)
                    : Colors.transparent,
                borderRadius: BorderRadius.circular(5),
                border: Border.all(
                  color: widget.value
                      ? color
                      : _hovered
                          ? OscColors.borderHover
                          : OscColors.border,
                  width: widget.value ? 1.5 : 1.0,
                ),
              ),
              child: Center(
                child: Transform.scale(
                  scale: _scale.value,
                  child: Icon(
                    Icons.check_rounded,
                    size: 14,
                    color: color,
                  ),
                ),
              ),
            );
          },
        ),
      ),
    );

    if (widget.label == null) return box;

    return GestureDetector(
      onTap: widget.onChanged != null
          ? () => widget.onChanged!(!widget.value)
          : null,
      child: Row(
        mainAxisSize: MainAxisSize.min,
        children: [
          box,
          const SizedBox(width: OscSpacing.md),
          Text(
            widget.label!,
            style: TextStyle(
              fontSize: 13,
              color: _hovered ? OscColors.textPrimary : OscColors.textMuted,
            ),
          ),
        ],
      ),
    );
  }
}

// ─── Previews ──────────────────────────────────────────────────────────

@OscPreview(name: 'Unchecked', group: 'OscCheckbox')
Widget oscCheckboxUncheckedPreview() {
  return const Padding(
    padding: EdgeInsets.all(16),
    child: OscCheckbox(value: false, label: 'Enable notifications'),
  );
}

@OscPreview(name: 'Checked', group: 'OscCheckbox')
Widget oscCheckboxCheckedPreview() {
  return const Padding(
    padding: EdgeInsets.all(16),
    child: OscCheckbox(value: true, label: 'I agree to the terms'),
  );
}

@OscPreview(name: 'All Accents', group: 'OscCheckbox')
Widget oscCheckboxAccentsPreview() {
  return const Padding(
    padding: EdgeInsets.all(16),
    child: Column(
      mainAxisSize: MainAxisSize.min,
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        OscCheckbox(value: true, label: 'Violet default', accent: OscColors.violet),
        SizedBox(height: 12),
        OscCheckbox(value: true, label: 'Sky blue', accent: OscColors.skyBlue),
        SizedBox(height: 12),
        OscCheckbox(value: true, label: 'Emerald', accent: OscColors.green),
        SizedBox(height: 12),
        OscCheckbox(value: false, label: 'Unchecked'),
      ],
    ),
  );
}
