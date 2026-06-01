import 'dart:convert';
import 'package:flutter/services.dart';
import '../models/app_tile.dart';

class ShellService {
  static const _channel = BasicMessageChannel<String>(
    'oscortex/shell',
    StringCodec(),
  );

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
    final decoded = _parseJsonObject(
      raw,
      fallback: const <String, dynamic>{'apps': <dynamic>[]},
    );
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

  static Future<bool> installFromPath(String path) async {
    trace('install_tap path=$path');
    try {
      final raw = await _safeSend('install:$path');
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

  // ── On-demand package delivery ─────────────────────────────────────────

  /// Fetch the remote package catalog from the configured server.
  /// Returns a list of available packages (not yet cached locally).
  static Future<List<AppTile>> fetchCatalog() async {
    trace('pkg_catalog_start');
    final raw = await _safeSend('pkg_catalog');
    final decoded = _parseJsonObject(
      raw,
      fallback: const <String, dynamic>{'packages': <dynamic>[]},
    );
    return (decoded['packages'] as List<dynamic>? ?? [])
        .map((e) => AppTile.fromCatalog(e as Map<String, dynamic>))
        .toList();
  }

  /// Resolve (fetch + verify + cache) a package by name and return the app_id.
  /// The kernel handles the full pipeline: HTTP fetch → SHA-256 verify → install.
  static Future<int> resolvePackage(String name) async {
    trace('pkg_resolve name=$name');
    final raw = await _safeSend('pkg_resolve:$name');
    final decoded = _parseJsonObject(
      raw,
      fallback: const <String, dynamic>{'app_id': -1},
    );
    final appId = decoded['app_id'] as int? ?? -1;
    if (appId >= 0) {
      trace('pkg_resolve_ok name=$name app_id=$appId');
    } else {
      trace('pkg_resolve_failed name=$name');
    }
    return appId;
  }

  /// Set the package server address (IP + port).
  static Future<bool> setPackageServer(String ip, int port) async {
    trace('pkg_set_server ip=$ip port=$port');
    final raw = await _safeSend('pkg_set_server:$ip:$port');
    return _isOkReply(raw);
  }
}

