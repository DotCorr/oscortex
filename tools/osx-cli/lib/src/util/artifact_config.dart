import 'dart:io';

/// Reads the shell-style key/value pairs out of `engine-port/artifact.config`
/// (the single source of truth for the engine pin + release host) so the CLI
/// never hard-codes the engine tag or repo. Values may reference an env-var
/// default like `${OSCORTEX_ENGINE_BASE_URL:-https://...}`; we resolve those.
class ArtifactConfig {
  ArtifactConfig(this._values);

  final Map<String, String> _values;

  String? get artifactVersion => _values['ARTIFACT_VERSION'];
  String? get githubRepo => _values['GITHUB_REPO'];
  String? get artifactBaseUrl => _values['ARTIFACT_BASE_URL'];
  String? get flutterVersion => _values['FLUTTER_VERSION'];

  static ArtifactConfig parse(File f) {
    final values = <String, String>{};
    if (!f.existsSync()) return ArtifactConfig(values);

    final assign = RegExp(r'^\s*([A-Z0-9_]+)=(.*)$');
    for (final raw in f.readAsLinesSync()) {
      final line = raw.trim();
      if (line.isEmpty || line.startsWith('#')) continue;
      final m = assign.firstMatch(line);
      if (m == null) continue;
      values[m.group(1)!] = _resolve(_stripQuotes(m.group(2)!.trim()), values);
    }
    return ArtifactConfig(values);
  }

  static String _stripQuotes(String v) {
    // Drop a trailing inline comment that isn't inside quotes (best-effort).
    if ((v.startsWith('"') && v.endsWith('"')) ||
        (v.startsWith("'") && v.endsWith("'"))) {
      return v.substring(1, v.length - 1);
    }
    return v;
  }

  /// Expand `${VAR}` / `${VAR:-default}` against the environment, then prior
  /// values in the same file. Mirrors how bash `source`s the config.
  static String _resolve(String v, Map<String, String> prior) {
    final ref = RegExp(r'\$\{([A-Z0-9_]+)(?::-(.*?))?\}');
    return v.replaceAllMapped(ref, (m) {
      final name = m.group(1)!;
      final fallback = m.group(2) ?? '';
      return Platform.environment[name] ?? prior[name] ?? fallback;
    });
  }
}
