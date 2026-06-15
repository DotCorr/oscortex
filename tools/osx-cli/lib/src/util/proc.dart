import 'dart:async';
import 'dart:io';

import 'errors.dart';
import 'logger.dart';

/// Thin wrapper around [Process] that streams child stdout/stderr straight to
/// the terminal and turns a non-zero exit into a [CliException]. This keeps the
/// UX of the wrapped shell scripts intact (their progress prints live) while
/// giving us a single, consistent failure path.
class Proc {
  Proc(this.log);

  final Logger log;

  /// Run [exe] with [args]. Streams I/O live unless [captureStdout] is set, in
  /// which case stdout is collected and returned instead of printed.
  ///
  /// Throws [CliException] on a non-zero exit (unless [allowFailure]).
  Future<ProcResult> run(
    String exe,
    List<String> args, {
    String? workingDirectory,
    Map<String, String>? environment,
    bool captureStdout = false,
    bool allowFailure = false,
    String? what,
  }) async {
    final label = what ?? '$exe ${args.join(' ')}';
    log.trace('exec: $label'
        '${workingDirectory != null ? '  (cwd=$workingDirectory)' : ''}');

    final Process proc;
    try {
      proc = await Process.start(
        exe,
        args,
        workingDirectory: workingDirectory,
        environment: environment,
        // Inherit the parent env so PATH/FLUTTER_ROOT/etc. flow through.
        includeParentEnvironment: true,
        mode: captureStdout
            ? ProcessStartMode.normal
            : ProcessStartMode.inheritStdio,
      );
    } on ProcessException catch (e) {
      throw CliException(
        'Failed to launch `$exe`: ${e.message}',
        hint: 'Is it installed and on your PATH?',
      );
    }

    final out = StringBuffer();
    if (captureStdout) {
      // Drain both streams; mirror stderr so errors are still visible.
      final outDone = proc.stdout
          .transform(const SystemEncoding().decoder)
          .forEach(out.write);
      final errDone = proc.stderr
          .transform(const SystemEncoding().decoder)
          .forEach(stderr.write);
      await Future.wait([outDone, errDone]);
    }

    final code = await proc.exitCode;
    if (code != 0 && !allowFailure) {
      throw CliException('`$label` failed (exit $code).', exitCode: code);
    }
    return ProcResult(code, out.toString());
  }

  /// True if [exe] resolves on PATH.
  static Future<bool> exists(String exe) async {
    final probe = Platform.isWindows ? 'where' : 'which';
    try {
      final r = await Process.run(probe, [exe]);
      return r.exitCode == 0;
    } catch (_) {
      return false;
    }
  }
}

class ProcResult {
  ProcResult(this.exitCode, this.stdout);
  final int exitCode;
  final String stdout;
}
