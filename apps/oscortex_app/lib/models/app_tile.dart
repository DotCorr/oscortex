enum CoreAppRole { canvas, files, web }

/// Where an app comes from in the on-demand delivery model.
enum AppSource {
  /// Installed locally (in the app registry).
  local,

  /// Available from the remote package server (not yet fetched).
  remote,

  /// Currently being fetched from the package server.
  resolving,
}

class AppTile {
  const AppTile({
    required this.id,
    required this.name,
    required this.version,
    required this.system,
    this.coreRole,
    this.source = AppSource.local,
    this.sizeBytes = 0,
  });

  final int id;
  final String name;
  final String version;
  final bool system;
  final CoreAppRole? coreRole;

  /// Package delivery state.
  final AppSource source;

  /// Bundle size in bytes (from catalog manifest).
  final int sizeBytes;

  bool get isCore => coreRole != null;

  /// Whether this app is available from the cloud (not yet cached locally).
  bool get isRemote => source == AppSource.remote;

  /// Whether this app is currently being fetched.
  bool get isResolving => source == AppSource.resolving;

  /// Whether this app is cached locally and ready to launch.
  bool get isCached => source == AppSource.local;

  /// Human-readable size string.
  String get sizeLabel {
    if (sizeBytes < 1024) return '$sizeBytes B';
    if (sizeBytes < 1024 * 1024) return '${(sizeBytes / 1024).toStringAsFixed(1)} KB';
    return '${(sizeBytes / (1024 * 1024)).toStringAsFixed(1)} MB';
  }

  AppTile copyWith({AppSource? source, int? id}) {
    return AppTile(
      id: id ?? this.id,
      name: name,
      version: version,
      system: system,
      coreRole: coreRole,
      source: source ?? this.source,
      sizeBytes: sizeBytes,
    );
  }

  factory AppTile.fromJson(Map<String, dynamic> json) {
    return AppTile(
      id: json['id'] as int? ?? 0,
      name: json['name'] as String? ?? 'app',
      version: json['version'] as String? ?? '1.0.0',
      system: json['system'] as bool? ?? false,
      coreRole: _parseCoreRole(json['coreRole'] as String?),
    );
  }

  /// Create from a remote catalog entry (not yet cached locally).
  factory AppTile.fromCatalog(Map<String, dynamic> json) {
    return AppTile(
      id: -1, // no local ID yet
      name: json['name'] as String? ?? 'app',
      version: json['version'] as String? ?? '1.0.0',
      system: json['system'] as bool? ?? false,
      source: AppSource.remote,
      sizeBytes: json['size'] as int? ?? 0,
    );
  }

  static CoreAppRole? _parseCoreRole(String? value) {
    return switch (value) {
      'canvas' => CoreAppRole.canvas,
      'files' => CoreAppRole.files,
      'web' => CoreAppRole.web,
      _ => null,
    };
  }
}

