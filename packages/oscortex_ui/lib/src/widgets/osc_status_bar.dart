import 'package:flutter/material.dart';

import '../preview/osc_preview.dart';
import '../tokens/colors.dart';

/// A bottom status bar matching the spec's footer strip.
///
/// ```dart
/// OscStatusBar(
///   leading: Icon(Icons.folder_rounded, size: 12, color: OscColors.textDim),
///   label: '12 items',
///   trailing: Text('VFS', style: TextStyle(fontSize: 10, color: OscColors.textDim)),
/// );
/// ```
class OscStatusBar extends StatelessWidget {
  const OscStatusBar({
    super.key,
    this.leading,
    this.label,
    this.trailing,
  });

  final Widget? leading;
  final String? label;
  final Widget? trailing;

  @override
  Widget build(BuildContext context) {
    return Container(
      height: 28,
      padding: const EdgeInsets.symmetric(horizontal: 12),
      decoration: BoxDecoration(
        border: Border(
          top: BorderSide(color: Colors.white.withValues(alpha: 0.05)),
        ),
        color: Colors.black.withValues(alpha: 0.20),
      ),
      child: Row(
        children: [
          if (leading != null) ...[
            leading!,
            const SizedBox(width: 6),
          ],
          if (label != null)
            Text(
              label!,
              style: const TextStyle(
                fontSize: 10,
                color: OscColors.textDim,
              ),
            ),
          const Spacer(),
          if (trailing != null) trailing!,
        ],
      ),
    );
  }
}

// ─── Previews ──────────────────────────────────────────────────────────

@OscPreview(name: 'Status Bar', group: 'OscStatusBar')
Widget oscStatusBarPreview() {
  return SizedBox(
    width: 500,
    child: OscStatusBar(
      leading: const Icon(Icons.folder_rounded, size: 12, color: OscColors.textDim),
      label: '12 items',
      trailing: const Text('VFS', style: TextStyle(fontSize: 10, color: OscColors.textDim)),
    ),
  );
}

@OscPreview(name: 'Status Bar Minimal', group: 'OscStatusBar')
Widget oscStatusBarMinimalPreview() {
  return const SizedBox(
    width: 500,
    child: OscStatusBar(label: 'Ready'),
  );
}
