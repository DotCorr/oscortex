import 'dart:io';
import 'dart:typed_data';

import 'package:args/command_runner.dart';
import 'package:path/path.dart' as p;

import '../context.dart';
import '../engine.dart';
import '../osx_bundle.dart';
import '../util/app_config.dart';
import '../util/errors.dart';
import '../util/host.dart';
import '../util/oscortex_repo.dart';

/// `osx build` — compile the current Flutter app into `.osx` bundles that are
/// native for both aarch64 and x86_64.
///
/// Pipeline (mirrors tools/build-flutter-osx.sh + engine-port/compile-app-aot.sh,
/// generalised to emit BOTH arches):
///
///   host    : flutter pub get
///   host    : flutter build bundle           -> build/flutter_assets/ (+ kernel_blob)
///   host    : frontend_server --aot --tfa     -> build/app_aot.dill   (arch-neutral)
///   docker  : gen_snapshot[arm64]  (--platform linux/arm64)  -> libapp-arm64.so
///   docker  : gen_snapshot[x64]    (--platform linux/amd64)  -> libapp-x64.so
///   host    : patch_libapp.py (x64 macos->linux feature tag, x64 only)
///   pack    : per-arch <App>-{arm64,x64}.osx  +  universal <App>.osx
class BuildCommand extends Command<int> {
  BuildCommand(this._ctx) {
    argParser
      ..addMultiOption('arch',
          allowed: ['arm64', 'aarch64', 'x64', 'x86_64'],
          help: 'Limit to specific arch(es). Repeatable.\n'
              '(default: both — required for a universal bundle)')
      ..addOption('name', help: 'Override the display name in the bundle.')
      ..addOption('output-dir',
          help: 'Directory for the .osx outputs.\n(default: <app>/build/osx)')
      ..addFlag('assets',
          defaultsTo: true,
          help: 'Also stage build/flutter_assets next to the bundle.')
      ..addFlag('universal',
          defaultsTo: true,
          help:
              'Emit the universal (fat) bundle in addition to per-arch ones.\n'
              'Requires both arches.');
  }

  final CliContext _ctx;

  @override
  String get name => 'build';

  @override
  String get description =>
      'Compile the current Flutter app into a universal .osx bundle\n'
      '(native aarch64 + x86_64), plus per-arch bundles.';

  @override
  String get invocation => 'osx build [options]';

  @override
  Future<int> run() async {
    final log = _ctx.log;
    final appDir = Directory.current.absolute.path;

    final cfg = AppConfig.load(appDir);
    if (cfg == null) {
      throw CliException(
        'This app is not initialised for OSCortex.',
        hint: 'Run `osx init` first.',
      );
    }
    if (!File(p.join(appDir, 'lib', 'main.dart')).existsSync()) {
      throw CliException('No lib/main.dart in $appDir — not a Flutter app.');
    }

    final repo =
        OscortexRepo.resolve(override: cfg.oscortexRoot, appConfig: cfg.asMap);
    final engine = EngineManager(repo, log, _ctx.proc);

    // Which arches to build.
    final requested = (argResults!['arch'] as List<String>)
        .map(Arch.parse)
        .whereType<Arch>()
        .toSet();
    final arches = requested.isEmpty ? Arch.values.toSet() : requested;

    final displayName = (argResults!['name'] as String?) ??
        cfg.displayName ??
        p.basename(appDir);
    final version = cfg.bundleVersion ?? '1.0.0';

    if (!await _dockerAvailable()) {
      throw CliException(
        'Docker is required to run the per-arch gen_snapshot.',
        hint: 'Install Docker Desktop (or a Linux docker daemon) and retry. '
            'The gen_snapshot binaries are Linux ELF and run in a container.',
      );
    }

    // Ensure engines/gen_snapshots are present for each target arch.
    for (final a in arches) {
      await engine.fetch(a);
    }

    // 1. pub get + asset bundle (host).
    log.info('flutter pub get');
    await _ctx.proc.run('flutter', ['pub', 'get'], workingDirectory: appDir);

    log.info('Building Flutter asset bundle (flutter build bundle)');
    await _ctx.proc.run(
      'flutter',
      ['--suppress-analytics', 'build', 'bundle'],
      workingDirectory: appDir,
    );

    // 2. AOT dill (host, arch-neutral).
    final dill = File(p.join(appDir, 'build', 'app_aot.dill'));
    await _compileAotDill(appDir, dill);

    final outDir = (argResults!['output-dir'] as String?) ??
        p.join(appDir, 'build', 'osx');

    // 3. Per-arch gen_snapshot (docker).
    final snapshots = <Arch, Uint8List>{};
    for (final a in arches) {
      final so = await _genSnapshot(repo, engine, a, dill, appDir, outDir);
      snapshots[a] = so;
    }

    // 4. Pack.
    final outputs = OsxOutputs(outDir, displayName);

    for (final a in arches) {
      OsxBundle.writeSingleArch(
        snapshot: snapshots[a]!,
        name: displayName,
        version: version,
        out: outputs.perArch[a]!,
      );
      log.ok('Bundle (${a.label}): '
          '${p.relative(outputs.perArch[a]!.path)} '
          '(${_human(outputs.perArch[a]!.lengthSync())})');
    }

    final wantUniversal = argResults!['universal'] as bool;
    if (wantUniversal && arches.length > 1) {
      // Host arch is the primary slot so an unmodified kernel boots it.
      final primary =
          _ctx.host.arch != null && snapshots.containsKey(_ctx.host.arch)
              ? _ctx.host.arch!
              : snapshots.keys.first;
      OsxBundle.writeUniversal(
        snapshots: snapshots,
        primary: primary,
        name: displayName,
        version: version,
        out: outputs.universal,
      );
      log.ok('Universal bundle: ${p.relative(outputs.universal.path)} '
          '(${_human(outputs.universal.lengthSync())}, primary=${primary.label})');
    } else if (wantUniversal) {
      log.warn('Universal bundle skipped — needs both arches '
          '(built: ${arches.map((a) => a.label).join(', ')}).');
    }

    // 5. Assets (optional).
    if (argResults!['assets'] as bool) {
      _stageAssets(appDir, outDir);
    }

    log.ok('Done. Preview with `osx run`.');
    return 0;
  }

  // --- pipeline steps ---

  Future<void> _compileAotDill(String appDir, File dill) async {
    _ctx.log.info('Compiling AOT kernel (frontend_server --aot --tfa)');
    final fl = _flutterRoot();
    final dartaot =
        p.join(fl, 'bin', 'cache', 'dart-sdk', 'bin', 'dartaotruntime');
    final frontend = p.join(fl, 'bin', 'cache', 'artifacts', 'engine',
        'darwin-x64', 'frontend_server_aot.dart.snapshot');
    // Product patched SDK — OSCortex runs the product engine; a release/profile
    // dill is rejected at load. (Matches engine-port/compile-app-aot.sh.)
    final sdkProduct = p.join(fl, 'bin', 'cache', 'artifacts', 'engine',
            'common', 'flutter_patched_sdk_product') +
        Platform.pathSeparator;
    final pkgConfig = p.join(appDir, '.dart_tool', 'package_config.json');

    for (final f in [dartaot, frontend]) {
      if (!File(f).existsSync()) {
        throw CliException('Missing Flutter AOT tool: $f',
            hint: 'Run `flutter precache` or reinstall Flutter.');
      }
    }

    dill.parent.createSync(recursive: true);
    await _ctx.proc.run(
      dartaot,
      [
        frontend,
        '--sdk-root',
        sdkProduct,
        '--target=flutter',
        '--aot',
        '--tfa',
        '--packages=$pkgConfig',
        '--output-dill',
        dill.path,
        p.join(appDir, 'lib', 'main.dart'),
      ],
      workingDirectory: appDir,
      what: 'frontend_server --aot',
    );
    if (!dill.existsSync()) {
      throw CliException('AOT kernel compilation produced no dill.');
    }
  }

  /// Run the arch-matched (Linux) gen_snapshot inside docker on [dill],
  /// returning the resulting libapp ELF bytes.
  Future<Uint8List> _genSnapshot(
    OscortexRepo repo,
    EngineManager engine,
    Arch arch,
    File dill,
    String appDir,
    String outDir,
  ) async {
    _ctx.log.info(
        'gen_snapshot (${arch.label}) — app_aot.dill -> libapp-${arch.osxToken}.so');

    final genSnap = engine.genSnapshot(arch);
    if (!genSnap.existsSync()) {
      throw CliException(
        'gen_snapshot missing for ${arch.label}: ${genSnap.path}',
        hint: 'Re-run `osx init` (engine ${engine.config.artifactVersion} '
            'should ship a per-arch gen_snapshot).',
      );
    }

    final outSo = File(p.join(outDir, 'libapp-${arch.osxToken}.so'));
    outSo.parent.createSync(recursive: true);

    final platform = arch == Arch.aarch64 ? 'linux/arm64' : 'linux/amd64';

    // Bind-mount every host tree the container must read/write, with identical
    // in-container paths (so the absolute paths we pass just work):
    //   - the repo root: gen_snapshot lives under .engine-cache there
    //   - the app dir:   the AOT dill is under <app>/build
    //   - the output dir: where we write libapp-<arch>.so
    // De-duplicate nested paths so docker doesn't reject overlapping mounts.
    final mounts = _dockerMounts([repo.root, appDir, outSo.parent.path]);

    await _ctx.proc.run(
      'docker',
      [
        'run',
        '--rm',
        '--platform',
        platform,
        ...mounts,
        '-w',
        repo.root,
        'ubuntu:22.04',
        genSnap.path,
        '--deterministic',
        '--snapshot_kind=app-aot-elf',
        '--elf=${outSo.path}',
        '--strip',
        dill.path,
      ],
      what: 'docker gen_snapshot ($platform)',
    );

    if (!outSo.existsSync() || outSo.lengthSync() < 1024) {
      throw CliException(
          'gen_snapshot produced no usable ELF for ${arch.label}.');
    }

    // x64-only: the macOS-host frontend_server tags the snapshot "x64 macos ...";
    // the Linux engine expects "x64 linux ...". patch_libapp.py rewrites it
    // in-place (same byte length). arm64 is built natively on linux/arm64 and
    // needs no patch. (Mirrors tools/build-flutter-osx.sh.)
    if (arch == Arch.x86_64) {
      final patch =
          File(p.join(repo.root, 'tools', 'flutter-engine', 'patch_libapp.py'));
      if (patch.existsSync()) {
        await _ctx.proc
            .run('python3', [patch.path, outSo.path], what: 'patch_libapp.py');
      }
    }

    return outSo.readAsBytesSync();
  }

  void _stageAssets(String appDir, String outDir) {
    final src = Directory(p.join(appDir, 'build', 'flutter_assets'));
    if (!src.existsSync()) {
      _ctx.log.warn('flutter_assets missing — skipping asset staging.');
      return;
    }
    final dst = Directory(p.join(outDir, 'flutter_assets'));
    if (dst.existsSync()) dst.deleteSync(recursive: true);
    dst.createSync(recursive: true);
    for (final e in src.listSync(recursive: true)) {
      final rel = p.relative(e.path, from: src.path);
      final target = p.join(dst.path, rel);
      if (e is Directory) {
        Directory(target).createSync(recursive: true);
      } else if (e is File) {
        File(target).parent.createSync(recursive: true);
        e.copySync(target);
      }
    }
    _ctx.log.detail('staged flutter_assets -> ${p.relative(dst.path)}');
  }

  // --- helpers ---

  /// Build `-v host:host` mount args for [paths], dropping any path nested
  /// inside another (docker rejects overlapping bind mounts).
  List<String> _dockerMounts(List<String> paths) {
    final normalized = paths.map((x) => p.normalize(p.absolute(x))).toSet();
    final roots = <String>[];
    for (final candidate in normalized) {
      final coveredBy = normalized
          .any((other) => other != candidate && p.isWithin(other, candidate));
      if (!coveredBy) roots.add(candidate);
    }
    return [
      for (final r in roots) ...['-v', '$r:$r']
    ];
  }

  Future<bool> _dockerAvailable() async {
    final r = await _ctx.proc.run(
      'docker',
      ['version', '--format', '{{.Server.Version}}'],
      captureStdout: true,
      allowFailure: true,
      what: 'docker version',
    );
    return r.exitCode == 0;
  }

  String _flutterRoot() {
    final env = Platform.environment['FLUTTER_ROOT'] ??
        Platform.environment['FLUTTER_HOME'];
    if (env != null && Directory(env).existsSync()) return env;
    // Derive from `which flutter` -> <root>/bin/flutter.
    try {
      final r = Process.runSync('which', ['flutter']);
      if (r.exitCode == 0) {
        final binFlutter = (r.stdout as String).trim();
        // .../flutter/bin/flutter  ->  .../flutter
        return p
            .dirname(p.dirname(File(binFlutter).resolveSymbolicLinksSync()));
      }
    } catch (_) {/* fall through */}
    return '/opt/homebrew/share/flutter';
  }

  String _human(int bytes) {
    if (bytes < 1024) return '$bytes B';
    if (bytes < 1024 * 1024) return '${(bytes / 1024).toStringAsFixed(1)} KiB';
    return '${(bytes / 1024 / 1024).toStringAsFixed(1)} MiB';
  }
}
