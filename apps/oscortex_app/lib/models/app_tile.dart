enum CoreAppRole { canvas, files, web }

class AppTile {
  const AppTile({
    required this.id,
    required this.name,
    required this.version,
    required this.system,
    this.coreRole,
  });

  final int id;
  final String name;
  final String version;
  final bool system;
  final CoreAppRole? coreRole;

  bool get isCore => coreRole != null;

  factory AppTile.fromJson(Map<String, dynamic> json) {
    return AppTile(
      id: json['id'] as int? ?? 0,
      name: json['name'] as String? ?? 'app',
      version: json['version'] as String? ?? '1.0.0',
      system: json['system'] as bool? ?? false,
      coreRole: _parseCoreRole(json['coreRole'] as String?),
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
