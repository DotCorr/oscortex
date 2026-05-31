class AppTile {
  const AppTile({required this.id, required this.name});

  final int id;
  final String name;

  factory AppTile.fromJson(Map<String, dynamic> json) {
    return AppTile(
      id: json['id'] as int? ?? 0,
      name: json['name'] as String? ?? 'app',
    );
  }
}
