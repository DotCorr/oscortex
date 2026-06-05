import 'dart:convert';

import 'package:flutter/material.dart';

import '../tokens/colors.dart';
import '../tokens/typography.dart';
import 'a2ui_data_store.dart';
import 'a2ui_parser.dart';
import 'a2ui_renderer.dart';
import 'a2ui_types.dart';

/// Top-level widget that takes an A2UI JSON surface definition and
/// renders it into native OSCortex widgets.
///
/// This is the primary entry point for A2UI integration. Agents send
/// A2UI JSON → this widget parses and renders it.
///
/// ```dart
/// A2UISurfaceWidget(
///   json: '{"root":"root","components":[...]}',
///   onAction: (action) => agent.dispatch(action.name),
/// );
/// ```
class A2UISurfaceWidget extends StatefulWidget {
  const A2UISurfaceWidget({
    super.key,
    this.json,
    this.jsonMap,
    this.store,
    this.onAction,
  }) : assert(json != null || jsonMap != null,
            'Provide either json string or jsonMap');

  /// Raw JSON string of the A2UI surface.
  final String? json;

  /// Pre-parsed JSON map (avoids double-parsing).
  final Map<String, dynamic>? jsonMap;

  /// External data store. If null, creates one from the surface's initial data.
  final A2UIDataStore? store;

  /// Callback when any A2UI action is triggered.
  final A2UIActionCallback? onAction;

  @override
  State<A2UISurfaceWidget> createState() => _A2UISurfaceWidgetState();
}

class _A2UISurfaceWidgetState extends State<A2UISurfaceWidget> {
  late A2UISurface _surface;
  late A2UIDataStore _store;
  String? _error;

  @override
  void initState() {
    super.initState();
    _parse();
  }

  @override
  void didUpdateWidget(A2UISurfaceWidget old) {
    super.didUpdateWidget(old);
    if (old.json != widget.json || old.jsonMap != widget.jsonMap) {
      _parse();
    }
  }

  void _parse() {
    try {
      final map = widget.jsonMap ??
          (jsonDecode(widget.json!) as Map<String, dynamic>);
      _surface = A2UIParser.parse(map);
      _store = widget.store ?? A2UIDataStore(_surface.initialData);
      _error = null;
    } catch (e) {
      _error = e.toString();
    }
    if (mounted) setState(() {});
  }

  @override
  Widget build(BuildContext context) {
    if (_error != null) {
      return Container(
        padding: const EdgeInsets.all(16),
        decoration: BoxDecoration(
          color: OscColors.red.withValues(alpha: 0.08),
          border: Border.all(color: OscColors.red.withValues(alpha: 0.3)),
          borderRadius: BorderRadius.circular(8),
        ),
        child: Column(
          mainAxisSize: MainAxisSize.min,
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            Text('A2UI Parse Error',
                style: OscTypography.monoSmall
                    .copyWith(color: OscColors.red, fontWeight: FontWeight.w600)),
            const SizedBox(height: 4),
            Text(_error!,
                style: OscTypography.monoSmall.copyWith(color: OscColors.textMuted)),
          ],
        ),
      );
    }

    return A2UIRenderer(
      surface: _surface,
      store: _store,
      onAction: widget.onAction,
    );
  }
}
