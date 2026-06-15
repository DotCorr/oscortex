import 'dart:io';

import 'package:path/path.dart' as p;

import 'errors.dart';

/// Locates the OSCortex source tree, which holds the build scripts this CLI
/// orchestrates (`tools/build-flutter-osx.sh`, `engine-port/*`,
/// `tools/oscortex-pack.py`, `scripts/run-*.sh`, the bootable images, …).
///
/// Resolution order:
///   1. `--oscortex-root` flag (passed in as [override]).
///   2. `OSCORTEX_ROOT` environment variable.
///   3. `oscortexRoot` recorded in the app's `.osx/config.json` (from `osx init`).
///   4. The repo this CLI itself lives in (when run from source at
///      `tools/osx-cli`) — handy for maintainers/contributors.
///
/// Throws [CliException] with guidance if none resolve to a valid tree.
class OscortexRepo {
  OscortexRepo(this.root);

  /// Absolute path to the OSCortex repo root.
  final String root;

  // Files we orchestrate. Centralised so a moved script fails clearly.
  String get buildFlutterOsx => p.join(root, 'tools', 'build-flutter-osx.sh');
  String get compileAppAot => p.join(root, 'engine-port', 'compile-app-aot.sh');
  String get fetchEngine => p.join(root, 'engine-port', 'fetch-engine.sh');
  String get pack => p.join(root, 'tools', 'oscortex-pack.py');
  String get artifactConfig => p.join(root, 'engine-port', 'artifact.config');
  String get runX86 => p.join(root, 'scripts', 'run-qemu.sh');
  String get runAarch64 => p.join(root, 'scripts', 'run-aarch64.sh');
  String get engineCache => p.join(root, '.engine-cache');

  /// Validate that this looks like a real OSCortex tree.
  bool get looksValid =>
      File(buildFlutterOsx).existsSync() && File(fetchEngine).existsSync();

  static OscortexRepo resolve({
    String? override,
    Map<String, String>? appConfig,
  }) {
    final candidates = <String?>[
      override,
      Platform.environment['OSCORTEX_ROOT'],
      appConfig?['oscortexRoot'],
      _selfRepoRoot(),
    ];

    for (final c in candidates) {
      if (c == null || c.isEmpty) continue;
      final abs = p.normalize(p.absolute(c));
      final repo = OscortexRepo(abs);
      if (repo.looksValid) return repo;
    }

    throw CliException(
      'Could not find the OSCortex source tree.',
      hint: 'Set OSCORTEX_ROOT, pass --oscortex-root <path>, or run `osx init` '
          'from your app directory to record it.',
    );
  }

  /// When the CLI runs from source (`tools/osx-cli`), walk up to the repo root.
  static String? _selfRepoRoot() {
    // Platform.script points at .../tools/osx-cli/bin/osx.dart when run via
    // `dart run`. Walk up looking for the marker scripts.
    var dir = p.dirname(Platform.script.toFilePath());
    for (var i = 0; i < 8 && dir != p.dirname(dir); i++) {
      if (File(p.join(dir, 'tools', 'build-flutter-osx.sh')).existsSync()) {
        return dir;
      }
      dir = p.dirname(dir);
    }
    return null;
  }
}
