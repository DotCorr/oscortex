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

    final isRemote = widget.app.isRemote;
    final isResolving = widget.app.isResolving;
    final iconColor = isRemote
        ? widget.accentColor.withValues(alpha: 0.50)
        : widget.accentColor.withValues(alpha: 0.85);

    return MouseRegion(
      onEnter: (_) => setState(() => _hovered = true),
      onExit: (_) => setState(() => _hovered = false),
      child: GestureDetector(
        onTap: isResolving ? null : widget.onLaunch,
        child: AnimatedContainer(
          duration: const Duration(milliseconds: 200),
          curve: Curves.easeOut,
          decoration: BoxDecoration(
            borderRadius: OscRadii.cardBorder,
            border: Border.all(
              color: isResolving
                  ? widget.accentColor.withValues(alpha: 0.40)
                  : _hovered
                      ? OscColors.borderHover
                      : OscColors.border,
            ),
            color: _hovered ? OscColors.surfaceHover : OscColors.surface,
            boxShadow: [
              _hovered ? OscShadows.hover : OscShadows.card,
            ],
          ),
          child: Padding(
            padding: const EdgeInsets.all(16),
            child: Stack(
              children: [
                // Main content
                Column(
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
                      child: isResolving
                          ? Center(
                              child: SizedBox.square(
                                dimension: 18,
                                child: CircularProgressIndicator(
                                  strokeWidth: 2,
                                  color: widget.accentColor,
                                ),
                              ),
                            )
                          : Icon(icon, size: 20, color: iconColor),
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
                      isResolving
                          ? 'Resolving…'
                          : isRemote
                              ? '${widget.app.version} · ${widget.app.sizeLabel}'
                              : widget.app.version,
                      style: TextStyle(
                        fontSize: 10,
                        fontWeight: FontWeight.w400,
                        color: isResolving
                            ? widget.accentColor.withValues(alpha: 0.60)
                            : Colors.white.withValues(alpha: 0.35),
                      ),
                    ),
                  ],
                ),
                // Cloud badge for remote apps
                if (isRemote)
                  Positioned(
                    top: 0,
                    right: 0,
                    child: Container(
                      width: 18,
                      height: 18,
                      decoration: BoxDecoration(
                        shape: BoxShape.circle,
                        color: OscColors.surface,
                        border: Border.all(
                          color: widget.accentColor.withValues(alpha: 0.30),
                        ),
                      ),
                      child: Icon(
                        Icons.cloud_download_outlined,
                        size: 10,
                        color: widget.accentColor.withValues(alpha: 0.70),
                      ),
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

// ─── Previews ──────────────────────────────────────────────────────────

@OscPreview(name: 'App Card — Default', group: 'AppCard')
Widget appCardPreview() {
  return SizedBox(
    width: 180,
    height: 180,
    child: AppCard(
      app: AppTile(id: 1, name: 'My App', version: 'v0.1.0', system: false),
      onLaunch: () {},
      accentColor: OscColors.violet,
    ),
  );
}

@OscPreview(name: 'App Card — Sky Blue', group: 'AppCard')
Widget appCardSkyPreview() {
  return SizedBox(
    width: 180,
    height: 180,
    child: AppCard(
      app: AppTile(id: 2, name: 'File Manager', version: 'v1.2.0', system: false),
      onLaunch: () {},
      accentColor: OscColors.skyBlue,
    ),
  );
}
