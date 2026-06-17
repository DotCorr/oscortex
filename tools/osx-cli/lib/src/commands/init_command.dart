import 'dart:io';

import 'package:args/command_runner.dart';
import 'package:path/path.dart' as p;

import '../context.dart';
import '../engine.dart';
import '../util/app_config.dart';
import '../util/errors.dart';
import '../util/host.dart';
import '../util/logger.dart';
import '../util/oscortex_repo.dart';
import '../util/proc.dart';

/// `osx init` — prepare a Flutter app directory to target OSCortex.
class InitCommand extends Command<int> {
  InitCommand(this._ctx) {
    argParser
      ..addOption('name',
          help: 'Display name shown in the OSCortex launcher.\n'
              '(defaults to the pubspec name)')
      ..addOption('oscortex-root',
          help: 'Path to the OSCortex source tree to record for this app.\n'
              '(defaults to OSCORTEX_ROOT or the tree this CLI lives in)')
      ..addFlag('skip-fetch',
          negatable: false,
          help: 'Record config + verify prerequisites without downloading '
              'the engine.');
  }

  final CliContext _ctx;

  @override
  String get name => 'init';

  @override
  String get description =>
      'Set up the OSCortex build target for the current Flutter app:\n'
      'fetch the prebuilt engine + gen_snapshot (both arches), verify the\n'
      'toolchain, and record per-app config in .osx/config.json.';

  @override
  String get invocation => 'osx init [options]';

  @override
  Future<int> run() async {
    final log = _ctx.log;
    final appDir = Directory.current.absolute.path;

    // 1. Confirm we're in a Flutter app.
    final pubspec = File(p.join(appDir, 'pubspec.yaml'));
    if (!pubspec.existsSync()) {
      throw CliException(
        'No pubspec.yaml in $appDir.',
        hint: 'Run `osx init` from the root of a Flutter app.',
      );
    }
    final pubspecText = pubspec.readAsStringSync();
    final pkgName = _yamlScalar(pubspecText, 'name') ?? p.basename(appDir);
    final pkgVersion = _yamlScalar(pubspecText, 'version') ?? '1.0.0';

    // 2. Locate the OSCortex tree (records it for build/run).
    final repo = OscortexRepo.resolve(
      override: argResults!['oscortex-root'] as String?,
    );
    log.info('OSCortex tree: ${repo.root}');

    // 3. Verify host prerequisites.
    await _verifyPrereqs(repo);

    // 4. Fetch engine + gen_snapshot for BOTH arches (unless skipped).
    final engine = EngineManager(repo, log, _ctx.proc);
    final pinnedVersion = engine.config.artifactVersion;
    if (argResults!['skip-fetch'] as bool) {
      log.warn('Skipping engine fetch (--skip-fetch).');
    } else {
      for (final arch in Arch.values) {
        await engine.fetch(arch);
      }
    }

    // 5. Write per-app config.
    final cfg = AppConfig(
      appDir: appDir,
      displayName: (argResults!['name'] as String?) ?? _humanize(pkgName),
      oscortexRoot: repo.root,
      engineVersion: pinnedVersion,
      bundleVersion: _normalizeVersion(pkgVersion),
    );
    cfg.save();
    log.ok('Wrote ${p.relative(cfg.file.path, from: appDir)}');
    log.detail('display name : ${cfg.displayName}');
    log.detail('engine       : ${cfg.engineVersion}');
    log.detail('bundle ver   : ${cfg.bundleVersion}');

    // 6. .gitignore the build dir hint.
    _ensureGitignore(appDir, log);

    log.ok('Ready. Build with `osx build`, preview with `osx run`.');
    return 0;
  }

  Future<void> _verifyPrereqs(OscortexRepo repo) async {
    final log = _ctx.log;
    final host = _ctx.host;
    log.info('Verifying prerequisites ...');

    // Flutter (host-native; produces the asset bundle + AOT dill).
    if (!await Proc.exists('flutter')) {
      final want = EngineManager(repo, log, _ctx.proc).config.flutterVersion;
      throw CliException(
        'Flutter SDK not found on PATH.',
        hint: 'Install Flutter${want != null ? ' $want' : ''} and ensure '
            '`flutter` is on PATH.',
      );
    }
    log.detail('flutter  ✓');

    // Docker (runs the Linux gen_snapshot for each arch during build).
    if (!await Proc.exists('docker')) {
      log.warn('Docker not found — `osx build` needs it to run the per-arch '
          'gen_snapshot. `osx run` does not.');
    } else {
      log.detail('docker   ✓');
    }

    // QEMU (preview).
    final qemuArm = await Proc.exists('qemu-system-aarch64');
    final qemuX86 = await Proc.exists('qemu-system-x86_64');
    if (qemuArm || qemuX86) {
      log.detail('qemu     ✓ '
          '(${[if (qemuArm) 'aarch64', if (qemuX86) 'x86_64'].join(', ')})');
    } else {
      log.warn('QEMU not found — `osx run`/`osx preview` will be unavailable '
          'until qemu-system-aarch64 / qemu-system-x86_64 are installed.');
    }

    log.detail('host     : ${host.os}/${host.arch?.label ?? 'unknown-arch'}'
        '${host.accelerator != null ? '  (accel: ${host.accelerator})' : '  (no hw accel)'}');
  }

  // --- small helpers ---

  String? _yamlScalar(String yaml, String key) {
    final m = RegExp('^$key:\\s*(.+)\$', multiLine: true).firstMatch(yaml);
    return m?.group(1)?.trim().replaceAll(RegExp('["\']'), '');
  }

  String _humanize(String pkg) => pkg
      .split(RegExp('[_-]'))
      .where((s) => s.isNotEmpty)
      .map((s) => s[0].toUpperCase() + s.substring(1))
      .join(' ');

  // The .osx header version field is 15 bytes; strip a Flutter build suffix.
  String _normalizeVersion(String v) => v.split('+').first;

  void _ensureGitignore(String appDir, Logger log) {
    final gi = File(p.join(appDir, '.gitignore'));
    const marker = '# OSCortex build artifacts';
    if (gi.existsSync() && gi.readAsStringSync().contains(marker)) return;
    gi.writeAsStringSync('\n$marker\nbuild/osx/\n', mode: FileMode.append);
    log.trace('appended $marker to .gitignore');
  }
}
