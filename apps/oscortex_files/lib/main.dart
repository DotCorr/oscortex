import 'dart:convert';

import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:oscortex_ui/oscortex_ui.dart';

// ─────────────────────────────────────────────────────────────
// App entry
// ─────────────────────────────────────────────────────────────
void main() {
  runApp(const FilesApp());
}

class FilesApp extends StatelessWidget {
  const FilesApp({super.key});

  @override
  Widget build(BuildContext context) {
    return MaterialApp(
      debugShowCheckedModeBanner: false,
      theme: OscTheme.dark(accent: OscColors.skyBlue),
      home: const FilesHome(),
    );
  }
}

// ─────────────────────────────────────────────────────────────
// Files Home
// ─────────────────────────────────────────────────────────────
class FilesHome extends StatefulWidget {
  const FilesHome({super.key});

  @override
  State<FilesHome> createState() => _FilesHomeState();
}

class _FilesHomeState extends State<FilesHome>
    with SingleTickerProviderStateMixin {
  static const _channel = BasicMessageChannel<String>(
    'oscortex/shell',
    StringCodec(),
  );

  static const Color _accent = OscColors.skyBlue;

  final _pathController = TextEditingController(text: '/Applications');
  final _scrollController = ScrollController();

  List<FsEntry> _entries = const [];
  bool _loading = false;
  String _status = 'Ready';
  InstallStage _installStage = InstallStage.idle;
  String? _installTarget;
  int? _lastInstalledId;

  late final AnimationController _bannerAnim;
  late final Animation<double> _bannerFade;

  @override
  void initState() {
    super.initState();
    _bannerAnim = AnimationController(
      vsync: this,
      duration: const Duration(milliseconds: 350),
    );
    _bannerFade = CurvedAnimation(
      parent: _bannerAnim,
      curve: Curves.easeOut,
    );
    _refresh();
  }

  @override
  void dispose() {
    _pathController.dispose();
    _scrollController.dispose();
    _bannerAnim.dispose();
    super.dispose();
  }

  // ── Shell communication ──────────────────────────────────
  Future<Map<String, dynamic>> _sendJson(String command) async {
    try {
      final raw = await _channel.send(command);
      if (raw == null || raw.trim().isEmpty) return const {};
      final decoded = jsonDecode(raw);
      return decoded is Map<String, dynamic> ? decoded : const {};
    } catch (_) {
      return const {};
    }
  }

  // ── List directory ───────────────────────────────────────
  Future<void> _refresh() async {
    setState(() {
      _loading = true;
      _status = 'Listing ${_pathController.text}…';
    });

    try {
      final json = await _sendJson('vfs:list:${_pathController.text}');
      final entries = (json['entries'] as List<dynamic>? ?? [])
          .whereType<Map>()
          .map((item) => FsEntry.fromJson(item.cast<String, dynamic>()))
          .toList();
      if (!mounted) return;
      setState(() {
        _entries = entries;
        _loading = false;
        _status = '${entries.length} item${entries.length == 1 ? '' : 's'}';
      });
    } catch (e) {
      if (!mounted) return;
      setState(() {
        _loading = false;
        _status = 'List failed: $e';
      });
    }
  }

  // ── Install bundle ───────────────────────────────────────
  Future<void> _install(FsEntry entry) async {
    final path = _joinPath(_pathController.text, entry.name);
    setState(() {
      _installStage = InstallStage.installing;
      _installTarget = path;
      _status = 'Installing $path…';
      _lastInstalledId = null;
    });
    _bannerAnim.forward(from: 0);

    try {
      final json = await _sendJson('install:$path');
      final ok = json['ok'] == true;
      final id = json['id'] as int?;
      if (!mounted) return;
      setState(() {
        _installStage = ok ? InstallStage.success : InstallStage.failed;
        _lastInstalledId = id;
        _status = ok
            ? 'Installed $path${id != null ? '  ·  app id $id' : ''}'
            : 'Install skipped or rejected for $path';
      });
      if (mounted) {
        ScaffoldMessenger.of(context)
          ..clearSnackBars()
          ..showSnackBar(
            SnackBar(
              content: Row(
                children: [
                  Icon(
                    ok
                        ? Icons.check_circle_rounded
                        : Icons.error_outline_rounded,
                    size: 16,
                    color: ok ? OscColors.green : OscColors.red,
                  ),
                  const SizedBox(width: OscSpacing.lg),
                  Expanded(child: Text(_status)),
                ],
              ),
              action: ok
                  ? SnackBarAction(
                      label: 'Refresh',
                      textColor: _accent,
                      onPressed: _refresh,
                    )
                  : null,
            ),
          );
      }
      if (ok) _refresh();
    } catch (e) {
      if (!mounted) return;
      setState(() {
        _installStage = InstallStage.failed;
        _status = 'Install error: $e';
      });
    }
  }

  // ── Helpers ──────────────────────────────────────────────
  String _joinPath(String base, String rel) {
    final cleanBase =
        base.endsWith('/') ? base.substring(0, base.length - 1) : base;
    return '$cleanBase/$rel';
  }

  void _jump(String path) {
    _pathController.text = path;
    _installStage = InstallStage.idle;
    _installTarget = null;
    _bannerAnim.reverse();
    _refresh();
  }

  void _openEntry(FsEntry entry) {
    if (entry.installable) return;
    _jump(_joinPath(_pathController.text, entry.name));
  }

  List<String> _breadcrumbs(String path) {
    final parts =
        path.split('/').where((part) => part.isNotEmpty).toList(growable: false);
    final crumbs = <String>['/'];
    var cur = '';
    for (final part in parts) {
      cur = '$cur/$part';
      crumbs.add(cur);
    }
    return crumbs;
  }

  String _crumbLabel(String path) {
    if (path == '/') return '/';
    final seg = path.split('/').where((s) => s.isNotEmpty);
    if (seg.isEmpty) return '/';
    return seg.last;
  }

  // ─────────────────────────────────────────────────────────
  // BUILD
  // ─────────────────────────────────────────────────────────
  @override
  Widget build(BuildContext context) {
    return Scaffold(
      backgroundColor: OscColors.bodyBg,
      body: Container(
        decoration: BoxDecoration(
          gradient: RadialGradient(
            center: Alignment.topRight,
            radius: 1.4,
            colors: [_accent.withValues(alpha: 0.13), const Color(0x0007080B)],
          ),
        ),
        child: SafeArea(
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.stretch,
            children: [
              // ── Header ─────────────────────────────────
              Padding(
                padding: const EdgeInsets.fromLTRB(
                    OscSpacing.xl5, 20, OscSpacing.xl5, 0),
                child: Row(
                  children: [
                    Container(
                      width: 40,
                      height: 40,
                      decoration: BoxDecoration(
                        color: _accent.withValues(alpha: 0.12),
                        borderRadius:
                            BorderRadius.circular(OscRadii.inner - 1),
                        border: Border.all(
                          color: _accent.withValues(alpha: 0.18),
                        ),
                      ),
                      child: const Icon(
                        Icons.folder_rounded,
                        color: _accent,
                        size: 22,
                      ),
                    ),
                    const SizedBox(width: OscSpacing.xl2),
                    Text(
                      'Files',
                      style: OscTypography.heading2.copyWith(
                        color: OscColors.textPrimary,
                        letterSpacing: -0.5,
                      ),
                    ),
                    const Spacer(),
                    _HeaderButton(
                      tooltip: 'Source',
                      child: PopupMenuButton<String>(
                        tooltip: '',
                        onSelected: _jump,
                        offset: const Offset(0, 44),
                        itemBuilder: (context) => const [
                          PopupMenuItem(
                              value: '/Applications',
                              child: _PopupRow(
                                  icon: Icons.apps_rounded,
                                  label: '/Applications')),
                          PopupMenuItem(
                              value: '/tmp',
                              child: _PopupRow(
                                  icon: Icons.folder_open_rounded,
                                  label: '/tmp')),
                          PopupMenuItem(
                              value: '/sys',
                              child: _PopupRow(
                                  icon: Icons.settings_rounded,
                                  label: '/sys')),
                          PopupMenuItem(
                              value: '/sys/app',
                              child: _PopupRow(
                                  icon: Icons.memory_rounded,
                                  label: '/sys/app')),
                        ],
                        child: const Icon(Icons.source_rounded,
                            size: 18, color: OscColors.textMuted),
                      ),
                    ),
                    const SizedBox(width: OscSpacing.sm),
                    _HeaderButton(
                      tooltip: 'Refresh',
                      child: IconButton(
                        onPressed: _loading ? null : _refresh,
                        icon: _loading
                            ? const SizedBox.square(
                                dimension: 16,
                                child: CircularProgressIndicator(
                                  strokeWidth: 2,
                                  color: _accent,
                                ),
                              )
                            : const Icon(Icons.refresh_rounded,
                                size: 18, color: OscColors.textMuted),
                        padding: EdgeInsets.zero,
                        constraints: const BoxConstraints(
                            minWidth: 36, minHeight: 36),
                      ),
                    ),
                  ],
                ),
              ),
              const SizedBox(height: OscSpacing.xl4),

              // ── Breadcrumbs ────────────────────────────
              Padding(
                padding:
                    const EdgeInsets.symmetric(horizontal: OscSpacing.xl5),
                child: SizedBox(
                  height: 32,
                  child: ListView.separated(
                    scrollDirection: Axis.horizontal,
                    itemCount: _breadcrumbs(_pathController.text).length,
                    separatorBuilder: (context, index) => const Padding(
                      padding: EdgeInsets.symmetric(horizontal: 2),
                      child: Center(
                        child: Icon(Icons.chevron_right_rounded,
                            size: 14, color: OscColors.textMuted),
                      ),
                    ),
                    itemBuilder: (context, index) {
                      final crumbs = _breadcrumbs(_pathController.text);
                      final crumb = crumbs[index];
                      final isLast = index == crumbs.length - 1;
                      return ActionChip(
                        label: Text(
                          _crumbLabel(crumb),
                          style: TextStyle(
                            fontSize: 12,
                            fontWeight:
                                isLast ? FontWeight.w700 : FontWeight.w500,
                            color: isLast ? _accent : OscColors.textMuted,
                          ),
                        ),
                        backgroundColor: isLast
                            ? _accent.withValues(alpha: 0.08)
                            : OscColors.surface,
                        side: BorderSide(
                          color: isLast
                              ? _accent.withValues(alpha: 0.2)
                              : OscColors.border,
                        ),
                        onPressed: () => _jump(crumb),
                      );
                    },
                  ),
                ),
              ),
              const SizedBox(height: OscSpacing.xl),

              // ── Path field ─────────────────────────────
              Padding(
                padding:
                    const EdgeInsets.symmetric(horizontal: OscSpacing.xl5),
                child: TextField(
                  controller: _pathController,
                  onSubmitted: (_) => _refresh(),
                  style: OscTypography.body.copyWith(
                    color: OscColors.textPrimary,
                    fontWeight: FontWeight.w500,
                  ),
                  decoration: InputDecoration(
                    hintText: 'Enter filesystem path…',
                    prefixIcon: const Icon(Icons.terminal_rounded,
                        size: 18, color: OscColors.textMuted),
                    suffixIcon: IconButton(
                      onPressed: _refresh,
                      icon: Container(
                        width: 28,
                        height: 28,
                        decoration: BoxDecoration(
                          color: _accent.withValues(alpha: 0.15),
                          borderRadius:
                              BorderRadius.circular(OscRadii.tag + 1),
                        ),
                        child: const Icon(Icons.arrow_forward_rounded,
                            size: 16, color: _accent),
                      ),
                    ),
                  ),
                ),
              ),
              const SizedBox(height: OscSpacing.xl),

              // ── Quick-access chips ─────────────────────
              Padding(
                padding:
                    const EdgeInsets.symmetric(horizontal: OscSpacing.xl5),
                child: Wrap(
                  spacing: OscSpacing.md,
                  runSpacing: OscSpacing.sm,
                  children: [
                    _QuickChip(
                      label: '/Applications',
                      icon: Icons.apps_rounded,
                      onTap: () => _jump('/Applications'),
                    ),
                    _QuickChip(
                      label: '/tmp',
                      icon: Icons.folder_open_rounded,
                      onTap: () => _jump('/tmp'),
                    ),
                    _QuickChip(
                      label: '/sys/app',
                      icon: Icons.memory_rounded,
                      onTap: () => _jump('/sys/app'),
                    ),
                  ],
                ),
              ),
              const SizedBox(height: OscSpacing.xl2),

              // ── Install banner ─────────────────────────
              Padding(
                padding:
                    const EdgeInsets.symmetric(horizontal: OscSpacing.xl5),
                child: _buildInstallBanner(),
              ),

              // ── Divider ───────────────────────────────
              const Padding(
                padding: EdgeInsets.symmetric(horizontal: OscSpacing.xl5),
                child: Divider(color: OscColors.border, height: 1),
              ),
              const SizedBox(height: OscSpacing.xl2),

              // ── File grid ──────────────────────────────
              Expanded(
                child: _entries.isEmpty && !_loading
                    ? Center(
                        child: Column(
                          mainAxisSize: MainAxisSize.min,
                          children: [
                            Icon(Icons.folder_off_rounded,
                                size: 48,
                                color: OscColors.textMuted
                                    .withValues(alpha: 0.4)),
                            const SizedBox(height: OscSpacing.xl),
                            Text(
                              'No files found here',
                              style: TextStyle(
                                color: OscColors.textMuted
                                    .withValues(alpha: 0.6),
                                fontSize: 14,
                                fontWeight: FontWeight.w500,
                              ),
                            ),
                            const SizedBox(height: OscSpacing.xs),
                            Text(
                              _pathController.text,
                              style: TextStyle(
                                color: OscColors.textMuted
                                    .withValues(alpha: 0.35),
                                fontSize: 12,
                              ),
                            ),
                          ],
                        ),
                      )
                    : Padding(
                        padding: const EdgeInsets.symmetric(
                            horizontal: OscSpacing.xl5),
                        child: GridView.builder(
                          controller: _scrollController,
                          gridDelegate:
                              const SliverGridDelegateWithMaxCrossAxisExtent(
                            maxCrossAxisExtent: 260,
                            mainAxisSpacing: OscSpacing.lg,
                            crossAxisSpacing: OscSpacing.lg,
                            childAspectRatio: 1.15,
                          ),
                          itemCount: _entries.length,
                          itemBuilder: (context, index) {
                            final entry = _entries[index];
                            final fullPath = _joinPath(
                                _pathController.text, entry.name);
                            final isInstallingThis =
                                _installStage == InstallStage.installing &&
                                    _installTarget == fullPath;
                            return _FileCard(
                              entry: entry,
                              isInstallingThis: isInstallingThis,
                              isAnyInstalling:
                                  _installStage == InstallStage.installing,
                              onOpen: () => _openEntry(entry),
                              onInstall: () => _install(entry),
                            );
                          },
                        ),
                      ),
              ),

              // ── Status bar ─────────────────────────────
              Container(
                padding: const EdgeInsets.symmetric(
                    horizontal: OscSpacing.xl5, vertical: OscSpacing.lg),
                decoration: const BoxDecoration(
                  border: Border(
                      top: BorderSide(color: OscColors.border)),
                ),
                child: Row(
                  children: [
                    Container(
                      width: 6,
                      height: 6,
                      decoration: BoxDecoration(
                        color: _loading
                            ? _accent
                            : _entries.isEmpty
                                ? OscColors.textMuted
                                : OscColors.green,
                        shape: BoxShape.circle,
                      ),
                    ),
                    const SizedBox(width: OscSpacing.md),
                    Expanded(
                      child: Text(
                        _status,
                        style: OscTypography.caption.copyWith(
                          fontWeight: FontWeight.w500,
                        ),
                        overflow: TextOverflow.ellipsis,
                      ),
                    ),
                    Text(
                      _pathController.text,
                      style: OscTypography.caption.copyWith(
                        color:
                            OscColors.textMuted.withValues(alpha: 0.5),
                      ),
                    ),
                  ],
                ),
              ),
            ],
          ),
        ),
      ),
    );
  }

  // ── Install banner widget ────────────────────────────────
  Widget _buildInstallBanner() {
    if (_installStage == InstallStage.idle) {
      return const SizedBox.shrink();
    }

    Color borderColor;
    Color bgColor;
    IconData icon;
    String label;

    switch (_installStage) {
      case InstallStage.installing:
        borderColor = _accent.withValues(alpha: 0.35);
        bgColor = _accent.withValues(alpha: 0.06);
        icon = Icons.hourglass_bottom_rounded;
        label = 'Installing ${_installTarget ?? ''}…';
      case InstallStage.success:
        borderColor = OscColors.green.withValues(alpha: 0.35);
        bgColor = OscColors.green.withValues(alpha: 0.06);
        icon = Icons.check_circle_outline_rounded;
        label = _lastInstalledId == null
            ? 'Install complete'
            : 'Install complete  ·  app id $_lastInstalledId';
      case InstallStage.failed:
        borderColor = OscColors.red.withValues(alpha: 0.35);
        bgColor = OscColors.red.withValues(alpha: 0.06);
        icon = Icons.error_outline_rounded;
        label = 'Install failed';
      case InstallStage.idle:
        borderColor = Colors.transparent;
        bgColor = Colors.transparent;
        icon = Icons.info_outline;
        label = '';
    }

    return FadeTransition(
      opacity: _bannerFade,
      child: AnimatedContainer(
        duration: const Duration(milliseconds: 300),
        curve: Curves.easeOut,
        margin: const EdgeInsets.only(bottom: OscSpacing.xl2),
        padding: const EdgeInsets.symmetric(
            horizontal: OscSpacing.xl2, vertical: OscSpacing.xl),
        decoration: BoxDecoration(
          color: bgColor,
          border: Border.all(color: borderColor),
          borderRadius: OscRadii.innerBorder,
        ),
        child: Row(
          children: [
            if (_installStage == InstallStage.installing)
              const SizedBox(
                width: 16,
                height: 16,
                child: CircularProgressIndicator(
                  strokeWidth: 2,
                  color: _accent,
                ),
              )
            else
              Icon(icon, size: 16, color: OscColors.textPrimary),
            const SizedBox(width: OscSpacing.lg),
            Expanded(
              child: Text(
                label,
                style: const TextStyle(
                  fontSize: 12,
                  fontWeight: FontWeight.w600,
                  color: OscColors.textPrimary,
                ),
              ),
            ),
            if (_installStage == InstallStage.success)
              TextButton.icon(
                onPressed: () {
                  _channel.send('list');
                },
                icon: const Icon(Icons.refresh_rounded,
                    size: 14, color: _accent),
                label: const Text(
                  'Refresh Shell',
                  style: TextStyle(
                    fontSize: 12,
                    fontWeight: FontWeight.w600,
                    color: _accent,
                  ),
                ),
              ),
            if (_installStage != InstallStage.installing)
              IconButton(
                onPressed: () {
                  _bannerAnim.reverse().then((_) {
                    if (mounted) {
                      setState(() {
                        _installStage = InstallStage.idle;
                        _installTarget = null;
                      });
                    }
                  });
                },
                icon: Icon(Icons.close_rounded,
                    size: 14,
                    color: OscColors.textMuted.withValues(alpha: 0.6)),
                padding: EdgeInsets.zero,
                constraints:
                    const BoxConstraints(minWidth: 24, minHeight: 24),
              ),
          ],
        ),
      ),
    );
  }
}

// ─────────────────────────────────────────────────────────────
// Header button wrapper
// ─────────────────────────────────────────────────────────────
class _HeaderButton extends StatelessWidget {
  const _HeaderButton({required this.tooltip, required this.child});

  final String tooltip;
  final Widget child;

  @override
  Widget build(BuildContext context) {
    return Tooltip(
      message: tooltip,
      child: Container(
        width: 38,
        height: 38,
        decoration: BoxDecoration(
          color: OscColors.surface,
          borderRadius: OscRadii.buttonBorder,
          border: Border.all(color: OscColors.border),
        ),
        child: Center(child: child),
      ),
    );
  }
}

// ─────────────────────────────────────────────────────────────
// Popup menu row
// ─────────────────────────────────────────────────────────────
class _PopupRow extends StatelessWidget {
  const _PopupRow({required this.icon, required this.label});

  final IconData icon;
  final String label;

  @override
  Widget build(BuildContext context) {
    return Row(
      children: [
        Icon(icon, size: 16, color: OscColors.textMuted),
        const SizedBox(width: OscSpacing.lg),
        Text(label,
            style: const TextStyle(fontSize: 13, color: OscColors.textPrimary)),
      ],
    );
  }
}

// ─────────────────────────────────────────────────────────────
// Quick-access chip
// ─────────────────────────────────────────────────────────────
class _QuickChip extends StatelessWidget {
  const _QuickChip({
    required this.label,
    required this.icon,
    required this.onTap,
  });

  final String label;
  final IconData icon;
  final VoidCallback onTap;

  @override
  Widget build(BuildContext context) {
    return ActionChip(
      avatar: Icon(icon, size: 14, color: OscColors.skyBlue),
      label: Text(
        label,
        style: const TextStyle(
          fontSize: 12,
          fontWeight: FontWeight.w500,
          color: OscColors.textPrimary,
        ),
      ),
      onPressed: onTap,
    );
  }
}

// ─────────────────────────────────────────────────────────────
// File card with hover effect
// ─────────────────────────────────────────────────────────────
class _FileCard extends StatefulWidget {
  const _FileCard({
    required this.entry,
    required this.isInstallingThis,
    required this.isAnyInstalling,
    required this.onOpen,
    required this.onInstall,
  });

  final FsEntry entry;
  final bool isInstallingThis;
  final bool isAnyInstalling;
  final VoidCallback onOpen;
  final VoidCallback onInstall;

  @override
  State<_FileCard> createState() => _FileCardState();
}

class _FileCardState extends State<_FileCard> {
  bool _hovered = false;

  @override
  Widget build(BuildContext context) {
    final isBundle = widget.entry.installable;

    return MouseRegion(
      onEnter: (_) => setState(() => _hovered = true),
      onExit: (_) => setState(() => _hovered = false),
      child: GestureDetector(
        onTap: isBundle ? null : widget.onOpen,
        child: AnimatedContainer(
          duration: const Duration(milliseconds: 200),
          curve: Curves.easeOut,
          decoration: BoxDecoration(
            color: _hovered ? OscColors.surfaceHover : OscColors.surface,
            borderRadius: OscRadii.cardBorder,
            border: Border.all(
              color: _hovered
                  ? isBundle
                      ? OscColors.skyBlue.withValues(alpha: 0.25)
                      : OscColors.borderHover
                  : OscColors.border,
            ),
            boxShadow: _hovered
                ? [
                    BoxShadow(
                      color: isBundle
                          ? OscColors.skyBlue.withValues(alpha: 0.06)
                          : Colors.black.withValues(alpha: 0.15),
                      blurRadius: 16,
                      offset: const Offset(0, 4),
                    ),
                  ]
                : [],
          ),
          child: Padding(
            padding: const EdgeInsets.all(OscSpacing.xl3),
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                // Icon
                Container(
                  width: 42,
                  height: 42,
                  decoration: BoxDecoration(
                    color: isBundle
                        ? OscColors.skyBlue.withValues(alpha: 0.10)
                        : const Color(0x0AFFFFFF),
                    borderRadius:
                        BorderRadius.circular(OscRadii.inner - 1),
                    border: Border.all(
                      color: isBundle
                          ? OscColors.skyBlue.withValues(alpha: 0.15)
                          : OscColors.border,
                    ),
                  ),
                  child: Icon(
                    isBundle
                        ? Icons.inventory_2_rounded
                        : Icons.folder_rounded,
                    color: isBundle ? OscColors.skyBlue : OscColors.textMuted,
                    size: 22,
                  ),
                ),
                const SizedBox(height: OscSpacing.xl2),

                // Name
                Text(
                  widget.entry.name,
                  style: const TextStyle(
                    fontSize: 13,
                    fontWeight: FontWeight.w600,
                    color: OscColors.textPrimary,
                  ),
                  maxLines: 1,
                  overflow: TextOverflow.ellipsis,
                ),
                const SizedBox(height: 3),

                // Subtitle
                Text(
                  isBundle ? 'OSCortex app bundle' : 'Directory',
                  style: TextStyle(
                    fontSize: 11,
                    fontWeight: FontWeight.w500,
                    color: isBundle
                        ? OscColors.skyBlue.withValues(alpha: 0.7)
                        : OscColors.textMuted,
                  ),
                ),

                const Spacer(),

                // Action button
                SizedBox(
                  width: double.infinity,
                  height: 34,
                  child: isBundle
                      ? FilledButton(
                          onPressed: widget.isAnyInstalling
                              ? null
                              : widget.onInstall,
                          child: widget.isInstallingThis
                              ? const SizedBox.square(
                                  dimension: 14,
                                  child: CircularProgressIndicator(
                                    strokeWidth: 2,
                                    color: OscColors.bodyBg,
                                  ),
                                )
                              : const Text('Install'),
                        )
                      : OutlinedButton(
                          onPressed: widget.onOpen,
                          child: const Text('Open'),
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

// ─────────────────────────────────────────────────────────────
// Data model
// ─────────────────────────────────────────────────────────────
class FsEntry {
  const FsEntry({required this.name, required this.installable});

  final String name;
  final bool installable;

  factory FsEntry.fromJson(Map<String, dynamic> json) {
    return FsEntry(
      name: json['name'] as String? ?? '<unknown>',
      installable: json['installable'] as bool? ?? false,
    );
  }
}

// ─────────────────────────────────────────────────────────────
// Install stages
// ─────────────────────────────────────────────────────────────
enum InstallStage {
  idle,
  installing,
  success,
  failed,
}

// ─── Previews ──────────────────────────────────────────────────────────

@OscPreview(name: 'Files App Shell', group: 'Apps')
Widget filesAppShellPreview() {
  return MaterialApp(
    debugShowCheckedModeBanner: false,
    theme: OscTheme.dark(accent: OscColors.skyBlue),
    home: Scaffold(
      backgroundColor: OscColors.bodyBg,
      body: Center(
        child: Column(
          mainAxisSize: MainAxisSize.min,
          children: [
            Icon(Icons.folder_rounded, size: 48,
                color: OscColors.skyBlue.withValues(alpha: 0.5)),
            const SizedBox(height: 12),
            Text('Files App — requires kernel VFS channel',
                style: TextStyle(color: OscColors.textMuted, fontSize: 12)),
          ],
        ),
      ),
    ),
  );
}

@OscPreview(name: 'Files App', group: 'Apps')
Widget filesAppPreview() {
  return const FilesApp();
}

@OscPreview(name: 'Header Button', group: 'Files Components')
Widget filesHeaderButtonPreview() {
  return const Padding(
    padding: EdgeInsets.all(16),
    child: _HeaderButton(
      tooltip: 'Refresh',
      child: Icon(Icons.refresh_rounded, size: 16, color: OscColors.skyBlue),
    ),
  );
}

@OscPreview(name: 'Quick Access Chips', group: 'Files Components')
Widget filesQuickChipsPreview() {
  return Padding(
    padding: const EdgeInsets.all(16),
    child: Wrap(
      spacing: 8,
      children: [
        _QuickChip(label: '/Applications', icon: Icons.apps_rounded, onTap: () {}),
        _QuickChip(label: '/tmp', icon: Icons.folder_open_rounded, onTap: () {}),
        _QuickChip(label: '/sys/app', icon: Icons.settings_rounded, onTap: () {}),
      ],
    ),
  );
}

@OscPreview(name: 'File Card — Directory', group: 'Files Components')
Widget filesCardDirectoryPreview() {
  return SizedBox(
    width: 160,
    child: _FileCard(
      entry: const FsEntry(name: 'Documents', installable: false),
      isInstallingThis: false,
      isAnyInstalling: false,
      onOpen: () {},
      onInstall: () {},
    ),
  );
}

@OscPreview(name: 'File Card — Bundle', group: 'Files Components')
Widget filesCardBundlePreview() {
  return SizedBox(
    width: 160,
    child: _FileCard(
      entry: const FsEntry(name: 'myapp.osx', installable: true),
      isInstallingThis: false,
      isAnyInstalling: false,
      onOpen: () {},
      onInstall: () {},
    ),
  );
}

@OscPreview(name: 'File Card — Installing', group: 'Files Components')
Widget filesCardInstallingPreview() {
  return SizedBox(
    width: 160,
    child: _FileCard(
      entry: const FsEntry(name: 'game.osx', installable: true),
      isInstallingThis: true,
      isAnyInstalling: true,
      onOpen: () {},
      onInstall: () {},
    ),
  );
}

