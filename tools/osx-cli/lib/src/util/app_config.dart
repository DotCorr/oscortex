import 'dart:convert';
import 'dart:io';

import 'package:path/path.dart' as p;

/// Per-app OSCortex config, written by `osx init` into `<app>/.osx/config.json`
/// and read by `osx build` / `osx run`. Keeps app-local choices (display name,
/// pinned engine, the OSCortex tree to use) out of the global environment.
class AppConfig {
  AppConfig({
    required this.appDir,
    this.displayName,
    this.oscortexRoot,
    this.engineVersion,
    this.bundleVersion,
  });

  /// Absolute path of the Flutter app this config belongs to.
  final String appDir;

  /// Display name shown in the OSCortex launcher. Defaults to the pubspec name.
  String? displayName;

  /// OSCortex source tree recorded at init time (so build/run need no flags).
  String? oscortexRoot;

  /// Engine artifact tag pinned for this app (e.g. `oscortex-engine-2`).
  String? engineVersion;

  /// Version string baked into the .osx header. Defaults to the pubspec version.
  String? bundleVersion;

  static String dirFor(String appDir) => p.join(appDir, '.osx');
  static String pathFor(String appDir) => p.join(dirFor(appDir), 'config.json');

  File get file => File(pathFor(appDir));

  /// Load config for [appDir], or null if `osx init` has not been run.
  static AppConfig? load(String appDir) {
    final f = File(pathFor(appDir));
    if (!f.existsSync()) return null;
    final m = jsonDecode(f.readAsStringSync()) as Map<String, dynamic>;
    return AppConfig(
      appDir: appDir,
      displayName: m['displayName'] as String?,
      oscortexRoot: m['oscortexRoot'] as String?,
      engineVersion: m['engineVersion'] as String?,
      bundleVersion: m['bundleVersion'] as String?,
    );
  }

  /// Raw map form, used to seed repo resolution before a full load.
  Map<String, String> get asMap => {
        if (oscortexRoot != null) 'oscortexRoot': oscortexRoot!,
        if (engineVersion != null) 'engineVersion': engineVersion!,
      };

  void save() {
    Directory(dirFor(appDir)).createSync(recursive: true);
    const encoder = JsonEncoder.withIndent('  ');
    file.writeAsStringSync('${encoder.convert({
          'displayName': displayName,
          'oscortexRoot': oscortexRoot,
          'engineVersion': engineVersion,
          'bundleVersion': bundleVersion,
        })}\n');
  }
}
