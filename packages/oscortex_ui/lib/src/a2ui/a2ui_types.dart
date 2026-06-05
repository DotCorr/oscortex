/// A2UI bound value — either a literal or a data-binding path.
///
/// v0.8: `{ "literalString": "Hello" }` or `{ "path": "/data/field" }`
/// v0.9: `"Hello"` (string literal) or `{ "path": "/data/field" }`
class A2UIBoundValue {
  const A2UIBoundValue.literal(this.literal) : path = null;
  const A2UIBoundValue.binding(this.path) : literal = null;

  final String? literal;
  final String? path;

  bool get isLiteral => literal != null;
  bool get isBinding => path != null;

  /// Resolve value from the data store. Falls back to literal.
  String resolve(Map<String, dynamic> data) {
    if (isLiteral) return literal!;
    if (path == null) return '';
    return _resolveJsonPath(data, path!) ?? '';
  }

  /// Resolve to a typed value from data store.
  T? resolveTyped<T>(Map<String, dynamic> data) {
    if (isLiteral) {
      if (T == bool && literal != null) return (literal == 'true') as T;
      if (T == double && literal != null) return double.tryParse(literal!) as T?;
      if (T == int && literal != null) return int.tryParse(literal!) as T?;
      return literal as T?;
    }
    if (path == null) return null;
    return _resolveJsonPath(data, path!) as T?;
  }

  static dynamic _resolveJsonPath(Map<String, dynamic> data, String jsonPath) {
    final segments = jsonPath.split('/').where((s) => s.isNotEmpty).toList();
    dynamic current = data;
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

  factory A2UIBoundValue.fromJson(dynamic json) {
    if (json is String) {
      return A2UIBoundValue.literal(json);
    }
    if (json is Map<String, dynamic>) {
      if (json.containsKey('literalString')) {
        return A2UIBoundValue.literal(json['literalString'] as String);
      }
      if (json.containsKey('path')) {
        return A2UIBoundValue.binding(json['path'] as String);
      }
    }
    return const A2UIBoundValue.literal('');
  }

  Map<String, dynamic> toJson() {
    if (isLiteral) return {'literalString': literal};
    return {'path': path};
  }
}

/// A2UI action triggered by interactive components.
class A2UIAction {
  const A2UIAction({required this.name, this.params});

  final String name;
  final Map<String, dynamic>? params;

  factory A2UIAction.fromJson(dynamic json) {
    if (json is Map<String, dynamic>) {
      // v0.9: { "event": { "name": "..." } }
      if (json.containsKey('event')) {
        final event = json['event'] as Map<String, dynamic>;
        return A2UIAction(
          name: event['name'] as String? ?? '',
          params: event['params'] as Map<String, dynamic>?,
        );
      }
      // v0.8: { "name": "..." }
      return A2UIAction(
        name: json['name'] as String? ?? '',
        params: json['params'] as Map<String, dynamic>?,
      );
    }
    return const A2UIAction(name: '');
  }

  Map<String, dynamic> toJson() => {
        'event': {'name': name, if (params != null) 'params': params},
      };
}

/// Accessibility attributes shared by all components.
class A2UIAccessibility {
  const A2UIAccessibility({this.label, this.role});

  final String? label;
  final String? role;

  factory A2UIAccessibility.fromJson(Map<String, dynamic>? json) {
    if (json == null) return const A2UIAccessibility();
    return A2UIAccessibility(
      label: json['label'] as String?,
      role: json['role'] as String?,
    );
  }
}

/// Children — either explicit list of IDs or a template with data binding.
class A2UIChildren {
  const A2UIChildren.explicit(this.ids)
      : templateBinding = null,
        templateComponentId = null;

  const A2UIChildren.template(this.templateBinding, this.templateComponentId)
      : ids = null;

  final List<String>? ids;
  final String? templateBinding;
  final String? templateComponentId;

  bool get isExplicit => ids != null;
  bool get isTemplate => templateBinding != null;

  factory A2UIChildren.fromJson(dynamic json) {
    // v0.9: ["a", "b"]
    if (json is List) {
      return A2UIChildren.explicit(json.cast<String>());
    }
    // v0.8/v0.9 map
    if (json is Map<String, dynamic>) {
      if (json.containsKey('explicitList')) {
        return A2UIChildren.explicit(
            (json['explicitList'] as List).cast<String>());
      }
      if (json.containsKey('template')) {
        final tmpl = json['template'] as Map<String, dynamic>;
        return A2UIChildren.template(
          tmpl['dataBinding'] as String?,
          tmpl['componentId'] as String?,
        );
      }
    }
    return const A2UIChildren.explicit([]);
  }
}

/// Choice option for MultipleChoice / ChoicePicker.
class A2UIChoiceOption {
  const A2UIChoiceOption({required this.label, required this.value});

  final A2UIBoundValue label;
  final String value;

  factory A2UIChoiceOption.fromJson(Map<String, dynamic> json) {
    return A2UIChoiceOption(
      label: A2UIBoundValue.fromJson(json['label']),
      value: json['value'] as String? ?? '',
    );
  }
}

/// Tab item for Tabs component.
class A2UITabItem {
  const A2UITabItem({required this.title, required this.child});

  final A2UIBoundValue title;
  final String child;

  factory A2UITabItem.fromJson(Map<String, dynamic> json) {
    return A2UITabItem(
      title: A2UIBoundValue.fromJson(json['title']),
      child: json['child'] as String? ?? '',
    );
  }
}

// ─── Component Type Enum ───────────────────────────────────────────────

enum A2UIComponentType {
  row,
  column,
  list,
  text,
  image,
  icon,
  divider,
  button,
  textField,
  checkBox,
  slider,
  dateTimeInput,
  multipleChoice,
  card,
  modal,
  tabs,
}

// ─── Unified Component Node ────────────────────────────────────────────

class A2UIComponent {
  const A2UIComponent({
    required this.id,
    required this.type,
    this.accessibility,
    this.weight,
    this.props = const {},
  });

  final String id;
  final A2UIComponentType type;
  final A2UIAccessibility? accessibility;
  final double? weight;

  /// Type-specific properties parsed into typed helpers.
  final Map<String, dynamic> props;

  // ── Typed property accessors ─────────────────────────────────────

  // Layout
  A2UIChildren? get children => props['children'] as A2UIChildren?;
  String? get distribution => props['distribution'] as String?;
  String? get alignment => props['alignment'] as String?;
  String? get direction => props['direction'] as String?;

  // Text
  A2UIBoundValue? get text => props['text'] as A2UIBoundValue?;
  String? get usageHint => props['usageHint'] as String?;

  // Image
  A2UIBoundValue? get url => props['url'] as A2UIBoundValue?;
  String? get fit => props['fit'] as String?;

  // Icon
  A2UIBoundValue? get iconName => props['name'] as A2UIBoundValue?;

  // Divider
  String? get axis => props['axis'] as String?;

  // Button
  String? get childId => props['child'] as String?;
  bool get primary => props['primary'] as bool? ?? false;
  String? get variant => props['variant'] as String?;
  A2UIAction? get action => props['action'] as A2UIAction?;

  // TextField
  A2UIBoundValue? get label => props['label'] as A2UIBoundValue?;
  A2UIBoundValue? get value => props['value'] as A2UIBoundValue?;
  String? get textFieldType => props['textFieldType'] as String?;
  String? get validationRegexp => props['validationRegexp'] as String?;

  // CheckBox — uses label and value

  // Slider
  double? get minValue => (props['minValue'] as num?)?.toDouble();
  double? get maxValue => (props['maxValue'] as num?)?.toDouble();

  // DateTimeInput
  bool get enableDate => props['enableDate'] as bool? ?? true;
  bool get enableTime => props['enableTime'] as bool? ?? false;

  // MultipleChoice / ChoicePicker
  List<A2UIChoiceOption>? get options =>
      props['options'] as List<A2UIChoiceOption>?;
  A2UIBoundValue? get selections => props['selections'] as A2UIBoundValue?;
  int? get maxAllowedSelections => props['maxAllowedSelections'] as int?;

  // Modal
  String? get entryPointChild => props['entryPointChild'] as String?;
  String? get contentChild => props['contentChild'] as String?;

  // Tabs
  List<A2UITabItem>? get tabItems => props['tabItems'] as List<A2UITabItem>?;
}

// ─── Surface ───────────────────────────────────────────────────────────

/// A complete A2UI surface: a flat map of component IDs → definitions,
/// plus a root component ID and optional initial data.
class A2UISurface {
  const A2UISurface({
    required this.rootId,
    required this.components,
    this.initialData = const {},
  });

  final String rootId;
  final Map<String, A2UIComponent> components;
  final Map<String, dynamic> initialData;

  A2UIComponent? operator [](String id) => components[id];
}
