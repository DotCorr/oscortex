import 'dart:io';

import 'package:oscortex_cli/oscortex_cli.dart' as cli;

Future<void> main(List<String> args) async {
  exitCode = await cli.run(args);
}
