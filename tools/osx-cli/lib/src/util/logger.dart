import 'dart:io';

/// Minimal ANSI-aware logger. Colours are disabled automatically when stdout is
/// not a terminal (e.g. piped into a file or CI) or when `NO_COLOR` is set.
class Logger {
  Logger({bool? verbose}) : verbose = verbose ?? false;

  /// When true, [trace] messages are printed.
  bool verbose;

  static final bool _color =
      stdout.hasTerminal && Platform.environment['NO_COLOR'] == null;

  static String _wrap(String code, String s) => _color ? '$code$s\x1b[0m' : s;

  /// Informational step, e.g. "fetching engine".
  void info(String msg) => stdout.writeln('${_wrap('\x1b[36m', '•')} $msg');

  /// Success / completion.
  void ok(String msg) => stdout.writeln('${_wrap('\x1b[32m', '✓')} $msg');

  /// Non-fatal warning.
  void warn(String msg) =>
      stderr.writeln('${_wrap('\x1b[33m', '!')} ${_wrap('\x1b[33m', msg)}');

  /// Fatal error (does not exit — caller decides).
  void error(String msg) =>
      stderr.writeln('${_wrap('\x1b[31m', '✗')} ${_wrap('\x1b[31m', msg)}');

  /// Verbose-only diagnostic.
  void trace(String msg) {
    if (verbose) stderr.writeln(_wrap('\x1b[90m', '  $msg'));
  }

  /// A dim sub-line under an [info]/[ok] line.
  void detail(String msg) => stdout.writeln(_wrap('\x1b[90m', '  $msg'));
}
