import 'dart:io';

import 'package:args/command_runner.dart';
import 'package:path/path.dart' as p;

import '../context.dart';
import '../emulator.dart';
import '../util/app_config.dart';
import '../util/errors.dart';
import '../util/host.dart';
import '../util/oscortex_repo.dart';

/// `osx run` (alias `osx preview`) — boot OSCortex in QEMU for previewing.
///
/// Picks hardware acceleration (HVF on macOS, KVM on Linux) when the requested
/// arch matches the host arch, and falls back to TCG emulation for the other
/// arch. Boots a local image if present, else downloads the release ISO.
class RunCommand extends Command<int> {
  RunCommand(this._ctx) {
    argParser
      ..addOption('arch',
          allowed: ['arm64', 'aarch64', 'x64', 'x86_64'],
          help: 'Architecture to boot.\n(default: the host arch, run fast)')
      ..addOption('image',
          help: 'Path to a specific .iso or .kernel image to boot.')
      ..addOption('release',
          help: 'Release tag to fetch the bootable ISO from.\n'
              '(default: ${Emulator.defaultReleaseTag})')
      ..addFlag('headless',
          negatable: false,
          help: 'Boot without a display window (serial on stdio).')
      ..addOption('serial-log',
          help: 'Write the guest serial output to this file.')
      ..addOption('memory', help: 'Guest RAM in MiB.', defaultsTo: '4096');
  }

  final CliContext _ctx;

  @override
  String get name => 'run';

  @override
  List<String> get aliases => const ['preview'];

  @override
  String get description =>
      'Boot OSCortex in an emulator (QEMU) to preview across architectures.\n'
      'Host-native arch runs accelerated (HVF/KVM); the other is emulated.\n'
      'Also available as `osx preview`.';

  @override
  String get invocation => 'osx run [options]';

  @override
  Future<int> run() async {
    final log = _ctx.log;
    final host = _ctx.host;

    // Choose target arch: explicit flag, else host arch (fast path).
    final reqArch = argResults!['arch'] as String?;
    final arch = reqArch != null
        ? Arch.parse(reqArch)!
        : (host.arch ??
            (throw CliException(
              'Could not detect host architecture; pass --arch.',
            )));

    final memMiB = int.tryParse(argResults!['memory'] as String) ?? 4096;
    final emulator = Emulator(host, log, _ctx.proc);

    // Resolve the OSCortex tree (optional here) to find a cache dir + local
    // kernel hint. Falls back to a temp cache if no tree is configured.
    String cacheDir;
    String? localKernel;
    final cfg = AppConfig.load(Directory.current.absolute.path);
    try {
      final repo = OscortexRepo.resolve(
        override: cfg?.oscortexRoot,
        appConfig: cfg?.asMap,
      );
      cacheDir = p.join(repo.root, '.image-cache');
    } on CliException {
      cacheDir = p.join(Directory.systemTemp.path, 'oscortex-images');
    }
    localKernel = _envKernel();

    final image = await emulator.resolveImage(
      arch: arch,
      explicit: argResults!['image'] as String?,
      localKernel: localKernel,
      releaseTag:
          (argResults!['release'] as String?) ?? Emulator.defaultReleaseTag,
      cacheDir: cacheDir,
    );

    log.info('Image: ${image.path} (${image.kind})');

    await emulator.boot(
      image,
      headless: argResults!['headless'] as bool,
      serialLog: argResults!['serial-log'] as String?,
      memMiB: memMiB,
    );
    return 0;
  }

  String? _envKernel() {
    final k = Platform.environment['OSCORTEX_KERNEL'];
    return (k != null && File(k).existsSync()) ? k : null;
  }
}
