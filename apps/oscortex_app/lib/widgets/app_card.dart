import 'package:flutter/material.dart';

import '../models/app_tile.dart';

class AppCard extends StatefulWidget {
  const AppCard({
    super.key,
    required this.app,
    required this.onLaunch,
    required this.accentColor,
  });

  final AppTile app;
  final VoidCallback onLaunch;
  final Color accentColor;

  @override
  State<AppCard> createState() => _AppCardState();
}

class _AppCardState extends State<AppCard> {
  bool _hovered = false;

  @override
  Widget build(BuildContext context) {
    final icon = switch (widget.app.coreRole) {
      CoreAppRole.canvas => Icons.dashboard_customize_rounded,
      CoreAppRole.files => Icons.folder_rounded,
      CoreAppRole.web => Icons.link_rounded,
      null => Icons.apps_rounded,
    };

    return MouseRegion(
      onEnter: (_) => setState(() => _hovered = true),
      onExit: (_) => setState(() => _hovered = false),
      child: GestureDetector(
        onTap: widget.onLaunch,
        child: AnimatedContainer(
          duration: const Duration(milliseconds: 200),
          curve: Curves.easeOut,
          decoration: BoxDecoration(
            borderRadius: BorderRadius.circular(14),
            border: Border.all(
              color: _hovered
                  ? Colors.white.withValues(alpha: 0.12)
                  : Colors.white.withValues(alpha: 0.08),
            ),
            color: _hovered
                ? Colors.white.withValues(alpha: 0.035)
                : Colors.white.withValues(alpha: 0.025),
            boxShadow: [
              BoxShadow(
                color: const Color(0x80000000),
                blurRadius: _hovered ? 20 : 12,
                offset: const Offset(0, 6),
                spreadRadius: -8,
              ),
            ],
          ),
          child: Padding(
            padding: const EdgeInsets.all(16),
            child: Column(
              mainAxisAlignment: MainAxisAlignment.center,
              children: [
                Container(
                  width: 44,
                  height: 44,
                  decoration: BoxDecoration(
                    borderRadius: BorderRadius.circular(12),
                    color: widget.accentColor.withValues(alpha: 0.12),
                    border: Border.all(
                      color: widget.accentColor.withValues(alpha: 0.20),
                    ),
                  ),
                  child: Icon(
                    icon,
                    size: 20,
                    color: widget.accentColor.withValues(alpha: 0.85),
                  ),
                ),
                const SizedBox(height: 12),
                Text(
                  widget.app.name,
                  textAlign: TextAlign.center,
                  maxLines: 2,
                  overflow: TextOverflow.ellipsis,
                  style: const TextStyle(
                    fontSize: 13,
                    fontWeight: FontWeight.w600,
                    color: Color(0xFFE5E5E5),
                  ),
                ),
                const SizedBox(height: 4),
                Text(
                  widget.app.version,
                  style: TextStyle(
                    fontSize: 10,
                    fontWeight: FontWeight.w400,
                    color: Colors.white.withValues(alpha: 0.35),
                  ),
                ),
              ],
            ),
          ),
        ),
      ),
    );
  }
}
