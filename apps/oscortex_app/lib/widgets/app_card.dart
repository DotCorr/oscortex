import 'package:flutter/material.dart';
import 'package:oscortex_ui/oscortex_ui.dart';

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
            borderRadius: OscRadii.cardBorder,
            border: Border.all(
              color: _hovered ? OscColors.borderHover : OscColors.border,
            ),
            color: _hovered ? OscColors.surfaceHover : OscColors.surface,
            boxShadow: [
              _hovered ? OscShadows.hover : OscShadows.card,
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
                    borderRadius: OscRadii.innerBorder,
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
                  style: OscTypography.body.copyWith(
                    fontWeight: FontWeight.w600,
                    color: OscColors.textBright,
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
