import 'dart:io';
import 'dart:typed_data';

import 'package:path/path.dart' as p;

import 'util/errors.dart';
import 'util/host.dart';
import 'util/logger.dart';
import 'util/proc.dart';

/// A bootable OSCortex image plus how it boots.
class BootImage {
  BootImage({required this.path, required this.arch, required this.kind});

  final String path;
  final Arch arch;

  /// `iso` (UEFI cdrom) or `kernel` (direct `-kernel`, aarch64 only).
  final String kind;

  bool get isKernel => kind == 'kernel';
}

/// Resolves and boots OSCortex images in QEMU, choosing HVF/KVM vs TCG by
/// matching the requested arch against the host arch.
///
/// QEMU invocations mirror the project's proven configs:
///   * aarch64 `-kernel` (interactive): `~/OSCortex-run/run-arm.sh`
///       -M virt[,accel=hvf] -cpu host/cortex-a72 -smp 1 -m 4G -kernel <k>
///       -device ramfb -device virtio-tablet-device -device virtio-keyboard-device
///   * aarch64 ISO: edk2 AAVMF UEFI (-M virt + pflash) — see build-iso-aarch64.sh.
///   * x86_64 ISO:  q35 + OVMF/edk2, -cdrom — see scripts/run-qemu.sh.
class Emulator {
  Emulator(this.host, this.log, this.proc);

  final Host host;
  final Logger log;
  final Proc proc;

  /// Default release tag the bootable ISOs are fetched from when no local image
  /// is found. Overridable via `--release`.
  static const defaultReleaseTag = 'v0.1.2';
  static const _githubRepo = 'DotCorr/oscortex';

  /// QEMU share dir (firmware lives here) derived from the qemu binary path.
  String? _qemuShareDir(String qemuBin) {
    try {
      final resolved = File(qemuBin).resolveSymbolicLinksSync();
      // <prefix>/bin/qemu-system-*  ->  <prefix>/share/qemu
      final prefix = p.dirname(p.dirname(resolved));
      final share = p.join(prefix, 'share', 'qemu');
      return Directory(share).existsSync() ? share : null;
    } catch (_) {
      return null;
    }
  }

  String _qemuBinary(Arch arch) =>
      arch == Arch.aarch64 ? 'qemu-system-aarch64' : 'qemu-system-x86_64';

  Future<void> ensureQemu(Arch arch) async {
    final bin = _qemuBinary(arch);
    if (!await Proc.exists(bin)) {
      throw CliException(
        'QEMU for ${arch.label} not found ($bin).',
        hint: host.isMacOS
            ? 'Install with: brew install qemu'
            : 'Install QEMU (e.g. apt install qemu-system).',
      );
    }
  }

  /// Find a local image for [arch], or download the release ISO.
  /// [explicit] (from --image) wins. [releaseTag] sets the fetch tag.
  Future<BootImage> resolveImage({
    required Arch arch,
    String? explicit,
    String? localKernel,
    required String releaseTag,
    required String cacheDir,
  }) async {
    if (explicit != null) {
      if (!File(explicit).existsSync()) {
        throw CliException('Image not found: $explicit');
      }
      final kind = explicit.endsWith('.iso') ? 'iso' : 'kernel';
      return BootImage(path: explicit, arch: arch, kind: kind);
    }

    // aarch64 boots most reliably via a local .kernel under HVF.
    if (arch == Arch.aarch64) {
      final candidates = [
        if (localKernel != null) localKernel,
        p.join(_home, 'OSCortex-run', 'oscortex-aarch64-release.kernel'),
        p.join(_home, 'OSCortex-run', 'oscortex-aarch64.kernel'),
      ];
      for (final c in candidates) {
        if (File(c).existsSync()) {
          log.trace('using local kernel: $c');
          return BootImage(path: c, arch: arch, kind: 'kernel');
        }
      }
    }

    // Otherwise fetch the release ISO.
    final iso = await _fetchReleaseIso(arch, releaseTag, cacheDir);
    return BootImage(path: iso, arch: arch, kind: 'iso');
  }

  Future<String> _fetchReleaseIso(
      Arch arch, String tag, String cacheDir) async {
    final asset = 'oscortex-${arch.label}-$tag.iso';
    final dest = File(p.join(cacheDir, tag, asset));
    if (dest.existsSync() && dest.lengthSync() > 0) {
      log.trace('using cached ISO: ${dest.path}');
      return dest.path;
    }
    dest.parent.createSync(recursive: true);
    final url = 'https://github.com/$_githubRepo/releases/download/$tag/$asset';
    log.info('Downloading bootable image: $asset ($tag)');
    await proc.run(
      'curl',
      ['-fSL', url, '-o', dest.path],
      what: 'curl $asset',
    );
    if (!dest.existsSync() || dest.lengthSync() == 0) {
      throw CliException('Download produced an empty file: ${dest.path}');
    }
    log.ok('Image ready: ${dest.path}');
    return dest.path;
  }

  /// Assemble + exec the QEMU command for [image]. Replaces the current process
  /// so Ctrl-C / window-close behave like running QEMU directly.
  Future<void> boot(
    BootImage image, {
    required bool headless,
    String? serialLog,
    int memMiB = 4096,
  }) async {
    final arch = image.arch;
    await ensureQemu(arch);
    final qemu = _qemuBinary(arch);

    final accelerate = host.canAccelerate(arch);
    final accel = accelerate ? host.accelerator : null;
    if (accelerate) {
      log.info('Booting ${arch.label} with hardware acceleration ($accel).');
    } else {
      log.warn('Booting ${arch.label} under TCG emulation (no $arch accel on '
          '${host.os}/${host.arch?.label}). This is correct but slow.');
    }

    final args = arch == Arch.aarch64
        ? _aarch64Args(image, accel: accel, headless: headless, memMiB: memMiB)
        : _x86Args(image, accel: accel, headless: headless, memMiB: memMiB);

    // Serial routing.
    if (serialLog != null) {
      args.addAll(['-serial', 'file:$serialLog']);
      log.detail('serial -> $serialLog');
    } else if (headless) {
      args.addAll(['-serial', 'mon:stdio']);
    } else {
      args.addAll([
        '-serial',
        'file:${p.join(Directory.systemTemp.path, 'oscortex-serial.log')}'
      ]);
    }

    log.detail('$qemu ${args.join(' ')}');
    log.info(headless
        ? 'Headless. Ctrl-A then X to quit.'
        : 'Click the window to capture input; release with Ctrl-Alt-G. '
            'Close the window to quit.');

    // Hand off: inherit stdio so QEMU's monitor/serial works interactively.
    final proc =
        await Process.start(qemu, args, mode: ProcessStartMode.inheritStdio);
    final code = await proc.exitCode;
    if (code != 0) {
      throw CliException('QEMU exited with code $code.', exitCode: code);
    }
  }

  List<String> _aarch64Args(
    BootImage image, {
    String? accel,
    required bool headless,
    required int memMiB,
  }) {
    // HVF requires `-cpu host`; TCG uses a concrete core model.
    final machine = accel != null ? 'virt,accel=$accel,gic-version=2' : 'virt';
    final cpu = accel != null ? 'host' : 'cortex-a72';

    final args = <String>[
      '-M', machine,
      '-cpu', cpu,
      '-smp', '1', // OSCortex is a cooperative single-core scheduler.
      '-m', '${memMiB}M',
      // virtio input: -M virt has no PS/2; this is how the shell gets mouse/kbd.
      // USB/xHCI hits an HVF assert under QEMU 11, so we use virtio (proven).
      '-device', 'ramfb',
      '-device', 'virtio-tablet-device',
      '-device', 'virtio-keyboard-device',
    ];

    if (image.isKernel) {
      args.addAll(['-kernel', image.path]);
    } else {
      // UEFI ISO boot via edk2 AAVMF (read-only code pflash + writable varstore).
      final pflash = _aavmfPflash();
      args.addAll([
        '-drive',
        'if=pflash,format=raw,readonly=on,file=${pflash.code}',
        '-drive',
        'if=pflash,format=raw,file=${pflash.vars}',
        '-cdrom',
        image.path,
      ]);
    }

    args.addAll(headless ? ['-display', 'none'] : ['-display', _guiDisplay()]);
    args.add('-no-reboot');
    return args;
  }

  List<String> _x86Args(
    BootImage image, {
    String? accel,
    required bool headless,
    required int memMiB,
  }) {
    final args = <String>[
      '-machine',
      accel != null ? 'q35,accel=$accel' : 'q35',
      if (accel == null) ...['-cpu', 'qemu64,+x2apic'],
      if (accel != null) ...['-cpu', 'host'],
      '-smp',
      '2',
      '-m',
      '${memMiB}M',
      '-device',
      'qemu-xhci,id=xhci',
    ];

    // x86_64 OSCortex images are UEFI; boot via OVMF/edk2 if available, else
    // fall back to a plain -cdrom (QEMU's built-in firmware).
    final ovmf = _ovmfCode();
    if (ovmf != null) {
      args.addAll(['-drive', 'if=pflash,format=raw,readonly=on,file=$ovmf']);
    }
    args.addAll(['-cdrom', image.path]);

    args.addAll(headless ? ['-display', 'none'] : ['-display', _guiDisplay()]);
    args.add('-no-reboot');
    return args;
  }

  String _guiDisplay() => host.isMacOS ? 'cocoa' : 'gtk';

  ({String code, String vars}) _aavmfPflash() {
    final dir = _resolveShareFor('qemu-system-aarch64');
    if (dir == null) {
      throw CliException(
          'Could not locate QEMU firmware (edk2-aarch64-code.fd).',
          hint: 'Install QEMU with edk2 firmware, or boot a .kernel image.');
    }
    final code = p.join(dir, 'edk2-aarch64-code.fd');
    if (!File(code).existsSync()) {
      throw CliException('Missing aarch64 UEFI firmware: $code',
          hint: 'Reinstall QEMU, or use an aarch64 .kernel image (faster).');
    }
    // edk2 needs a writable, 64MiB varstore; create a private copy.
    final vars = p.join(Directory.systemTemp.path, 'oscortex-aavmf-vars.fd');
    if (!File(vars).existsSync()) {
      final blank = Uint8List(64 * 1024 * 1024);
      File(vars).writeAsBytesSync(blank);
    }
    return (code: code, vars: vars);
  }

  String? _ovmfCode() {
    final dir = _resolveShareFor('qemu-system-x86_64');
    if (dir == null) return null;
    for (final name in ['edk2-x86_64-code.fd', 'OVMF_CODE.fd', 'OVMF.fd']) {
      final f = p.join(dir, name);
      if (File(f).existsSync()) return f;
    }
    return null;
  }

  String? _resolveShareFor(String qemuBin) {
    try {
      final probe = Platform.isWindows ? 'where' : 'which';
      final r = Process.runSync(probe, [qemuBin]);
      if (r.exitCode != 0) return null;
      final bin = (r.stdout as String).trim().split('\n').first;
      return _qemuShareDir(bin);
    } catch (_) {
      return null;
    }
  }

  String get _home =>
      Platform.environment['HOME'] ??
      Platform.environment['USERPROFILE'] ??
      '.';
}
