import 'package:flutter/material.dart';

import '../preview/osc_preview.dart';
import '../tokens/colors.dart';
import '../tokens/radii.dart';
import '../tokens/spacing.dart';

/// An OSCortex-native date/time picker — no Material showDatePicker.
///
/// Inline scrollable wheel selector for day, month, year.
///
/// ```dart
/// OscDatePicker(
///   value: DateTime.now(),
///   onChanged: (date) => setState(() => selected = date),
/// );
/// ```
class OscDatePicker extends StatefulWidget {
  const OscDatePicker({
    super.key,
    this.value,
    this.onChanged,
    this.accent,
    this.firstYear = 2000,
    this.lastYear = 2100,
  });

  final DateTime? value;
  final ValueChanged<DateTime>? onChanged;
  final Color? accent;
  final int firstYear;
  final int lastYear;

  @override
  State<OscDatePicker> createState() => _OscDatePickerState();
}

class _OscDatePickerState extends State<OscDatePicker> {
  late int _year;
  late int _month;
  late int _day;

  late FixedExtentScrollController _yearCtrl;
  late FixedExtentScrollController _monthCtrl;
  late FixedExtentScrollController _dayCtrl;

  @override
  void initState() {
    super.initState();
    final d = widget.value ?? DateTime.now();
    _year = d.year;
    _month = d.month;
    _day = d.day;
    _yearCtrl = FixedExtentScrollController(
        initialItem: _year - widget.firstYear);
    _monthCtrl = FixedExtentScrollController(initialItem: _month - 1);
    _dayCtrl = FixedExtentScrollController(initialItem: _day - 1);
  }

  @override
  void dispose() {
    _yearCtrl.dispose();
    _monthCtrl.dispose();
    _dayCtrl.dispose();
    super.dispose();
  }

  int get _daysInMonth => DateUtils.getDaysInMonth(_year, _month);

  void _emit() {
    final clamped = _day.clamp(1, _daysInMonth);
    widget.onChanged?.call(DateTime(
      _year,
      _month,
      clamped,
    ));
  }

  static const _months = [
    'Jan', 'Feb', 'Mar', 'Apr', 'May', 'Jun',
    'Jul', 'Aug', 'Sep', 'Oct', 'Nov', 'Dec',
  ];

  @override
  Widget build(BuildContext context) {
    final color = widget.accent ?? OscColors.violet;

    return Container(
      height: 160,
      decoration: BoxDecoration(
        color: OscColors.surface,
        borderRadius: OscRadii.cardBorder,
        border: Border.all(color: OscColors.border),
      ),
      child: Stack(
        children: [
          // Selection highlight band
          Center(
            child: Container(
              height: 36,
              margin: const EdgeInsets.symmetric(horizontal: OscSpacing.sm),
              decoration: BoxDecoration(
                color: color.withValues(alpha: 0.08),
                borderRadius: OscRadii.buttonBorder,
                border: Border.all(color: color.withValues(alpha: 0.20)),
              ),
            ),
          ),
          // Wheels
          Row(
            children: [
              // Day
              Expanded(
                child: _wheel(
                  controller: _dayCtrl,
                  count: _daysInMonth,
                  labelBuilder: (i) => '${i + 1}'.padLeft(2, '0'),
                  onChanged: (i) {
                    _day = i + 1;
                    _emit();
                  },
                  color: color,
                ),
              ),
              // Month
              Expanded(
                flex: 2,
                child: _wheel(
                  controller: _monthCtrl,
                  count: 12,
                  labelBuilder: (i) => _months[i],
                  onChanged: (i) {
                    _month = i + 1;
                    setState(() {}); // rebuild days
                    _emit();
                  },
                  color: color,
                ),
              ),
              // Year
              Expanded(
                child: _wheel(
                  controller: _yearCtrl,
                  count: widget.lastYear - widget.firstYear + 1,
                  labelBuilder: (i) => '${widget.firstYear + i}',
                  onChanged: (i) {
                    _year = widget.firstYear + i;
                    setState(() {}); // rebuild days
                    _emit();
                  },
                  color: color,
                ),
              ),
            ],
          ),
        ],
      ),
    );
  }

  Widget _wheel({
    required FixedExtentScrollController controller,
    required int count,
    required String Function(int) labelBuilder,
    required ValueChanged<int> onChanged,
    required Color color,
  }) {
    return ListWheelScrollView.useDelegate(
      controller: controller,
      itemExtent: 36,
      physics: const FixedExtentScrollPhysics(),
      diameterRatio: 1.6,
      onSelectedItemChanged: onChanged,
      childDelegate: ListWheelChildBuilderDelegate(
        childCount: count,
        builder: (context, index) {
          return Center(
            child: Text(
              labelBuilder(index),
              style: TextStyle(
                fontSize: 14,
                fontWeight: FontWeight.w500,
                fontFamily: 'RobotoMono',
                color: OscColors.textPrimary,
              ),
            ),
          );
        },
      ),
    );
  }
}

// ─── Previews ──────────────────────────────────────────────────────────

@OscPreview(name: 'Date Picker', group: 'OscDatePicker')
Widget oscDatePickerPreview() {
  return SizedBox(
    width: 300,
    child: Padding(
      padding: const EdgeInsets.all(16),
      child: OscDatePicker(value: DateTime(2026, 6, 1)),
    ),
  );
}

@OscPreview(name: 'Sky Blue Accent', group: 'OscDatePicker')
Widget oscDatePickerSkyPreview() {
  return SizedBox(
    width: 300,
    child: Padding(
      padding: const EdgeInsets.all(16),
      child: OscDatePicker(
        value: DateTime(2026, 12, 25),
        accent: OscColors.skyBlue,
      ),
    ),
  );
}
