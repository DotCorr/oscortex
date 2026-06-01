import 'dart:async';

import 'package:flutter/material.dart';

import '../tokens/colors.dart';
import '../tokens/typography.dart';

/// Intent / command prompt bar with blinking caret.
///
/// ```dart
/// OscIntentBar(prompt: 'What are we building today?')
/// OscIntentBar(
///   prompt: 'Search files...',
///   accentColor: OscColors.skyBlue,
///   onSubmitted: (text) => search(text),
/// )
/// ```
class OscIntentBar extends StatefulWidget {
  const OscIntentBar({
    super.key,
    this.prompt = 'What are we building today?',
    this.accentColor = OscColors.violetLight,
    this.editable = false,
    this.onSubmitted,
    this.controller,
  });

  final String prompt;
  final Color accentColor;

  /// If true, the bar becomes an actual text input.
  final bool editable;
  final ValueChanged<String>? onSubmitted;
  final TextEditingController? controller;

  @override
  State<OscIntentBar> createState() => _OscIntentBarState();
}

class _OscIntentBarState extends State<OscIntentBar> {
  bool _caretOn = true;
  Timer? _caretTimer;

  @override
  void initState() {
    super.initState();
    if (!widget.editable) {
      _caretTimer = Timer.periodic(
        const Duration(milliseconds: 500),
        (_) {
          if (!mounted) return;
          setState(() => _caretOn = !_caretOn);
        },
      );
    }
  }

  @override
  void dispose() {
    _caretTimer?.cancel();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    return Container(
      padding: const EdgeInsets.symmetric(horizontal: 16, vertical: 10),
      decoration: BoxDecoration(
        border: Border(
          bottom: BorderSide(
            color: Colors.white.withValues(alpha: 0.06),
          ),
        ),
      ),
      child: Container(
        padding: const EdgeInsets.symmetric(horizontal: 12, vertical: 9),
        decoration: BoxDecoration(
          color: Colors.white.withValues(alpha: 0.035),
          borderRadius: BorderRadius.circular(12),
          border: Border.all(
            color: Colors.white.withValues(alpha: 0.06),
          ),
        ),
        child: Row(
          children: [
            Text(
              '>',
              style: OscTypography.monoPrompt.copyWith(
                color: widget.accentColor,
              ),
            ),
            const SizedBox(width: 8),
            if (widget.editable)
              Expanded(
                child: TextField(
                  controller: widget.controller,
                  onSubmitted: widget.onSubmitted,
                  style: OscTypography.monoBody.copyWith(
                    color: Colors.white.withValues(alpha: 0.80),
                  ),
                  decoration: InputDecoration(
                    hintText: widget.prompt,
                    hintStyle: OscTypography.monoBody.copyWith(
                      color: Colors.white.withValues(alpha: 0.40),
                    ),
                    border: InputBorder.none,
                    isDense: true,
                    contentPadding: EdgeInsets.zero,
                  ),
                ),
              )
            else ...[
              Text(
                widget.prompt,
                style: OscTypography.monoBody.copyWith(
                  color: Colors.white.withValues(alpha: 0.70),
                ),
              ),
              const SizedBox(width: 2),
              AnimatedOpacity(
                duration: const Duration(milliseconds: 80),
                opacity: _caretOn ? 1 : 0,
                child: Container(
                  width: 7,
                  height: 16,
                  decoration: BoxDecoration(
                    color: widget.accentColor,
                    borderRadius: BorderRadius.circular(1),
                  ),
                ),
              ),
            ],
          ],
        ),
      ),
    );
  }
}
