import 'dart:io';

import 'package:path/path.dart' as p;

import 'util/artifact_config.dart';
import 'util/errors.dart';
import 'util/host.dart';
import 'util/logger.dart';
import 'util/oscortex_repo.dart';
import 'util/proc.dart';

/// Manages the prebuilt OSCortex engine + gen_snapshot via the repo's
/// `engine-port/fetch-engine.sh`. We do not re-implement the download/checksum
/// logic — we shell out to the canonical script so the pin in
/// `engine-port/artifact.config` stays the single source of truth.
class EngineManager {
  EngineManager(this.repo, this.log, this.proc);

  final OscortexRepo repo;
  final Logger log;
  final Proc proc;

  ArtifactConfig get config => ArtifactConfig.parse(File(repo.artifactConfig));

  /// Directory where fetch-engine.sh caches a fetched ([arch],release) bundle:
  /// `<repo>/.engine-cache/<ARTIFACT_VERSION>/oscortex-<token>-release`.
  Directory cacheDir(Arch arch) {
    final version = config.artifactVersion ?? 'oscortex-engine-2';
    return Directory(p.join(
      repo.engineCache,
      version,
      'oscortex-${arch.engineToken}-release',
    ));
  }

  File genSnapshot(Arch arch) =>
      File(p.join(cacheDir(arch).path, 'gen_snapshot'));

  File engineSo(Arch arch) =>
      File(p.join(cacheDir(arch).path, 'libflutter_engine.so'));

  bool isFetched(Arch arch) => engineSo(arch).existsSync();

  /// Fetch (or reuse cached) engine + gen_snapshot for [arch]. Idempotent.
  Future<void> fetch(Arch arch) async {
    if (isFetched(arch)) {
      log.trace(
          'engine ${arch.engineToken} already cached at ${cacheDir(arch).path}');
      return;
    }
    log.info('Fetching prebuilt engine (${arch.label}) '
        'via engine-port/fetch-engine.sh ...');
    await proc.run(
      'bash',
      [repo.fetchEngine, arch.engineToken, 'release'],
      workingDirectory: repo.root,
      what: 'fetch-engine.sh ${arch.engineToken} release',
    );
    if (!isFetched(arch)) {
      throw CliException(
        'Engine fetch reported success but ${engineSo(arch).path} is missing.',
        hint: 'Check engine-port/artifact.config (ARTIFACT_VERSION / '
            'ARTIFACT_BASE_URL) and your network.',
      );
    }
    log.ok('Engine ready: ${arch.label} (${config.artifactVersion})');
  }
}
