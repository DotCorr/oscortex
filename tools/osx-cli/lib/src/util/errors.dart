/// Thrown by commands for an expected, user-facing failure (missing toolchain,
/// bad config, a wrapped script exiting non-zero, …). [main] catches this and
/// prints [message] without a Dart stack trace, then exits with [exitCode].
///
/// Use this for "the user did something we can explain" failures. Let real bugs
/// throw normally so their stack traces surface.
class CliException implements Exception {
  CliException(this.message, {this.exitCode = 1, this.hint});

  final String message;
  final int exitCode;

  /// Optional actionable next step shown after the error.
  final String? hint;

  @override
  String toString() => message;
}
