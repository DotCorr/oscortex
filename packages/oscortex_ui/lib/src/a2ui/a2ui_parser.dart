import 'a2ui_types.dart';

/// Parses raw A2UI JSON (v0.8 or v0.9) into an [A2UISurface].
///
/// ```dart
/// final surface = A2UIParser.parse(jsonMap);
/// // surface.rootId, surface.components, surface.initialData
/// ```
class A2UIParser {
  const A2UIParser._();

  /// Parse a complete surface JSON.
  ///
  /// Expected shape:
  /// ```json
  /// {
  ///   "root": "root-id",
  ///   "components": [ { "id": "...", "component": { ... } }, ... ],
  ///   "data": { ... }
  /// }
  /// ```
  static A2UISurface parse(Map<String, dynamic> json) {
    final rootId = json['root'] as String? ?? '';
    final data = (json['data'] as Map<String, dynamic>?) ?? {};
    final componentsList = json['components'] as List<dynamic>? ?? [];

    final components = <String, A2UIComponent>{};
    for (final raw in componentsList) {
      if (raw is Map<String, dynamic>) {
        final comp = _parseComponent(raw);
        if (comp != null) {
          components[comp.id] = comp;
        }
      }
    }

    return A2UISurface(
      rootId: rootId,
      components: components,
      initialData: data,
    );
  }

  /// Parse a single component node.
  static A2UIComponent? _parseComponent(Map<String, dynamic> json) {
    final id = json['id'] as String? ?? '';
    if (id.isEmpty) return null;

    final weight = (json['weight'] as num?)?.toDouble();
    final accessibility = A2UIAccessibility.fromJson(
        json['accessibility'] as Map<String, dynamic>?);

    // Detect format
    final componentField = json['component'];

    A2UIComponentType? type;
    Map<String, dynamic> rawProps = {};

    if (componentField is Map<String, dynamic>) {
      // v0.8: { "component": { "Text": { "text": ... } } }
      final entry = componentField.entries.first;
      type = _parseType(entry.key);
      rawProps = entry.value is Map<String, dynamic>
          ? entry.value as Map<String, dynamic>
          : {};
    } else if (componentField is String) {
      // v0.9: { "component": "Text", "text": ... }
      type = _parseType(componentField);
      rawProps = Map<String, dynamic>.from(json)
        ..remove('id')
        ..remove('component')
        ..remove('weight')
        ..remove('accessibility');
    }

    if (type == null) return null;

    final props = _parseProps(type, rawProps);

    return A2UIComponent(
      id: id,
      type: type,
      accessibility: accessibility,
      weight: weight,
      props: props,
    );
  }

  static A2UIComponentType? _parseType(String name) {
    return switch (name.toLowerCase()) {
      'row' => A2UIComponentType.row,
      'column' => A2UIComponentType.column,
      'list' => A2UIComponentType.list,
      'text' => A2UIComponentType.text,
      'image' => A2UIComponentType.image,
      'icon' => A2UIComponentType.icon,
      'divider' => A2UIComponentType.divider,
      'button' => A2UIComponentType.button,
      'textfield' => A2UIComponentType.textField,
      'checkbox' => A2UIComponentType.checkBox,
      'slider' => A2UIComponentType.slider,
      'datetimeinput' => A2UIComponentType.dateTimeInput,
      'multiplechoice' => A2UIComponentType.multipleChoice,
      'choicepicker' => A2UIComponentType.multipleChoice,
      'card' => A2UIComponentType.card,
      'modal' => A2UIComponentType.modal,
      'tabs' => A2UIComponentType.tabs,
      _ => null,
    };
  }

  static Map<String, dynamic> _parseProps(
      A2UIComponentType type, Map<String, dynamic> raw) {
    final props = <String, dynamic>{};

    switch (type) {
      case A2UIComponentType.row:
      case A2UIComponentType.column:
      case A2UIComponentType.list:
        if (raw.containsKey('children')) {
          props['children'] = A2UIChildren.fromJson(raw['children']);
        }
        props['distribution'] = raw['distribution'] ?? raw['justify'];
        props['alignment'] = raw['alignment'] ?? raw['align'];
        if (raw.containsKey('direction')) {
          props['direction'] = raw['direction'];
        }

      case A2UIComponentType.text:
        if (raw.containsKey('text')) {
          props['text'] = A2UIBoundValue.fromJson(raw['text']);
        }
        props['usageHint'] = raw['usageHint'] ?? raw['variant'];

      case A2UIComponentType.image:
        if (raw.containsKey('url')) {
          props['url'] = A2UIBoundValue.fromJson(raw['url']);
        }
        props['fit'] = raw['fit'];
        props['usageHint'] = raw['usageHint'] ?? raw['variant'];

      case A2UIComponentType.icon:
        if (raw.containsKey('name')) {
          props['name'] = A2UIBoundValue.fromJson(raw['name']);
        }

      case A2UIComponentType.divider:
        props['axis'] = raw['axis'] ?? 'horizontal';

      case A2UIComponentType.button:
        props['child'] = raw['child'];
        props['primary'] = raw['primary'] ?? (raw['variant'] == 'primary');
        props['variant'] = raw['variant'];
        if (raw.containsKey('action')) {
          props['action'] = A2UIAction.fromJson(raw['action']);
        }

      case A2UIComponentType.textField:
        if (raw.containsKey('label')) {
          props['label'] = A2UIBoundValue.fromJson(raw['label']);
        }
        // v0.8: 'text', v0.9: 'value'
        final valKey = raw.containsKey('value') ? 'value' : 'text';
        if (raw.containsKey(valKey)) {
          props['value'] = A2UIBoundValue.fromJson(raw[valKey]);
        }
        props['textFieldType'] = raw['textFieldType'];
        props['validationRegexp'] = raw['validationRegexp'];

      case A2UIComponentType.checkBox:
        if (raw.containsKey('label')) {
          props['label'] = A2UIBoundValue.fromJson(raw['label']);
        }
        if (raw.containsKey('value')) {
          props['value'] = A2UIBoundValue.fromJson(raw['value']);
        }

      case A2UIComponentType.slider:
        if (raw.containsKey('value')) {
          props['value'] = A2UIBoundValue.fromJson(raw['value']);
        }
        props['minValue'] = raw['minValue'];
        props['maxValue'] = raw['maxValue'];

      case A2UIComponentType.dateTimeInput:
        if (raw.containsKey('value')) {
          props['value'] = A2UIBoundValue.fromJson(raw['value']);
        }
        props['enableDate'] = raw['enableDate'] ?? true;
        props['enableTime'] = raw['enableTime'] ?? false;

      case A2UIComponentType.multipleChoice:
        if (raw.containsKey('options')) {
          props['options'] = (raw['options'] as List)
              .map((o) => A2UIChoiceOption.fromJson(o as Map<String, dynamic>))
              .toList();
        }
        if (raw.containsKey('selections')) {
          props['selections'] = A2UIBoundValue.fromJson(raw['selections']);
        }
        props['maxAllowedSelections'] = raw['maxAllowedSelections'];

      case A2UIComponentType.card:
        props['child'] = raw['child'];

      case A2UIComponentType.modal:
        props['entryPointChild'] = raw['entryPointChild'];
        props['contentChild'] = raw['contentChild'];

      case A2UIComponentType.tabs:
        if (raw.containsKey('tabItems')) {
          props['tabItems'] = (raw['tabItems'] as List)
              .map((t) => A2UITabItem.fromJson(t as Map<String, dynamic>))
              .toList();
        }
    }

    return props;
  }
}
