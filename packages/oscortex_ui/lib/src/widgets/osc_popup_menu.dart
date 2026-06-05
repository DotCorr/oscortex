import 'package:flutter/material.dart';

import '../preview/osc_preview.dart';
import '../tokens/colors.dart';
import '../tokens/radii.dart';

/// An OSCortex popup menu with spec-compliant dark styling.
///
/// Wraps [PopupMenuButton] with OSCortex surface/border tokens.
///
/// ```dart
/// OscPopupMenu<String>(
///   items: [
///     OscPopupItem(value: '/Applications', icon: Icons.apps_rounded, label: '/Applications'),
///     OscPopupItem(value: '/tmp', icon: Icons.folder_open_rounded, label: '/tmp'),
///   ],
///   onSelected: (path) => navigate(path),
///   child: Icon(Icons.source_rounded, size: 18),
/// );
/// ```
class OscPopupItem<T> {
  const OscPopupItem({
    required this.value,
    required this.label,
    this.icon,
  });

  final T value;
  final String label;
  final IconData? icon;
}

class OscPopupMenu<T> extends StatelessWidget {
  const OscPopupMenu({
    super.key,
    required this.items,
    this.onSelected,
    this.child,
    this.tooltip = '',
    this.offset = const Offset(0, 44),
  });

  final List<OscPopupItem<T>> items;
  final ValueChanged<T>? onSelected;
  final Widget? child;
  final String tooltip;
  final Offset offset;

  @override
  Widget build(BuildContext context) {
    return PopupMenuButton<T>(
      tooltip: tooltip,
      onSelected: onSelected,
      offset: offset,
      color: OscColors.canvasBg,
      surfaceTintColor: Colors.transparent,
      shape: RoundedRectangleBorder(
        borderRadius: OscRadii.cardBorder,
        side: BorderSide(color: OscColors.border),
      ),
      itemBuilder: (context) => [
        for (final item in items)
          PopupMenuItem<T>(
            value: item.value,
            child: Row(
              children: [
                if (item.icon != null) ...[
                  Icon(item.icon, size: 16, color: OscColors.textMuted),
                  const SizedBox(width: 10),
                ],
                Text(
                  item.label,
                  style: const TextStyle(
                    fontSize: 13,
                    color: OscColors.textPrimary,
                  ),
                ),
              ],
            ),
          ),
      ],
      child: child,
    );
  }
}

// ─── Previews ──────────────────────────────────────────────────────────

@OscPreview(name: 'Popup Menu Trigger', group: 'OscPopupMenu')
Widget oscPopupMenuPreview() {
  return Padding(
    padding: const EdgeInsets.all(16),
    child: OscPopupMenu<String>(
      items: const [
        OscPopupItem(value: '/Applications', icon: Icons.apps_rounded, label: '/Applications'),
        OscPopupItem(value: '/tmp', icon: Icons.folder_open_rounded, label: '/tmp'),
        OscPopupItem(value: '/sys', icon: Icons.settings_rounded, label: '/sys'),
      ],
      child: Container(
        padding: const EdgeInsets.symmetric(horizontal: 12, vertical: 8),
        decoration: BoxDecoration(
          color: OscColors.surface,
          borderRadius: OscRadii.buttonBorder,
          border: Border.all(color: OscColors.border),
        ),
        child: const Row(
          mainAxisSize: MainAxisSize.min,
          children: [
            Icon(Icons.source_rounded, size: 16, color: OscColors.textMuted),
            SizedBox(width: 6),
            Text('Source', style: TextStyle(color: OscColors.textPrimary, fontSize: 12)),
          ],
        ),
      ),
    ),
  );
}
