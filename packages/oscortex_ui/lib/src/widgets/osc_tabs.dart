import 'dart:ui';

import 'package:flutter/material.dart';

import '../preview/osc_preview.dart';
import '../tokens/colors.dart';
import '../tokens/radii.dart';
import '../tokens/spacing.dart';

/// An OSCortex-native tabbed view using the segment system.
///
/// Uses [OscTabSegment] pills (not Material TabBar) with optional
/// glassmorphic blur on scroll. Each tab body is a widget.
///
/// ```dart
/// OscTabs(
///   tabs: [
///     OscTabEntry(label: 'General', child: generalSettings),
///     OscTabEntry(label: 'Privacy', child: privacySettings),
///   ],
/// );
/// ```
class OscTabEntry {
  const OscTabEntry({required this.label, required this.child});
  final String label;
  final Widget child;
}

class OscTabs extends StatefulWidget {
  const OscTabs({
    super.key,
    required this.tabs,
    this.accent,
    this.initialIndex = 0,
    this.onChanged,
  });

  final List<OscTabEntry> tabs;
  final Color? accent;
  final int initialIndex;
  final ValueChanged<int>? onChanged;

  @override
  State<OscTabs> createState() => _OscTabsState();
}

class _OscTabsState extends State<OscTabs> {
  late int _active;

  @override
  void initState() {
    super.initState();
    _active = widget.initialIndex;
  }

  @override
  Widget build(BuildContext context) {
    final color = widget.accent ?? OscColors.violet;

    return Column(
      mainAxisSize: MainAxisSize.min,
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        // ── Segment bar ────────────────────────────────────────
        ClipRRect(
          borderRadius: OscRadii.buttonBorder,
          child: BackdropFilter(
            filter: ImageFilter.blur(sigmaX: 12, sigmaY: 12),
            child: Container(
              padding: const EdgeInsets.all(3),
              decoration: BoxDecoration(
                color: Colors.white.withValues(alpha: 0.04),
                borderRadius: OscRadii.buttonBorder,
                border: Border.all(color: Colors.white.withValues(alpha: 0.06)),
              ),
              child: Row(
                mainAxisSize: MainAxisSize.min,
                children: [
                  for (var i = 0; i < widget.tabs.length; i++)
                    Expanded(
                      child: GestureDetector(
                        onTap: () {
                          setState(() => _active = i);
                          widget.onChanged?.call(i);
                        },
                        child: AnimatedContainer(
                          duration: const Duration(milliseconds: 200),
                          curve: Curves.easeOut,
                          padding: const EdgeInsets.symmetric(
                              vertical: 8, horizontal: 12),
                          decoration: BoxDecoration(
                            color: _active == i
                                ? color.withValues(alpha: 0.14)
                                : Colors.transparent,
                            borderRadius: BorderRadius.circular(6),
                            border: Border.all(
                              color: _active == i
                                  ? color.withValues(alpha: 0.30)
                                  : Colors.transparent,
                            ),
                          ),
                          child: Center(
                            child: Text(
                              widget.tabs[i].label,
                              style: TextStyle(
                                fontSize: 12,
                                fontWeight: _active == i
                                    ? FontWeight.w600
                                    : FontWeight.w400,
                                color: _active == i
                                    ? color
                                    : OscColors.textMuted,
                              ),
                            ),
                          ),
                        ),
                      ),
                    ),
                ],
              ),
            ),
          ),
        ),

        const SizedBox(height: OscSpacing.lg),

        // ── Tab body ───────────────────────────────────────────
        Expanded(
          child: AnimatedSwitcher(
            duration: const Duration(milliseconds: 220),
            switchInCurve: Curves.easeOut,
            child: KeyedSubtree(
              key: ValueKey(_active),
              child: widget.tabs[_active].child,
            ),
          ),
        ),
      ],
    );
  }
}

// ─── Previews ──────────────────────────────────────────────────────────

@OscPreview(name: 'Settings Tabs', group: 'OscTabs')
Widget oscTabsPreview() {
  return SizedBox(
    width: 400,
    height: 250,
    child: Padding(
      padding: const EdgeInsets.all(16),
      child: OscTabs(
        tabs: [
          OscTabEntry(
            label: 'General',
            child: Center(
              child: Text('General settings content',
                  style: TextStyle(color: OscColors.textMuted, fontSize: 13)),
            ),
          ),
          OscTabEntry(
            label: 'Privacy',
            child: Center(
              child: Text('Privacy settings content',
                  style: TextStyle(color: OscColors.textMuted, fontSize: 13)),
            ),
          ),
          OscTabEntry(
            label: 'Display',
            child: Center(
              child: Text('Display settings content',
                  style: TextStyle(color: OscColors.textMuted, fontSize: 13)),
            ),
          ),
        ],
      ),
    ),
  );
}
