import 'dart:io';

import 'package:args/args.dart';
import 'package:args/command_runner.dart';

import 'src/commands/build_command.dart';
import 'src/commands/init_command.dart';
import 'src/commands/run_command.dart';
import 'src/context.dart';
import 'src/util/errors.dart';
import 'src/util/host.dart';
import 'src/util/logger.dart';

/// Package version, surfaced via `osx --version`. Keep in sync with pubspec.
const osxCliVersion = '0.1.0';

/// Entry point used by `bin/osx.dart`. Returns the process exit code.
Future<int> run(List<String> args) async {
  final log = Logger();
  final ctx = CliContext(log: log, host: Host.detect());

  final runner = OsxCommandRunner(ctx);
  try {
    final code = await runner.run(args);
    return code ?? 0;
  } on UsageException catch (e) {
    stderr.writeln(e.message);
    stderr.writeln();
    stderr.writeln(e.usage);
    return 64; // EX_USAGE
  } on CliException catch (e) {
    log.error(e.message);
    if (e.hint != null) log.detail('→ ${e.hint}');
    return e.exitCode;
  }
}

class OsxCommandRunner extends CommandRunner<int> {
  OsxCommandRunner(this._ctx)
      : super(
            'osx',
            'Build and run Flutter apps for OSCortex.\n\n'
                'Compile a Flutter app into a universal .osx bundle (native\n'
                'aarch64 + x86_64) and preview it across architectures in QEMU.') {
    argParser
      ..addFlag('verbose',
          abbr: 'v', negatable: false, help: 'Print diagnostic (trace) output.')
      ..addFlag('version',
          negatable: false, help: 'Print the osx CLI version and exit.');

    addCommand(InitCommand(_ctx));
    addCommand(BuildCommand(_ctx));
    addCommand(RunCommand(_ctx));
  }

  final CliContext _ctx;

  @override
  Future<int?> runCommand(ArgResults topLevelResults) async {
    if (topLevelResults['version'] as bool) {
      stdout.writeln('osx $osxCliVersion');
      return 0;
    }
    _ctx.log.verbose = topLevelResults['verbose'] as bool;
    return super.runCommand(topLevelResults);
  }
}
