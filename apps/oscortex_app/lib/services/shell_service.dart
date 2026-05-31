import 'dart:convert';
import 'package:flutter/services.dart';
import '../models/app_tile.dart';

class ShellService {
  static const _channel = BasicMessageChannel<String>('oscortex/shell', StringCodec());

  static void trace(String message) {
    // ignore: avoid_print
    print('[shell-ui] $message');
  }

  static Future<String?> _safeSend(String command) async {
    try {
      return await _channel.send(command);
    } catch (_) {
      return null;
    }
  }

  static bool _isOkReply(String? raw) {
    final json = _parseJsonObject(
      raw,
      fallback: const <String, dynamic>{'ok': false},
    );
    return json['ok'] == true;
  }

  static Map<String, dynamic> _parseJsonObject(
    String? raw, {
    required Map<String, dynamic> fallback,
  }) {
    if (raw == null || raw.isEmpty) return fallback;
    final trimmed = raw.trim();
    if (trimmed.isEmpty) return fallback;

    try {
      final decoded = jsonDecode(trimmed);
      if (decoded is Map<String, dynamic>) return decoded;
      if (decoded is Map) return decoded.cast<String, dynamic>();
    } catch (_) {
      // Some host replies may include wrappers/noise around JSON payload.
    }

    final start = trimmed.indexOf('{');
    final end = trimmed.lastIndexOf('}');
    if (start >= 0 && end > start) {
      final slice = trimmed.substring(start, end + 1);
      try {
        final decoded = jsonDecode(slice);
        if (decoded is Map<String, dynamic>) return decoded;
        if (decoded is Map) return decoded.cast<String, dynamic>();
      } catch (_) {
        return fallback;
      }
    }

    return fallback;
  }

  static Future<List<AppTile>> refreshApps() async {
    trace('refresh_start');
    final raw = await _safeSend('list');
    final decoded = _parseJsonObject(raw, fallback: const <String, dynamic>{'apps': <dynamic>[]});
    return (decoded['apps'] as List<dynamic>? ?? [])
        .map((e) => AppTile.fromJson(e as Map<String, dynamic>))
        .toList();
  }

  static Future<bool> launchApp(int id) async {
    trace('launch_tap id=$id');
    try {
      final raw = await _safeSend('launch:$id');
      final success = _isOkReply(raw);
      if (success) {
        trace('launch_ok id=$id');
      } else {
        trace('launch_failed id=$id');
      }
      return success;
    } catch (e) {
      trace('launch_error id=$id err=$e');
      return false;
    }
  }

  static Future<bool> installSeed() async {
    trace('install_tap');
    try {
      final raw = await _safeSend('install:/system/seed/demo.osx');
      final success = _isOkReply(raw);
      if (success) {
        trace('install_ok');
      } else {
        trace('install_failed');
      }
      return success;
    } catch (e) {
      trace('install_error err=$e');
      return false;
    }
  }
}
