import 'package:flutter/foundation.dart';

/// Reactive data store for A2UI data bindings.
///
/// Stores a JSON-like map and notifies listeners when values change.
/// Components bind to paths like `/user/name` and auto-update.
///
/// ```dart
/// final store = A2UIDataStore({'user': {'name': 'Alice'}});
/// store.get('/user/name'); // 'Alice'
/// store.set('/user/name', 'Bob'); // triggers rebuild
/// ```
class A2UIDataStore extends ChangeNotifier {
  A2UIDataStore([Map<String, dynamic>? initial])
      : _data = initial ?? {};

  Map<String, dynamic> _data;

  /// Read the entire data tree.
  Map<String, dynamic> get data => _data;

  /// Replace the entire data tree and notify.
  set data(Map<String, dynamic> value) {
    _data = value;
    notifyListeners();
  }

  /// Merge new data into the store.
  void merge(Map<String, dynamic> patch) {
    _deepMerge(_data, patch);
    notifyListeners();
  }

  /// Get a value at a JSON path like `/user/name`.
  dynamic get(String path) {
    final segments = path.split('/').where((s) => s.isNotEmpty).toList();
    dynamic current = _data;
    for (final seg in segments) {
      if (current is Map<String, dynamic>) {
        current = current[seg];
      } else if (current is List) {
        final idx = int.tryParse(seg);
        if (idx != null && idx < current.length) {
          current = current[idx];
        } else {
          return null;
        }
      } else {
        return null;
      }
    }
    return current;
  }

  /// Set a value at a JSON path and notify listeners.
  void set(String path, dynamic value) {
    final segments = path.split('/').where((s) => s.isNotEmpty).toList();
    if (segments.isEmpty) return;

    dynamic current = _data;
    for (var i = 0; i < segments.length - 1; i++) {
      final seg = segments[i];
      if (current is Map<String, dynamic>) {
        current.putIfAbsent(seg, () => <String, dynamic>{});
        current = current[seg];
      } else {
        return; // Can't traverse further
      }
    }

    if (current is Map<String, dynamic>) {
      current[segments.last] = value;
      notifyListeners();
    }
  }

  /// Toggle a boolean at a path.
  void toggle(String path) {
    final current = get(path);
    if (current is bool) {
      set(path, !current);
    }
  }

  /// Append to a list at a path.
  void appendToList(String path, dynamic item) {
    final current = get(path);
    if (current is List) {
      current.add(item);
      notifyListeners();
    }
  }

  /// Deep merge two maps.
  static void _deepMerge(Map<String, dynamic> target, Map<String, dynamic> source) {
    for (final key in source.keys) {
      if (target[key] is Map<String, dynamic> && source[key] is Map<String, dynamic>) {
        _deepMerge(target[key] as Map<String, dynamic>,
            source[key] as Map<String, dynamic>);
      } else {
        target[key] = source[key];
      }
    }
  }
}
