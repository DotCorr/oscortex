/// A2UI protocol support for OSCortex.
///
/// Maps Google's Agent-to-UI (A2UI) protocol components to **native
/// OSCortex design system widgets only** — never Material UI.
///
/// Agents compose UI via A2UI JSON → the renderer produces OS-native
/// widgets that look and feel like OSCortex, not Android.
///
/// ## Quick Start
///
/// ```dart
/// import 'package:oscortex_ui/oscortex_ui.dart';
///
/// A2UISurfaceWidget(
///   json: agentJsonPayload,
///   onAction: (action) {
///     agent.dispatch(action.name, action.params);
///   },
/// );
/// ```
///
/// ## Architecture
///
/// ```
/// Agent JSON (A2UI v0.8/v0.9)
///   → A2UIParser.parse()
///     → A2UISurface { components, data }
///       → A2UIRenderer
///         → OscButton, OscCard, OscCheckbox, OscSlider, ...
/// ```
///
/// ## Component Mapping
///
/// ### Passthrough (no design language)
/// | A2UI       | Widget                    |
/// |------------|---------------------------|
/// | Row        | Flutter Row               |
/// | Column     | Flutter Column            |
/// | List       | Flutter ListView          |
/// | Text       | Text + OscTypography      |
/// | Image      | ClipRRect + Image.network |
/// | Icon       | Icon via A2UIIcons        |
/// | Divider    | Container (1px)           |
///
/// ### Native Osc* (our design language)
/// | A2UI           | OSCortex Widget      |
/// |----------------|----------------------|
/// | Button         | OscButton            |
/// | TextField      | OscTextField         |
/// | CheckBox       | OscCheckbox          |
/// | Slider         | OscSlider            |
/// | DateTimeInput  | OscDatePicker        |
/// | MultipleChoice | OscChip              |
/// | Card           | OscCard              |
/// | Modal          | OscModal             |
/// | Tabs           | OscTabs (segments)   |
library;

export 'a2ui_data_store.dart';
export 'a2ui_icons.dart';
export 'a2ui_parser.dart';
export 'a2ui_renderer.dart';
export 'a2ui_surface.dart';
export 'a2ui_types.dart';
