import 'util/host.dart';
import 'util/logger.dart';
import 'util/proc.dart';

/// Shared services handed to every command: the logger, detected host facts,
/// and the process runner (which uses the logger for tracing).
class CliContext {
  CliContext({required this.log, required this.host}) : proc = Proc(log);

  final Logger log;
  final Host host;
  final Proc proc;
}
